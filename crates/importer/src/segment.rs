//! Pass 1 — reads one photographed dictionary page into one
//! `{headword, raw_entry}` block per bold headword, verbatim, no
//! restructuring.
use crate::engine::VisionEngine;
use crate::json_extract::extract_json_array;
use crate::schema::segment_schema;
use image::{DynamicImage, imageops::FilterType};
use std::path::{Path, PathBuf};

const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

const MAX_COMPOSITE_PIXELS: u64 = 6_000_000;

fn cap_total_pixels(img: DynamicImage, max_pixels: u64) -> DynamicImage {
    let (width, height) = (u64::from(img.width()), u64::from(img.height()));
    let total = width * height;
    if total <= max_pixels || total == 0 {
        return img;
    }
    #[allow(clippy::cast_precision_loss)]
    let (width_f, height_f, max_pixels_f, total_f) =
        (width as f64, height as f64, max_pixels as f64, total as f64);
    let scale = (max_pixels_f / total_f).sqrt();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let new_width = ((width_f * scale).round() as u64).clamp(1, u64::from(u32::MAX)) as u32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let new_height = ((height_f * scale).round() as u64).clamp(1, u64::from(u32::MAX)) as u32;
    img.resize_exact(new_width, new_height, FilterType::Lanczos3)
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Entry {
    pub headword: String,
    pub raw_entry: String,
}

const SEGMENT_PROMPT: &str = "This is a photo of a page from a dictionary. Each entry starts with a bold \
headword. Transcribe the page into one object per bold headword: \
{\"headword\": \"<the headword exactly as printed>\", \"raw_entry\": \"<the \
complete text of that entry, from the headword up to (not including) the \
next bold headword, transcribed exactly as printed, verbatim — do not \
summarize, reorder, or drop anything>\"}. Transcribe every headword on the \
page, even ones you're only partly confident about — getting the \
segmentation boundaries right matters more than getting every character \
perfect.\n\n\
Two things about how pages like this are printed: a page often prints a \
guide word in each top corner (showing the first and/or last headword on \
that page, for navigation) — these are not real entries, so never create \
one for a guide word. And an entry can run on from the bottom of one \
column straight into the top of the next column, sometimes splitting a \
single word across a line-end hyphen right at that break — keep reading \
it as the one entry it is, rather than starting a new one at the column \
break.";

pub fn segment_page(engine: &mut VisionEngine, image_path: &Path) -> Result<Vec<Entry>, String> {
    let ext = image_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if !SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "unrecognized image type for {} (expected .jpg/.jpeg/.png/.webp)",
            image_path.display()
        ));
    }

    let schema = segment_schema();
    let text = engine.generate(SEGMENT_PROMPT, image_path, Some(&schema))?;
    let context = image_path.display().to_string();
    let items = extract_json_array(&text, &context)?;
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Entry>(v).ok())
        .filter(|e| !e.headword.is_empty() && !e.raw_entry.is_empty())
        .collect())
}

fn compose_vertical_stack(images: Vec<DynamicImage>, out_path: &Path) -> Result<PathBuf, String> {
    let target_width = images
        .iter()
        .map(DynamicImage::width)
        .max()
        .ok_or("no images to stitch")?;

    let resized: Vec<DynamicImage> = images
        .into_iter()
        .map(|img| {
            if img.width() == target_width {
                img
            } else {
                let new_height_u64 =
                    u64::from(img.height()) * u64::from(target_width) / u64::from(img.width());
                let new_height = u32::try_from(new_height_u64).unwrap_or(u32::MAX).max(1);
                img.resize_exact(target_width, new_height, FilterType::Lanczos3)
            }
        })
        .collect();

    let total_height: u32 = resized.iter().map(DynamicImage::height).sum();
    let mut composite = DynamicImage::new_rgb8(target_width, total_height);
    let mut y_offset: i64 = 0;
    for img in &resized {
        image::imageops::overlay(&mut composite, img, 0, y_offset);
        y_offset += i64::from(img.height());
    }

    let composite = cap_total_pixels(composite, MAX_COMPOSITE_PIXELS);

    composite.save(out_path).map_err(|e| {
        format!(
            "failed to save stitched image to {}: {e}",
            out_path.display()
        )
    })?;
    Ok(out_path.to_path_buf())
}

const BOUNDARY_CROP_FRACTION: f64 = 0.2;

#[allow(clippy::cast_precision_loss)]
fn crop_bottom_fraction(img: &DynamicImage, fraction: f64) -> DynamicImage {
    let height = img.height();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let crop_height = ((f64::from(height) * fraction).round() as u32).max(1);
    let y = height.saturating_sub(crop_height);
    img.crop_imm(0, y, img.width(), crop_height)
}

#[allow(clippy::cast_precision_loss)]
fn crop_top_fraction(img: &DynamicImage, fraction: f64) -> DynamicImage {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let crop_height = ((f64::from(img.height()) * fraction).round() as u32).max(1);
    img.crop_imm(0, 0, img.width(), crop_height)
}

fn stitch_boundary_crop(
    top_image: &Path,
    bottom_image: &Path,
    out_path: &Path,
) -> Result<PathBuf, String> {
    let top = image::open(top_image)
        .map_err(|e| format!("failed to open image {}: {e}", top_image.display()))?;
    let bottom = image::open(bottom_image)
        .map_err(|e| format!("failed to open image {}: {e}", bottom_image.display()))?;
    let slices = vec![
        crop_bottom_fraction(&top, BOUNDARY_CROP_FRACTION),
        crop_top_fraction(&bottom, BOUNDARY_CROP_FRACTION),
    ];
    compose_vertical_stack(slices, out_path)
}

const COLUMN_SPLIT_FRACTION: f64 = 0.5;

#[allow(clippy::cast_precision_loss)]
fn crop_column(img: &DynamicImage, left_half: bool) -> DynamicImage {
    let width = img.width();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let split_x = ((f64::from(width) * COLUMN_SPLIT_FRACTION).round() as u32)
        .clamp(1, width.saturating_sub(1).max(1));
    if left_half {
        img.crop_imm(0, 0, split_x, img.height())
    } else {
        img.crop_imm(split_x, 0, width - split_x, img.height())
    }
}

fn stitch_in_page_column_boundary_crop(
    image_path: &Path,
    out_path: &Path,
) -> Result<PathBuf, String> {
    let img = image::open(image_path)
        .map_err(|e| format!("failed to open image {}: {e}", image_path.display()))?;
    let slices = vec![
        crop_bottom_fraction(&crop_column(&img, true), BOUNDARY_CROP_FRACTION),
        crop_top_fraction(&crop_column(&img, false), BOUNDARY_CROP_FRACTION),
    ];
    compose_vertical_stack(slices, out_path)
}

#[allow(clippy::cast_precision_loss)]
fn crop_band_fraction(img: &DynamicImage, start_fraction: f64, end_fraction: f64) -> DynamicImage {
    let height = img.height();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let start_y = ((f64::from(height) * start_fraction).round() as u32).min(height);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let end_y = ((f64::from(height) * end_fraction).round() as u32).clamp(start_y, height);
    img.crop_imm(0, start_y, img.width(), (end_y - start_y).max(1))
}

const CONTINUATION_CROP_BANDS: &[(f64, f64)] = &[(0.15, 0.45), (0.40, 0.70)];

fn stitch_in_page_continuation_crop(
    image_path: &Path,
    out_path: &Path,
    band: (f64, f64),
) -> Result<PathBuf, String> {
    let img = image::open(image_path)
        .map_err(|e| format!("failed to open image {}: {e}", image_path.display()))?;
    let slice = crop_band_fraction(&crop_column(&img, false), band.0, band.1);
    compose_vertical_stack(vec![slice], out_path)
}

fn extend_truncated_boundary_entry(
    engine: &mut VisionEngine,
    image_path: &Path,
    page_entries: &[Entry],
    scratch_dir: &Path,
    crop_name: &str,
    mut boundary_entries: Vec<Entry>,
) -> Result<Vec<Entry>, String> {
    for (attempt, band) in CONTINUATION_CROP_BANDS.iter().enumerate() {
        let folded_preview =
            fold_unanchored_boundary_entries(page_entries, boundary_entries.clone());
        let Some(target) = folded_preview.last() else {
            break;
        };
        let crop_itself_looks_cut_off =
            attempt == 0 && raw_last_entry_is_unanchored(page_entries, &boundary_entries);
        if ends_with_terminal_punctuation(&target.raw_entry) && !crop_itself_looks_cut_off {
            break;
        }
        let target_headword = target.headword.to_lowercase();
        let crop_path = scratch_dir.join(format!("continuation-crop-{attempt}-{crop_name}.png"));
        let crop = stitch_in_page_continuation_crop(image_path, &crop_path, *band)?;
        let continuation_entries = segment_page(engine, &crop)?;
        println!(
            "  found {} headword(s) in continuation crop {attempt} for \"{target_headword}\"",
            continuation_entries.len()
        );
        let Some(first) = continuation_entries.first() else {
            break;
        };
        if continuation_crop_moved_past_target(page_entries, &target_headword, &first.headword) {
            break;
        }
        if let Some(last) = boundary_entries.last_mut() {
            last.raw_entry = format!("{} {}", last.raw_entry.trim_end(), first.raw_entry.trim());
        }
    }
    Ok(boundary_entries)
}

fn continuation_crop_moved_past_target(
    page_entries: &[Entry],
    target_headword: &str,
    first_continuation_headword: &str,
) -> bool {
    let first_continuation_headword = first_continuation_headword.to_lowercase();
    first_continuation_headword != target_headword
        && page_entries
            .iter()
            .any(|e| e.headword.to_lowercase() == first_continuation_headword)
}

fn raw_last_entry_is_unanchored(page_entries: &[Entry], boundary_entries: &[Entry]) -> bool {
    boundary_entries.last().is_some_and(|last| {
        !page_entries
            .iter()
            .any(|e| e.headword.to_lowercase() == last.headword.to_lowercase())
    })
}

const OVERLAP_SEARCH_WINDOW: usize = 5;

fn append_after_overlap(accumulated: &mut Vec<Entry>, new_entries: Vec<Entry>) {
    let Some(last) = accumulated.last() else {
        accumulated.extend(new_entries);
        return;
    };
    let last_headword = last.headword.to_lowercase();
    let overlap_pos = new_entries
        .iter()
        .take(OVERLAP_SEARCH_WINDOW)
        .position(|e| e.headword.to_lowercase() == last_headword);
    match overlap_pos {
        Some(pos) => accumulated.extend(new_entries.into_iter().skip(pos + 1)),
        None => accumulated.extend(new_entries),
    }
}

const COLUMN_FIX_MIN_REPLACE_WINDOW: usize = 2;

fn fold_unanchored_boundary_entries(
    page_entries: &[Entry],
    boundary_entries: Vec<Entry>,
) -> Vec<Entry> {
    let mut result: Vec<Entry> = Vec::with_capacity(boundary_entries.len());
    for entry in boundary_entries {
        let is_anchored = page_entries
            .iter()
            .any(|e| e.headword.to_lowercase() == entry.headword.to_lowercase());
        if !is_anchored && let Some(previous) = result.last_mut() {
            previous.raw_entry = format!(
                "{} {}",
                previous.raw_entry.trim_end(),
                entry.raw_entry.trim()
            );
            continue;
        }
        result.push(entry);
    }
    result
}

fn apply_column_boundary_fix(page_entries: Vec<Entry>, boundary_entries: &[Entry]) -> Vec<Entry> {
    let Some(first_matched_idx) = boundary_entries.iter().position(|b| {
        let bh = b.headword.to_lowercase();
        page_entries.iter().any(|e| e.headword.to_lowercase() == bh)
    }) else {
        return page_entries;
    };
    let trimmed_boundary_entries = boundary_entries[first_matched_idx..].to_vec();
    let raw_boundary_entry_count = trimmed_boundary_entries.len();
    let boundary_entries =
        fold_unanchored_boundary_entries(&page_entries, trimmed_boundary_entries);
    let first_headword = boundary_entries[0].headword.to_lowercase();
    let Some(start_idx) = page_entries
        .iter()
        .rposition(|e| e.headword.to_lowercase() == first_headword)
    else {
        return page_entries;
    };
    let boundary_headwords: std::collections::HashSet<String> = boundary_entries
        .iter()
        .map(|e| e.headword.to_lowercase())
        .collect();
    let search_limit = (start_idx + raw_boundary_entry_count + COLUMN_FIX_MIN_REPLACE_WINDOW)
        .min(page_entries.len());
    let matched_end = page_entries[start_idx..search_limit]
        .iter()
        .rposition(|e| boundary_headwords.contains(&e.headword.to_lowercase()))
        .map_or(start_idx, |rel_pos| start_idx + rel_pos + 1);
    let end_idx = if raw_boundary_entry_count == 1 {
        (start_idx + COLUMN_FIX_MIN_REPLACE_WINDOW).max(matched_end)
    } else {
        matched_end
    }
    .min(page_entries.len());
    let mut result = page_entries;
    result.splice(start_idx..end_idx, boundary_entries);
    result
}

fn apply_boundary_fix_to_previous_page_tail(
    mut accumulated: Vec<Entry>,
    boundary_entries: &[Entry],
) -> Vec<Entry> {
    let Some(last) = accumulated.last() else {
        return accumulated;
    };
    let last_headword = last.headword.to_lowercase();
    if let Some(replacement) = boundary_entries
        .iter()
        .find(|e| e.headword.to_lowercase() == last_headword)
    {
        let last_idx = accumulated.len() - 1;
        accumulated[last_idx] = replacement.clone();
    }
    accumulated
}

fn drop_leading_duplicate_headword(mut entries: Vec<Entry>) -> Vec<Entry> {
    if entries.len() >= 2
        && entries[0].headword.to_lowercase() == entries[1].headword.to_lowercase()
    {
        entries.remove(0);
    }
    entries
}

fn segment_page_with_column_fix(
    engine: &mut VisionEngine,
    image_path: &Path,
    scratch_dir: &Path,
) -> Result<Vec<Entry>, String> {
    let page_entries = drop_leading_duplicate_headword(segment_page(engine, image_path)?);
    let crop_name = image_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page");
    let crop_path = scratch_dir.join(format!("column-boundary-crop-{crop_name}.png"));
    let crop = stitch_in_page_column_boundary_crop(image_path, &crop_path)?;
    let boundary_entries = segment_page(engine, &crop)?;
    println!(
        "  found {} headword(s) in this page's own column-break crop",
        boundary_entries.len()
    );
    let boundary_entries = extend_truncated_boundary_entry(
        engine,
        image_path,
        &page_entries,
        scratch_dir,
        crop_name,
        boundary_entries,
    )?;
    Ok(apply_column_boundary_fix(page_entries, &boundary_entries))
}

fn page_sort_key(entries: &[Entry]) -> Option<String> {
    entries
        .get(1)
        .or_else(|| entries.first())
        .map(|e| e.headword.to_lowercase())
}

fn order_images_by_page_content(
    engine: &mut VisionEngine,
    image_paths: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    let mut keyed = Vec::with_capacity(image_paths.len());
    for image_path in image_paths {
        println!(
            "skimming {} to determine its page order...",
            image_path.display()
        );
        let entries = segment_page(engine, image_path)?;
        let key = page_sort_key(&entries);
        println!(
            "  sorts near \"{}\"",
            key.as_deref().unwrap_or("(unknown — no entries found)")
        );
        keyed.push((image_path.clone(), key));
    }
    keyed.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(keyed.into_iter().map(|(path, _)| path).collect())
}

/// Processes N pages as N-1 *overlapping* pairs — (page 1, page 2), (page
/// 2, page 3), ... — rather than stitching all N into one ever-growing
/// composite. A single mega-composite doesn't scale: fed a real
/// thousand-page document, its own visual footprint (and the output
/// needed to transcribe all of it) would blow well past any practical
/// context window long before reaching the end. A pair call only ever
/// costs as much as two pages, no matter how many pages the whole
/// document has. `image_paths` must already be in page order — see
/// `order_images_by_page_content`.
///
/// Each page is read independently at full resolution (reliable all
/// session), and, between each pair, a small crop of just the boundary
/// between them (`stitch_boundary_crop`) is read too, to catch whatever
/// straddles the join. That crop's own result is used only to patch
/// the previous page's own last entry if it was truncated
/// (`apply_boundary_fix_to_previous_page_tail`), not appended wholesale
/// — confirmed in a real run that the crop's own read past that one
/// patched entry can be less reliable than the next page's own full
/// read of the same content, and blindly trusting it caused that
/// better, later read to be discarded as redundant overlap instead.
/// `append_after_overlap` (via `segment_page_with_column_fix`'s own
/// caller, at the top of the next page's turn through this loop) still
/// handles the ordinary case of a page's own read re-finding a couple
/// of entries already seen. This replaced an earlier design that
/// stitched each pair's
/// *whole* pages into one composite: that composite's extreme aspect
/// ratio (two tall pages stacked) reproducibly crashed Qwen2.5-VL's
/// vision path in llama.cpp with "failed to find a memory slot" — a
/// known, documented upstream difficulty with this model family's
/// tiling (the maintainers' own fix caps Qwen2VL/Pixtral inputs for
/// exactly this reason) — confirmed to be about the composite's shape,
/// not raw image size: downscaling it changed nothing, while ordinary
/// single-page photos and small boundary crops, at a similar or even
/// higher token count, never hit it.
fn run_segment_sliding_window_cropped(
    engine: &mut VisionEngine,
    image_paths: &[PathBuf],
    entries_out_file: &Path,
) -> Result<Vec<Entry>, String> {
    let scratch_dir = entries_out_file.parent().unwrap_or_else(|| Path::new("."));
    let mut all_entries: Vec<Entry> = Vec::new();
    for (i, image_path) in image_paths.iter().enumerate() {
        println!(
            "reading page {} ({}/{})...",
            i + 1,
            i + 1,
            image_paths.len()
        );
        let found = segment_page_with_column_fix(engine, image_path, scratch_dir)?;
        println!("  found {} headword(s)", found.len());
        append_after_overlap(&mut all_entries, found);

        if let Some(next_image_path) = image_paths.get(i + 1) {
            println!(
                "reading the boundary between pages {} and {}...",
                i + 1,
                i + 2
            );
            let crop_path = scratch_dir.join(format!("boundary-crop-{}-{}.png", i + 1, i + 2));
            let crop = stitch_boundary_crop(image_path, next_image_path, &crop_path)?;
            let boundary_found = segment_page(engine, &crop)?;
            println!(
                "  found {} headword(s) in the boundary crop",
                boundary_found.len()
            );
            all_entries = apply_boundary_fix_to_previous_page_tail(all_entries, &boundary_found);
        }
    }
    Ok(all_entries)
}

fn merge_leading_parenthetical_continuations(entries: Vec<Entry>) -> Vec<Entry> {
    let mut result: Vec<Entry> = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.raw_entry.trim_start().starts_with('(')
            && let Some(previous) = result.last_mut()
        {
            previous.raw_entry = format!(
                "{} {}",
                previous.raw_entry.trim_end(),
                entry.raw_entry.trim()
            );
            continue;
        }
        result.push(entry);
    }
    result
}

fn ends_with_terminal_punctuation(raw_entry: &str) -> bool {
    raw_entry.trim_end().ends_with(['.', ';', ',', ')'])
}

fn merge_entries_with_no_terminal_punctuation(entries: Vec<Entry>) -> Vec<Entry> {
    let mut result: Vec<Entry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let is_continuation = result
            .last()
            .is_some_and(|previous: &Entry| !ends_with_terminal_punctuation(&previous.raw_entry));
        if is_continuation {
            let previous = result.last_mut().expect("checked by is_continuation above");
            previous.raw_entry = format!(
                "{} {}",
                previous.raw_entry.trim_end(),
                entry.raw_entry.trim()
            );
            continue;
        }
        result.push(entry);
    }
    result
}

fn truncated_entry_indices(entries: &[Entry]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !ends_with_terminal_punctuation(&e.raw_entry))
        .map(|(i, _)| i)
        .collect()
}

fn strip_latin_accents(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        'ñ' => 'n',
        'ç' => 'c',
        other => other,
    }
}

fn diacritic_insensitive_lowercase(s: &str) -> String {
    s.to_lowercase().chars().map(strip_latin_accents).collect()
}

fn headword_order_anomalies(entries: &[Entry]) -> Vec<usize> {
    let mut anomalies = Vec::new();
    for i in 1..entries.len() {
        let previous = diacritic_insensitive_lowercase(&entries[i - 1].headword);
        let current = diacritic_insensitive_lowercase(&entries[i].headword);
        if current < previous {
            anomalies.push(i);
        }
    }
    anomalies
}

#[derive(serde::Serialize)]
struct ReviewNote<'a> {
    headword: &'a str,
    raw_entry: &'a str,
    reason: &'static str,
}

fn needs_review_path(entries_out_file: &Path) -> PathBuf {
    let stem = entries_out_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("entries");
    let dir = entries_out_file.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!("{stem}.needs-review.json"))
}

pub fn run_segment(
    model_path: &Path,
    mmproj_path: &Path,
    entries_out_file: &Path,
    image_paths: &[PathBuf],
) -> Result<(), String> {
    println!("loading segment model from {} ...", model_path.display());
    let mut engine = VisionEngine::load(model_path, mmproj_path)?;

    if let Some(parent) = entries_out_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let entries = if image_paths.len() > 1 {
        let ordered = order_images_by_page_content(&mut engine, image_paths)?;
        run_segment_sliding_window_cropped(&mut engine, &ordered, entries_out_file)?
    } else {
        let first_image = image_paths.first().ok_or("no images given")?;
        println!("reading {} ...", first_image.display());
        let scratch_dir = entries_out_file.parent().unwrap_or_else(|| Path::new("."));
        let found = segment_page_with_column_fix(&mut engine, first_image, scratch_dir)?;
        println!("  found {} headword(s)", found.len());
        found
    };
    let entries = merge_leading_parenthetical_continuations(entries);
    let entries = merge_entries_with_no_terminal_punctuation(entries);

    let mut review_notes = Vec::new();
    for idx in truncated_entry_indices(&entries) {
        let e = &entries[idx];
        println!(
            "  WARNING: \"{}\" (entry {}) has no terminal punctuation — looks truncated, worth a manual look",
            e.headword,
            idx + 1
        );
        review_notes.push(ReviewNote {
            headword: &e.headword,
            raw_entry: &e.raw_entry,
            reason: "entry ends without terminal punctuation — likely cut off by a page or crop edge",
        });
    }
    for idx in headword_order_anomalies(&entries) {
        println!(
            "  WARNING: \"{}\" (entry {}) sorts before the preceding entry \"{}\" — possible segmentation bug, worth a manual look",
            entries[idx].headword,
            idx + 1,
            entries[idx - 1].headword
        );
        review_notes.push(ReviewNote {
            headword: &entries[idx].headword,
            raw_entry: &entries[idx].raw_entry,
            reason: "headword sorts out of alphabetical order relative to the preceding entry",
        });
    }
    if !review_notes.is_empty() {
        let review_path = needs_review_path(entries_out_file);
        let review_json = serde_json::to_string_pretty(&review_notes).map_err(|e| e.to_string())?;
        std::fs::write(&review_path, review_json).map_err(|e| e.to_string())?;
        println!(
            "wrote {} entr{} needing review to {}",
            review_notes.len(),
            if review_notes.len() == 1 { "y" } else { "ies" },
            review_path.display()
        );
    }

    let json = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
    std::fs::write(entries_out_file, json).map_err(|e| e.to_string())?;
    println!(
        "wrote {} entries to {}",
        entries.len(),
        entries_out_file.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(headword: &str, raw_entry: &str) -> Entry {
        Entry {
            headword: headword.to_string(),
            raw_entry: raw_entry.to_string(),
        }
    }

    #[test]
    fn cap_total_pixels_leaves_a_small_image_unchanged() {
        let img = DynamicImage::new_rgb8(100, 200);
        let capped = cap_total_pixels(img, 1_000_000);
        assert_eq!((capped.width(), capped.height()), (100, 200));
    }

    #[test]
    fn cap_total_pixels_downscales_an_oversized_image_preserving_aspect_ratio() {
        let img = DynamicImage::new_rgb8(2000, 4000); // 8,000,000 px, 1:2 ratio
        let capped = cap_total_pixels(img, 2_000_000);
        let total = u64::from(capped.width()) * u64::from(capped.height());
        assert!(total <= 2_000_000, "capped total was {total}");
        // Aspect ratio preserved (within integer rounding).
        assert!((f64::from(capped.height()) / f64::from(capped.width()) - 2.0).abs() < 0.01);
    }

    #[test]
    fn drop_leading_duplicate_headword_drops_only_the_first_of_a_leading_pair() {
        // Real run: a page's own first entry for "abscess" carried the
        // missing tail of the previous page's "abrupt" entry, mislabeled,
        // immediately followed by a correctly-labeled "abscess".
        let entries = vec![
            entry("abscess", "ubukrwada; iqubuliso (ili-ama-); ubuqengqelezi."),
            entry("abscess", "n. ithumba (ili- ama-)."),
            entry("abscond", "..."),
        ];
        let result = drop_leading_duplicate_headword(entries);
        assert_eq!(
            result.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abscess", "abscond"]
        );
        assert_eq!(result[0].raw_entry, "n. ithumba (ili- ama-).");
    }

    #[test]
    fn drop_leading_duplicate_headword_leaves_a_page_without_a_leading_duplicate_unchanged() {
        let entries = vec![entry("abode", "..."), entry("abolish", "...")];
        let result = drop_leading_duplicate_headword(entries);
        assert_eq!(
            result.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abode", "abolish"]
        );
    }

    #[test]
    fn merge_leading_parenthetical_continuations_folds_a_dangling_fragment_into_the_previous_entry()
    {
        let entries = vec![
            entry(
                "abet",
                "vt. (legal: help somebody in doing wrong) -ncedisa ekwenzeni ububi;",
            ),
            entry(
                "aboard",
                "(encourage vice, crime) -khuthaza/-thele-kisa okungendawo.",
            ),
            entry("abeyance", "n. ukumiswa, ukuxhonywa."),
        ];
        let result = merge_leading_parenthetical_continuations(entries);
        assert_eq!(
            result.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abet", "abeyance"]
        );
        assert_eq!(
            result[0].raw_entry,
            "vt. (legal: help somebody in doing wrong) -ncedisa ekwenzeni ububi; \
             (encourage vice, crime) -khuthaza/-thele-kisa okungendawo."
        );
    }

    #[test]
    fn merge_leading_parenthetical_continuations_leaves_ordinary_entries_unchanged() {
        let entries = vec![
            entry("abode", "n. ikhaya."),
            entry("abolish", "vt. -bhangisa."),
        ];
        let result = merge_leading_parenthetical_continuations(entries.clone());
        assert_eq!(result, entries);
    }

    #[test]
    fn merge_leading_parenthetical_continuations_leaves_a_leading_fragment_with_nothing_before_it_alone()
     {
        let entries = vec![
            entry("aboard", "(stray fragment)"),
            entry("abode", "n. ikhaya."),
        ];
        let result = merge_leading_parenthetical_continuations(entries.clone());
        assert_eq!(result, entries);
    }

    #[test]
    fn merge_entries_with_no_terminal_punctuation_folds_a_cut_off_entry_into_the_next_one() {
        let entries = vec![
            entry("abstruse", "..."),
            entry("absurd", "adj. (unreasonable) -ngafanele"),
            entry(
                "accelerate",
                "kanga; (foolish, ridiculous) -nobudenge; ~-ity, n. ubudenge.",
            ),
            entry("abundant", "..."),
        ];
        let result = merge_entries_with_no_terminal_punctuation(entries);
        assert_eq!(
            result.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abstruse", "absurd", "abundant"]
        );
        assert_eq!(
            result[1].raw_entry,
            "adj. (unreasonable) -ngafanele kanga; (foolish, ridiculous) -nobudenge; \
             ~-ity, n. ubudenge."
        );
    }

    #[test]
    fn merge_entries_with_no_terminal_punctuation_leaves_ordinary_entries_unchanged() {
        let entries = vec![
            entry("abode", "n. ikhaya."),
            entry("abolish", "vt. -bhangisa."),
        ];
        let result = merge_entries_with_no_terminal_punctuation(entries.clone());
        assert_eq!(result, entries);
    }

    #[test]
    fn merge_entries_with_no_terminal_punctuation_leaves_a_dangling_last_entry_alone() {
        let entries = vec![
            entry("abode", "n. ikhaya."),
            entry("abolish", "vt. -bhangisa, -shenx"),
        ];
        let result = merge_entries_with_no_terminal_punctuation(entries.clone());
        assert_eq!(result, entries);
    }

    #[test]
    fn truncated_entry_indices_finds_entries_with_nothing_left_to_merge() {
        let entries = vec![
            entry("abode", "n. ikhaya."),
            entry("abolish", "vt. -bhangisa, -shenx"),
            entry("abominate", "vt. -cekisa"),
        ];
        assert_eq!(truncated_entry_indices(&entries), vec![1, 2]);
    }

    #[test]
    fn truncated_entry_indices_is_empty_when_every_entry_is_complete() {
        let entries = vec![entry("abode", "n. ikhaya."), entry("abolish", "vt.;")];
        assert_eq!(truncated_entry_indices(&entries), Vec::<usize>::new());
    }

    #[test]
    fn headword_order_anomalies_flags_the_real_accelerate_case() {
        let entries = vec![
            entry("absurd", "..."),
            entry("accelerate", "..."),
            entry("abundant", "..."),
        ];
        assert_eq!(headword_order_anomalies(&entries), vec![2]);
    }

    #[test]
    fn headword_order_anomalies_allows_a_headword_repeated_for_a_distinct_sense() {
        let entries = vec![
            entry("about", "first sense"),
            entry("about", "second sense"),
            entry("about", "third sense"),
            entry("above", "..."),
        ];
        assert_eq!(headword_order_anomalies(&entries), Vec::<usize>::new());
    }

    #[test]
    fn headword_order_anomalies_is_diacritic_insensitive() {
        let entries = vec![entry("abbé", "..."), entry("abbess", "...")];
        assert_eq!(headword_order_anomalies(&entries), Vec::<usize>::new());
    }

    #[test]
    fn crop_column_splits_the_image_in_half_with_no_gap_or_overlap() {
        let img = DynamicImage::new_rgb8(1000, 2000);
        let left = crop_column(&img, true);
        let right = crop_column(&img, false);
        assert_eq!(left.height(), 2000);
        assert_eq!(right.height(), 2000);
        assert_eq!(left.width() + right.width(), 1000);
        assert_eq!(left.width(), 500);
    }

    #[test]
    fn crop_band_fraction_extracts_the_requested_slice_of_the_height() {
        let img = DynamicImage::new_rgb8(100, 1000);
        let band = crop_band_fraction(&img, 0.15, 0.45);
        assert_eq!(band.height(), 300);
        assert_eq!(band.width(), 100);
    }

    #[test]
    fn crop_band_fraction_never_panics_when_fractions_are_reversed() {
        let img = DynamicImage::new_rgb8(100, 1000);
        let band = crop_band_fraction(&img, 0.9, 0.1);
        assert!(band.height() >= 1);
    }

    #[test]
    fn continuation_crop_stops_at_a_different_real_headword() {
        let page_entries = vec![
            entry("about", "..."),
            entry("above", "adv. (at a higher point...)"),
        ];
        assert!(continuation_crop_moved_past_target(
            &page_entries,
            "about",
            "above"
        ));
    }

    #[test]
    fn continuation_crop_keeps_going_on_an_unanchored_pseudo_headword() {
        let page_entries = vec![entry("about", "...")];
        assert!(!continuation_crop_moved_past_target(
            &page_entries,
            "about",
            "apha"
        ));
    }

    #[test]
    fn continuation_crop_keeps_going_when_it_repeats_the_target_headword() {
        let page_entries = vec![entry("about", "...")];
        assert!(!continuation_crop_moved_past_target(
            &page_entries,
            "about",
            "About"
        ));
    }

    #[test]
    fn raw_last_entry_is_unanchored_detects_the_real_about_prep_case() {
        let page_entries = vec![entry("about", "..."), entry("above", "...")];
        let boundary_entries = vec![
            entry("about", "..."),
            entry("apha", "naphaya egumbini lakhe;"),
        ];
        assert!(raw_last_entry_is_unanchored(
            &page_entries,
            &boundary_entries
        ));
    }

    #[test]
    fn raw_last_entry_is_unanchored_is_false_when_the_crop_ends_on_a_real_headword() {
        let page_entries = vec![entry("about", "..."), entry("above", "...")];
        let boundary_entries = vec![entry("about", "..."), entry("above", "...")];
        assert!(!raw_last_entry_is_unanchored(
            &page_entries,
            &boundary_entries
        ));
    }

    #[test]
    fn raw_last_entry_is_unanchored_is_false_when_boundary_entries_is_empty() {
        let page_entries = vec![entry("about", "...")];
        assert!(!raw_last_entry_is_unanchored(&page_entries, &[]));
    }

    #[test]
    fn column_boundary_fix_replaces_a_corrupted_pair_with_the_crops_own_reading() {
        let page_entries = vec![
            entry("abstruse", "..."),
            entry("absurd", "(unreasonable) -ngafanele-"),
            entry("kanga", "(foolish, ridiculous) -nobudenge;"),
            entry("abundant", "..."),
        ];
        let boundary_entries = vec![entry(
            "absurd",
            "(unreasonable) -ngafanele-kanga; (foolish, ridiculous) -nobudenge;",
        )];
        let fixed = apply_column_boundary_fix(page_entries, &boundary_entries);
        assert_eq!(
            fixed.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abstruse", "absurd", "abundant"]
        );
        assert_eq!(
            fixed[1].raw_entry,
            "(unreasonable) -ngafanele-kanga; (foolish, ridiculous) -nobudenge;"
        );
    }

    #[test]
    fn column_boundary_fix_extends_past_the_minimum_window_to_cover_everything_the_crop_also_found()
    {
        let page_entries = vec![
            entry("abdomen", "..."),
            entry("abduct", "..."),
            entry("aberration", "..."),
            entry("abet", "..."),
            entry("(encourage vice, crime)", "garbage"),
            entry("abeyance", "..."),
            entry("abhor", "..."),
            entry("abide", "..."),
            entry("abject", "real, unrelated"),
            entry("abjure", "real, unrelated"),
            entry("aboard", "later distinct sense"),
        ];
        let boundary_entries = vec![
            entry("abduct", "..."),
            entry("aberration", "..."),
            entry("abet", "..."),
            entry("aboard", "correct, early sense"),
            entry("abeyance", "..."),
            entry("abhor", "..."),
            entry("abide", "..."),
        ];
        let fixed = apply_column_boundary_fix(page_entries, &boundary_entries);
        assert_eq!(
            fixed.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            [
                "abdomen",
                "abduct",
                "aberration",
                "abet",
                "aboard",
                "abeyance",
                "abhor",
                "abide",
                "abject",
                "abjure",
                "aboard",
            ]
        );
        assert_eq!(fixed[4].raw_entry, "correct, early sense");
        assert_eq!(fixed[8].raw_entry, "real, unrelated");
        assert_eq!(fixed[9].raw_entry, "real, unrelated");
        assert_eq!(fixed[10].raw_entry, "later distinct sense");
    }

    #[test]
    fn column_boundary_fix_skips_a_hallucinated_leading_pseudo_headword_in_the_crop() {
        let page_entries = vec![
            entry("about", "earlier, unrelated sense"),
            entry("about", "(with verbs of movement) ... ~·face, vi. -jika."),
            entry("about", "prep. (with verbs of movement) ... kukho iincwadi"),
            entry("above", "adv. (at a higher point...)"),
        ];
        let boundary_entries = vec![
            entry("ezininzi", "stray continuation of the previous entry"),
            entry("about", "prep. (with verbs of movement) ... kukho iincwadi"),
            entry("apha", "naphaya egumbini lakhe; (near to) kufuphi na-;"),
        ];
        let fixed = apply_column_boundary_fix(page_entries, &boundary_entries);
        assert_eq!(
            fixed.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["about", "about", "about", "above"]
        );
        assert_eq!(
            fixed[2].raw_entry,
            "prep. (with verbs of movement) ... kukho iincwadi naphaya egumbini lakhe; (near to) kufuphi na-;"
        );
    }

    #[test]
    fn column_boundary_fix_matches_the_last_occurrence_of_a_repeated_headword_not_the_first() {
        let page_entries = vec![
            entry("about", "first, unrelated sense"),
            entry("about", "second, unrelated sense"),
            entry("about", "truncated at the column break"),
            entry("above", "..."),
        ];
        let boundary_entries = vec![
            entry("about", "complete, from the crop"),
            entry("above", "also confirmed by the crop"),
        ];
        let fixed = apply_column_boundary_fix(page_entries, &boundary_entries);
        assert_eq!(
            fixed.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["about", "about", "about", "above"]
        );
        assert_eq!(fixed[0].raw_entry, "first, unrelated sense");
        assert_eq!(fixed[1].raw_entry, "second, unrelated sense");
        assert_eq!(fixed[2].raw_entry, "complete, from the crop");
        assert_eq!(fixed[3].raw_entry, "also confirmed by the crop");
    }

    #[test]
    fn column_boundary_fix_leaves_page_entries_untouched_when_the_crop_found_nothing_matching() {
        let page_entries = vec![entry("abode", "...")];
        let boundary_entries = vec![entry("zebra", "...")];
        let fixed = apply_column_boundary_fix(page_entries, &boundary_entries);
        assert_eq!(
            fixed.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abode"]
        );
    }

    #[test]
    fn boundary_fix_to_previous_page_tail_replaces_a_truncated_last_entry() {
        let accumulated = vec![
            entry("abroad", "..."),
            entry("abrupt", "adj. ... ~ness, n."),
        ];
        let boundary_entries = vec![
            entry("about", "..."),
            entry("abrupt", "adj. ... ~ness, n. ubudenge."),
        ];
        let fixed = apply_boundary_fix_to_previous_page_tail(accumulated, &boundary_entries);
        assert_eq!(
            fixed.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abroad", "abrupt"]
        );
        assert_eq!(fixed[1].raw_entry, "adj. ... ~ness, n. ubudenge.");
    }

    #[test]
    fn boundary_fix_to_previous_page_tail_leaves_it_unchanged_when_no_match_is_found() {
        let accumulated = vec![entry("abroad", "..."), entry("abrupt", "...")];
        let boundary_entries = vec![entry("zebra", "...")];
        let fixed = apply_boundary_fix_to_previous_page_tail(accumulated, &boundary_entries);
        assert_eq!(
            fixed.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abroad", "abrupt"]
        );
        assert_eq!(fixed[1].raw_entry, "...");
    }

    #[test]
    fn column_boundary_fix_is_a_no_op_when_the_crop_found_nothing() {
        let page_entries = vec![entry("abode", "...")];
        let fixed = apply_column_boundary_fix(page_entries, &[]);
        assert_eq!(
            fixed.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abode"]
        );
    }

    #[test]
    fn page_sort_key_prefers_the_second_entry_over_a_runover_first_entry() {
        let entries = vec![
            entry("(encourage vice, crime)", "orphaned continuation text"),
            entry("abeyance", "..."),
            entry("abhor", "..."),
        ];
        assert_eq!(page_sort_key(&entries).as_deref(), Some("abeyance"));
    }

    #[test]
    fn page_sort_key_falls_back_to_the_first_entry_when_theres_only_one() {
        let entries = vec![entry("abode", "...")];
        assert_eq!(page_sort_key(&entries).as_deref(), Some("abode"));
    }

    #[test]
    fn page_sort_key_is_none_for_a_page_with_no_entries() {
        assert_eq!(page_sort_key(&[]), None);
    }

    #[test]
    fn appends_everything_when_accumulated_is_empty() {
        let mut accumulated = Vec::new();
        append_after_overlap(
            &mut accumulated,
            vec![entry("abode", "..."), entry("abolish", "...")],
        );
        assert_eq!(
            accumulated.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abode", "abolish"]
        );
    }

    #[test]
    fn skips_the_redundant_overlap_and_appends_only_whats_new() {
        let mut accumulated = vec![
            entry("abode", "..."),
            entry("abolish", "..."),
            entry("abominate", "..."),
        ];
        append_after_overlap(
            &mut accumulated,
            vec![
                entry("abominate", "..."),
                entry("aboriginal", "..."),
                entry("aborigines", "..."),
            ],
        );
        assert_eq!(
            accumulated.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abode", "abolish", "abominate", "aboriginal", "aborigines"]
        );
    }

    #[test]
    fn matches_headwords_case_insensitively() {
        let mut accumulated = vec![entry("Abode", "...")];
        append_after_overlap(
            &mut accumulated,
            vec![entry("abode", "..."), entry("abolish", "...")],
        );
        assert_eq!(
            accumulated.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["Abode", "abolish"]
        );
    }

    #[test]
    fn appends_everything_rather_than_drop_content_when_no_overlap_is_found() {
        let mut accumulated = vec![entry("abode", "...")];
        append_after_overlap(&mut accumulated, vec![entry("zebra", "...")]);
        assert_eq!(
            accumulated.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abode", "zebra"]
        );
    }

    #[test]
    fn ignores_a_distant_coincidental_match_instead_of_dropping_everything_before_it() {
        let mut accumulated = vec![entry("absurd", "...")];
        append_after_overlap(
            &mut accumulated,
            vec![
                entry("abscess", "..."),
                entry("abscond", "..."),
                entry("absent", "..."),
                entry("absinthe", "..."),
                entry("absolute", "..."),
                entry("absolve", "..."),
                entry("abstain", "..."),
                entry("abstemious", "..."),
                entry("abstract", "..."),
                entry("abstruse", "..."),
                entry("absurd", "..."),
                entry("accelerate", "..."),
            ],
        );
        assert_eq!(
            accumulated.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            [
                "absurd",
                "abscess",
                "abscond",
                "absent",
                "absinthe",
                "absolute",
                "absolve",
                "abstain",
                "abstemious",
                "abstract",
                "abstruse",
                "absurd",
                "accelerate",
            ]
        );
    }

    fn entries_from(headwords: &[String]) -> Vec<Entry> {
        headwords.iter().map(|h| entry(h, "x")).collect()
    }

    fn headwords_of(entries: &[Entry]) -> Vec<String> {
        entries.iter().map(|e| e.headword.clone()).collect()
    }

    proptest::proptest! {
        #[test]
        fn append_after_overlap_never_touches_the_existing_prefix(
            accumulated_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
            new_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let mut accumulated = entries_from(&accumulated_headwords);
            let original_len = accumulated.len();
            let new_entries = entries_from(&new_headwords);
            append_after_overlap(&mut accumulated, new_entries);
            proptest::prop_assert!(accumulated.len() >= original_len);
            proptest::prop_assert_eq!(&headwords_of(&accumulated[..original_len]), &accumulated_headwords);
        }

        #[test]
        fn append_after_overlap_with_empty_accumulated_copies_new_entries_verbatim(
            new_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let mut accumulated: Vec<Entry> = Vec::new();
            let new_entries = entries_from(&new_headwords);
            append_after_overlap(&mut accumulated, new_entries);
            proptest::prop_assert_eq!(headwords_of(&accumulated), new_headwords);
        }

        #[test]
        fn column_boundary_fix_is_a_no_op_when_boundary_entries_is_empty(
            page_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let page_entries = entries_from(&page_headwords);
            let result = apply_column_boundary_fix(page_entries, &[]);
            proptest::prop_assert_eq!(headwords_of(&result), page_headwords);
        }

        #[test]
        fn column_boundary_fix_is_a_no_op_when_no_match_is_found(
            page_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
            boundary_headwords in proptest::collection::vec("[a-z]{1,6}", 1..5),
        ) {
            proptest::prop_assume!(
                !boundary_headwords.iter().any(|b|
                    page_headwords.iter().any(|h| h.eq_ignore_ascii_case(b))
                )
            );
            let page_entries = entries_from(&page_headwords);
            let boundary_entries = entries_from(&boundary_headwords);
            let result = apply_column_boundary_fix(page_entries, &boundary_entries);
            proptest::prop_assert_eq!(headwords_of(&result), page_headwords);
        }

        #[test]
        fn column_boundary_fix_preserves_everything_before_the_match(
            before_headwords in proptest::collection::vec("[a-z]{1,6}", 0..4),
            match_headword in "[a-z]{1,6}",
            after_headwords in proptest::collection::vec("[a-z]{1,6}", 0..4),
            boundary_rest in proptest::collection::vec("[a-z]{1,6}", 0..3),
        ) {
            proptest::prop_assume!(
                !before_headwords.iter().any(|h| h.eq_ignore_ascii_case(&match_headword))
            );
            let mut page_entries = entries_from(&before_headwords);
            page_entries.push(entry(&match_headword, "x"));
            page_entries.extend(entries_from(&after_headwords));

            let mut boundary_entries = vec![entry(&match_headword, "y")];
            boundary_entries.extend(entries_from(&boundary_rest));

            let before_len = before_headwords.len();
            let result = apply_column_boundary_fix(page_entries, &boundary_entries);
            proptest::prop_assert!(result.len() >= before_len);
            proptest::prop_assert_eq!(headwords_of(&result[..before_len]), before_headwords);
        }

        #[test]
        fn boundary_fix_to_previous_page_tail_never_changes_length(
            accumulated_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
            boundary_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let accumulated = entries_from(&accumulated_headwords);
            let original_len = accumulated.len();
            let boundary_entries = entries_from(&boundary_headwords);
            let result = apply_boundary_fix_to_previous_page_tail(accumulated, &boundary_entries);
            proptest::prop_assert_eq!(result.len(), original_len);
        }

        #[test]
        fn boundary_fix_to_previous_page_tail_is_a_no_op_when_no_match_is_found(
            accumulated_headwords in proptest::collection::vec("[a-z]{1,6}", 1..8),
            boundary_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let last_headword = accumulated_headwords.last().unwrap().clone();
            proptest::prop_assume!(
                !boundary_headwords.iter().any(|h| h.eq_ignore_ascii_case(&last_headword))
            );
            let accumulated = entries_from(&accumulated_headwords);
            let boundary_entries = entries_from(&boundary_headwords);
            let result = apply_boundary_fix_to_previous_page_tail(accumulated, &boundary_entries);
            proptest::prop_assert_eq!(headwords_of(&result), accumulated_headwords);
        }

        #[test]
        fn drop_leading_duplicate_headword_removes_at_most_the_first_entry(
            headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let entries = entries_from(&headwords);
            let original_len = entries.len();
            let result = drop_leading_duplicate_headword(entries);
            proptest::prop_assert!(result.len() == original_len || result.len() + 1 == original_len);
            if result.len() + 1 == original_len {
                proptest::prop_assert_eq!(headwords_of(&result), &headwords[1..]);
            } else {
                proptest::prop_assert_eq!(headwords_of(&result), headwords);
            }
        }

        #[test]
        fn crop_column_widths_sum_to_the_original_with_height_unchanged(
            width in 2u32..500,
            height in 1u32..500,
        ) {
            let img = DynamicImage::new_rgb8(width, height);
            let left = crop_column(&img, true);
            let right = crop_column(&img, false);
            proptest::prop_assert_eq!(left.width() + right.width(), width);
            proptest::prop_assert_eq!(left.height(), height);
            proptest::prop_assert_eq!(right.height(), height);
            proptest::prop_assert!(left.width() >= 1);
            proptest::prop_assert!(right.width() >= 1);
        }

        #[test]
        fn cap_total_pixels_never_upscales_and_never_collapses_to_zero(
            width in 1u32..500,
            height in 1u32..500,
            max_pixels in 1u64..250_000,
        ) {
            let img = DynamicImage::new_rgb8(width, height);
            let capped = cap_total_pixels(img, max_pixels);
            proptest::prop_assert!(capped.width() >= 1);
            proptest::prop_assert!(capped.height() >= 1);
            let original = u64::from(width) * u64::from(height);
            if original <= max_pixels {
                proptest::prop_assert_eq!((capped.width(), capped.height()), (width, height));
            } else {
                proptest::prop_assert!(capped.width() <= width);
                proptest::prop_assert!(capped.height() <= height);
            }
        }
    }
}

//! Pass 1 — reads one photographed dictionary page into one
//! `{headword, raw_entry}` block per bold headword, verbatim, no
//! restructuring.
use crate::engine::VisionEngine;
use crate::json_extract::extract_json_array;
use crate::schema::segment_schema;
use image::{DynamicImage, imageops::FilterType};
use std::path::{Path, PathBuf};

const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// A single real dictionary page photo (confirmed working reliably all
/// session) is around 4.5-4.8 megapixels; the known-bad two-page
/// composite that crashed was ~10 megapixels. This sits meaningfully
/// below the failure point while still giving each stitched page more
/// detail than it would get if the whole composite were squeezed down to
/// one page's own budget.
const MAX_COMPOSITE_PIXELS: u64 = 6_000_000;

/// Downscales `img` (preserving aspect ratio) so its total pixel count
/// is at most `max_pixels`, or returns it unchanged if already under the
/// budget.
fn cap_total_pixels(img: DynamicImage, max_pixels: u64) -> DynamicImage {
    let (width, height) = (u64::from(img.width()), u64::from(img.height()));
    let total = width * height;
    if total <= max_pixels || total == 0 {
        return img;
    }
    // Image dimensions never come close to f64's 52-bit mantissa limit,
    // so these u64->f64 casts lose no real precision.
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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

/// The shared compositing core behind `stitch_boundary_crop` — scales
/// every image to a shared width, stacks them top to bottom, caps the
/// result's total pixel count, and saves it.
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
                // Scale to a shared width (preserving each photo's own
                // aspect ratio) so the composite isn't jagged — these are
                // independent phone photos, not scans, so their widths
                // rarely match exactly even for the same physical page
                // size.
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

    // A ~10-megapixel composite (two real pages stacked) reproducibly
    // crashed Qwen2.5-VL's vision path in llama.cpp with
    // "failed to find a memory slot" — a known, documented upstream
    // difficulty specific to this model family and large images (the
    // maintainers' own fix caps Qwen2VL/Pixtral inputs for exactly this
    // reason). Downscaling the composite as a whole, not per-source-page,
    // is deliberate: it keeps every page in the stitch proportionally
    // represented rather than privileging whichever one got processed
    // first.
    let composite = cap_total_pixels(composite, MAX_COMPOSITE_PIXELS);

    // PNG (lossless), not re-encoding to JPEG: these photos are already
    // JPEG-compressed once; compressing the composite a second time would
    // compound artifacts right where legibility matters most.
    composite.save(out_path).map_err(|e| {
        format!(
            "failed to save stitched image to {}: {e}",
            out_path.display()
        )
    })?;
    Ok(out_path.to_path_buf())
}

/// How much of each page's height to include in a boundary crop — enough
/// to comfortably contain whatever entry straddles the join (dictionary
/// entries are rarely more than a few lines), while staying small enough
/// to never approach the large-image failure `MAX_COMPOSITE_PIXELS`
/// works around in `stitch_images` — this crop is meant to avoid that
/// problem by construction instead, by never getting large in the first
/// place.
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

/// Alternative to stitching whole pages: crops just the bottom sliver of
/// `top_image` and the top sliver of `bottom_image` and stitches only
/// those together — small enough by construction to never approach the
/// large-image failure a full two-page composite can hit, while still
/// giving the model both sides of the join to read together.
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

/// Where a page's two columns split, as a fraction of its total width —
/// a fixed 50/50 default rather than a detected whitespace gutter: this
/// book's photos all split into two roughly-equal columns, and a
/// deterministic constant is simpler and more predictable than a
/// gutter-detection heuristic that could misfire on an unseen photo's
/// particular skew or margins. Adjust this (or detect it properly) if a
/// future book's layout genuinely isn't close to 50/50.
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

/// Like `stitch_boundary_crop`, but for the boundary a single page's own
/// two columns share, rather than between two separate page photos:
/// crops a sliver of the bottom of the left column and the top of the
/// right column from the *same* image and stitches just those together.
/// This is what actually fixes the column-break mis-segmentation that
/// the `SEGMENT_PROMPT` instruction alone couldn't: whether the first
/// word at the top of a column is a genuine new (bold) headword or a
/// plain-type continuation of the entry that was in progress at the
/// bottom of the previous column is a font-weight judgment call —
/// confirmed, independently of this project's own testing, to be a
/// near-chance-accuracy weakness of this model family at this size, not
/// a prompt-wording gap. Column position, unlike font weight, is a
/// structural fact this pipeline can compute directly, so this crop
/// replaces that unreliable visual judgment with one the model doesn't
/// have to make at all: it only ever sees a small, focused sliver with
/// no "new column" framing to get confused by.
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

/// How many of `new_entries`' leading items to search for the overlap
/// boundary — deliberately not the whole list. A genuine overlap is
/// `new_entries` re-transcribing content physically adjacent to the
/// join, so it only ever shows up right at the front; a handful of
/// slots of headroom covers the original full-page-pair design
/// re-finding a couple of leading entries before reaching new content.
/// A match found deeper than that is coincidence or a hallucinated
/// duplicate, not real overlap — confirmed in a real run: a boundary
/// crop's own last headword happened to also appear at position 12 of
/// the next page's independent 21-entry read, and trusting that one
/// silently discarded the 12 genuine entries before it.
const OVERLAP_SEARCH_WINDOW: usize = 5;

/// Appends `new_entries` onto `accumulated`, skipping whatever prefix of
/// `new_entries` duplicates the tail already in `accumulated` — found by
/// matching headwords (case-insensitive) within `OVERLAP_SEARCH_WINDOW`
/// of the front, not by asking the model to judge anything, since
/// that's already been confirmed unreliable for this kind of call.
/// Falls back to appending everything if no match is found within that
/// window: better to risk a few duplicate cards downstream (which
/// `decompose`'s own dedup logic already catches) than to silently drop
/// real content.
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

/// How many of a page's own entries, starting at the match point, get
/// replaced *at minimum* by the column-boundary crop's own reading of
/// that same span, regardless of whether they individually match
/// anything in the crop's own result — a small unconditional floor,
/// not a ceiling. Confirmed necessary in a real run: a truncated entry
/// immediately followed by a bogus one invented for its own
/// continuation (which the crop correctly merged away and so never
/// relabels on its own) both need removing, even though the bogus one
/// has nothing in the crop's result to match against.
const COLUMN_FIX_MIN_REPLACE_WINDOW: usize = 2;

/// Corrects `page_entries` using `boundary_entries` — the result of
/// independently re-reading a small crop of just the page's own column
/// break (`stitch_in_page_column_boundary_crop`). Unlike
/// `append_after_overlap`, this *replaces* rather than only appends:
/// the page's own single read already covers the whole page in order,
/// so the crop's job isn't to supply new content but to correct
/// whatever the page's own read got wrong right at the column break.
///
/// The replaced span always covers at least `COLUMN_FIX_MIN_REPLACE_WINDOW`
/// entries, but extends further to cover every one of the page's own
/// entries (even non-contiguous ones, skipping over a stray bogus entry
/// in between) that the crop's result also independently found —
/// confirmed necessary in a real run: a crop that legitimately read
/// seven overlapping entries, not just two, left five stale duplicates
/// behind when only the first two got replaced. That search is bounded
/// to a modest multiple of the crop's own result size specifically so
/// it can't run away and match a headword's *later*, unrelated
/// recurrence — dictionaries do sometimes repeat a headword for a
/// distinct sense — which would incorrectly delete real, unrelated
/// entries in between.
fn apply_column_boundary_fix(page_entries: Vec<Entry>, boundary_entries: Vec<Entry>) -> Vec<Entry> {
    let Some(first) = boundary_entries.first() else {
        return page_entries;
    };
    let first_headword = first.headword.to_lowercase();
    let Some(start_idx) = page_entries
        .iter()
        .position(|e| e.headword.to_lowercase() == first_headword)
    else {
        // The crop's own result has no connection at all to anything
        // the page's own read already found — confirmed in a real run
        // to mean the crop (a tiny, information-poor sliver) simply
        // hallucinated unrelated content, not that it found something
        // real the page's own read missed. Leave page_entries alone
        // rather than pollute it with that.
        return page_entries;
    };
    let boundary_headwords: std::collections::HashSet<String> = boundary_entries
        .iter()
        .map(|e| e.headword.to_lowercase())
        .collect();
    let search_limit = (start_idx + boundary_entries.len() + COLUMN_FIX_MIN_REPLACE_WINDOW)
        .min(page_entries.len());
    let matched_end = page_entries[start_idx..search_limit]
        .iter()
        .rposition(|e| boundary_headwords.contains(&e.headword.to_lowercase()))
        .map_or(start_idx, |rel_pos| start_idx + rel_pos + 1);
    let end_idx = (start_idx + COLUMN_FIX_MIN_REPLACE_WINDOW)
        .max(matched_end)
        .min(page_entries.len());
    let mut result = page_entries;
    result.splice(start_idx..end_idx, boundary_entries);
    result
}

/// Corrects `accumulated`'s own last entry using a page-to-page
/// boundary crop — similar in spirit to `apply_column_boundary_fix`,
/// but for the join between two separate page photos rather than one
/// page's own two columns. Unlike the in-page case, this crop doesn't
/// start right at the transition (it also covers whatever's visible
/// higher up the previous page's own bottom sliver), so the match is
/// found by searching the *whole* crop for the previous page's own
/// last headword, not by trusting the crop's own first entry. Nothing
/// past that match point is used: confirmed in a real run that while
/// the crop's own independent read of the previous page's truncated
/// last entry ("abrupt") was the fuller, correct version, the crop's
/// own read of whatever came right after it ("abscess") was wrong —
/// and blindly trusting it caused the *next* page's own correct read
/// of that same headword to be discarded as redundant overlap. The
/// next page's own full read is more reliable for everything past
/// this one patched entry, so it's left to the ordinary
/// `append_after_overlap` merge that already follows this call.
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

/// Reads one page, then corrects its own column break using a small
/// crop of just that break — see `apply_column_boundary_fix`'s doc
/// comment for why this corrects rather than appends. Used for every
/// page, whether it's the only image given or one of several, since
/// the bug this guards against lives within a single page's own two
/// columns, independent of how many pages there are.
/// If a page's own read starts with two entries sharing the exact same
/// headword, the first is overwhelmingly likely to be an orphaned
/// continuation of whatever entry was still in progress right where
/// this photo starts, mislabeled under the next real headword reached
/// rather than recognized as a continuation — the same underlying
/// failure as the in-page column break, just at the very top of a
/// photo instead. Confirmed in a real run: a page's own first entry
/// for "abscess" carried content that was actually the missing tail of
/// "abrupt"'s entry from the *previous* page, immediately followed by
/// a second, correctly-labeled "abscess" entry with abscess's own real
/// content — and left uncorrected, a page-to-page boundary merge can
/// end up keeping the wrong copy and discarding the correct one,
/// confirmed in that same run. Applied to every page's own read,
/// including a lone single-page run, since the risk isn't specific to
/// multi-page input: whatever a photo's framing happens to catch right
/// at its own top edge can be an orphaned fragment regardless.
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
    Ok(apply_column_boundary_fix(page_entries, boundary_entries))
}

/// A page's rough position in the dictionary's own alphabetic order,
/// derived from its own already-transcribed entries — not a literal
/// printed page number, which a real phone photo won't always show
/// legibly, and which would be redundant anyway: a dictionary is
/// alphabetically ordered by definition, so this is a complete,
/// sufficient ordering signal entirely on its own.
///
/// Prefers the *second* entry over the first: if a page starts mid-entry
/// (continuation runover from the previous page, with no bold headword
/// of its own), that first transcribed "entry" is exactly the kind of
/// mislabeled fragment already confirmed to appear at page/column
/// boundaries — the second entry is reliably a genuine, fully-visible
/// headword instead, and close enough alphabetically either way.
fn page_sort_key(entries: &[Entry]) -> Option<String> {
    entries
        .get(1)
        .or_else(|| entries.first())
        .map(|e| e.headword.to_lowercase())
}

/// Lets the caller supply photographed pages in any order — phone photos
/// are often named by capture time, not page sequence — by reading each
/// one independently first and sorting by its own rough alphabetic
/// position (`page_sort_key`).
///
/// This does mean every page gets read twice in total (once here, once
/// more as part of a pair in `run_segment_sliding_window`) — a deliberate
/// cost, not an oversight: determining order from content needs the same
/// kind of read the boundary-correctness pass does, and reusing the
/// already-proven segment prompt for it is safer than inventing a new,
/// lighter-weight one — this project has already confirmed more than
/// once that small prompt variations can make this model behave
/// unpredictably.
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
    fn column_boundary_fix_replaces_a_corrupted_pair_with_the_crops_own_reading() {
        // Real failure: the page's own read produced a truncated "absurd"
        // immediately followed by a bogus "kanga" entry invented for its
        // own continuation, while the column-boundary crop correctly
        // read the same span as one complete "absurd" entry.
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
        let fixed = apply_column_boundary_fix(page_entries, boundary_entries);
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
        // Real run: a column-break crop legitimately read seven
        // overlapping entries, not just two, while the page's own read
        // had a bogus entry sandwiched in the middle of that same span.
        // A later, unrelated recurrence of "aboard" (dictionaries do
        // repeat a headword for a distinct sense) must survive
        // untouched, not get swept up by an overeager match search.
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
        let fixed = apply_column_boundary_fix(page_entries, boundary_entries);
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
    fn column_boundary_fix_leaves_page_entries_untouched_when_the_crop_found_nothing_matching() {
        // A real run: a column-break crop with little real content to
        // work with hallucinated headwords unrelated to anything on the
        // page — trusting them polluted an otherwise-correct page with
        // garbage. None of the crop's own entries matching anything the
        // page's own read already has is the cheap, confirmed-necessary
        // signal for that, so the page is left alone instead.
        let page_entries = vec![entry("abode", "...")];
        let boundary_entries = vec![entry("zebra", "...")];
        let fixed = apply_column_boundary_fix(page_entries, boundary_entries);
        assert_eq!(
            fixed.iter().map(|e| &e.headword).collect::<Vec<_>>(),
            ["abode"]
        );
    }

    #[test]
    fn boundary_fix_to_previous_page_tail_replaces_a_truncated_last_entry() {
        // Real run: the previous page's own read of "abrupt" was cut
        // off at the bottom of its photo; the boundary crop's own
        // independent read of the same headword was the complete one.
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
        let fixed = apply_column_boundary_fix(page_entries, Vec::new());
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
        // Pair (P1, P2) found: abode, abolish, abominate.
        let mut accumulated = vec![
            entry("abode", "..."),
            entry("abolish", "..."),
            entry("abominate", "..."),
        ];
        // Pair (P2, P3) redundantly re-finds abominate (the last page-2
        // entry, now with both its neighbors in view) before continuing
        // into page 3's own entries.
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
        // A real run: the boundary crop's last headword ("absurd") also
        // happened to appear, as a hallucinated duplicate, at position 12
        // of the next page's own independent 21-entry read — far past
        // OVERLAP_SEARCH_WINDOW. Trusting that match would have skipped
        // the 12 genuine entries before it.
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
        /// `append_after_overlap` only ever extends `accumulated` — it
        /// should never be possible for it to shrink, reorder, or
        /// modify any entry that was already there, no matter what
        /// `new_entries` contains.
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

        /// With nothing accumulated yet, there's nothing to find an
        /// overlap with — the result must be exactly `new_entries`,
        /// unchanged and in order.
        #[test]
        fn append_after_overlap_with_empty_accumulated_copies_new_entries_verbatim(
            new_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let mut accumulated: Vec<Entry> = Vec::new();
            let new_entries = entries_from(&new_headwords);
            append_after_overlap(&mut accumulated, new_entries);
            proptest::prop_assert_eq!(headwords_of(&accumulated), new_headwords);
        }

        /// An empty crop result has nothing to correct with — the page
        /// must come back completely unchanged.
        #[test]
        fn column_boundary_fix_is_a_no_op_when_boundary_entries_is_empty(
            page_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let page_entries = entries_from(&page_headwords);
            let result = apply_column_boundary_fix(page_entries, Vec::new());
            proptest::prop_assert_eq!(headwords_of(&result), page_headwords);
        }

        /// If the crop's own first headword doesn't appear anywhere in
        /// the page's own read, there's nothing to safely correct —
        /// confirmed necessary in a real run (see this fn's own doc
        /// comment) that trusting such a crop pollutes a correct page
        /// with hallucinated content instead.
        #[test]
        fn column_boundary_fix_is_a_no_op_when_no_match_is_found(
            page_headwords in proptest::collection::vec("[a-z]{1,6}", 0..8),
            boundary_first in "[a-z]{1,6}",
            boundary_rest in proptest::collection::vec("[a-z]{1,6}", 0..4),
        ) {
            proptest::prop_assume!(
                !page_headwords.iter().any(|h| h.eq_ignore_ascii_case(&boundary_first))
            );
            let page_entries = entries_from(&page_headwords);
            let mut boundary_entries = vec![entry(&boundary_first, "y")];
            boundary_entries.extend(entries_from(&boundary_rest));
            let result = apply_column_boundary_fix(page_entries, boundary_entries);
            proptest::prop_assert_eq!(headwords_of(&result), page_headwords);
        }

        /// When the crop's first headword *does* match something in the
        /// page's own read, whatever comes before that match must
        /// survive untouched — the fix only ever corrects the matched
        /// span onward, never anything earlier in the page.
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
            let result = apply_column_boundary_fix(page_entries, boundary_entries);
            proptest::prop_assert!(result.len() >= before_len);
            proptest::prop_assert_eq!(headwords_of(&result[..before_len]), before_headwords);
        }

        /// This fix only ever replaces `accumulated`'s own last entry's
        /// *content* — it never adds or removes entries, so the length
        /// must always come back exactly unchanged.
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

        /// If nothing in the boundary crop matches `accumulated`'s own
        /// last headword, there's nothing safe to patch with — the
        /// last entry (and everything before it) must come back
        /// completely unchanged.
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

        /// Dropping a leading duplicate can only ever remove at most
        /// the very first entry — the result is always either the
        /// whole input or the input with just `entries[0]` gone, never
        /// anything more, less, or reordered.
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

        /// Splitting a page into its left/right column must never lose
        /// or duplicate so much as a pixel of width, and must never
        /// touch the height at all.
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

        /// Downscaling never collapses a real image to nothing, and
        /// never *upscales* past the original in either dimension —
        /// the target scale factor is always < 1 once the image is
        /// over budget, and rounding a value below `n` to the nearest
        /// integer can never land above `n` itself. (A tight bound on
        /// how far the total can land *under* that ceiling isn't
        /// asserted here: it depends on the original aspect ratio in a
        /// way that doesn't reduce to a simple constant, and the
        /// hand-written unit test above already covers one concrete,
        /// worked example of the budget actually being met.)
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

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
/// place. Tried raising this to 0.35 to fully cover one real, unusually
/// long entry ("about, prep.") that 0.2 cuts off partway through — made
/// things worse, not better: a real run on the larger crop sent this
/// model into a repetition/garbling failure mode (a looped, corrupted
/// entry, with both "about" and "above" lost entirely) rather than
/// just reading further. Left at 0.2; a long entry getting cut off at
/// the crop's edge is a known, accepted limitation, not a bug to chase
/// by enlarging the image further.
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

#[allow(clippy::cast_precision_loss)]
fn crop_band_fraction(img: &DynamicImage, start_fraction: f64, end_fraction: f64) -> DynamicImage {
    let height = img.height();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let start_y = ((f64::from(height) * start_fraction).round() as u32).min(height);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let end_y = ((f64::from(height) * end_fraction).round() as u32).clamp(start_y, height);
    img.crop_imm(0, start_y, img.width(), (end_y - start_y).max(1))
}

/// Where each continuation crop (see `extend_truncated_boundary_entry`)
/// looks, as fractions of the right column's own height from its top —
/// a short, fixed list of bands rather than one bigger crop or a
/// computed stride. Each band is the same modest size already proven
/// safe for `stitch_in_page_column_boundary_crop` (enlarging a single
/// crop to reach further down the column, confirmed in a real run, sent
/// this model into a repetition/garbling failure instead of just
/// reading more — see `BOUNDARY_CROP_FRACTION`'s own doc comment), just
/// moved further down the column instead of grown; each one overlaps
/// the end of the previous band slightly, so there's always a bit of
/// shared text for continuity rather than a hard seam. Two bands is
/// already enough to fully cover the one real case this targets
/// ("about, prep.", which only needed to reach roughly a third of the
/// way down the next column).
const CONTINUATION_CROP_BANDS: &[(f64, f64)] = &[(0.15, 0.45), (0.40, 0.70)];

/// Like `stitch_in_page_column_boundary_crop`, but reads further down
/// the same (right) column instead of the column break itself — see
/// `extend_truncated_boundary_entry`.
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

/// If `boundary_entries`' own last entry still looks cut off (per
/// `ends_with_terminal_punctuation`, checked *after* folding away any
/// unanchored trailing fragments the same way `apply_column_boundary_fix`
/// itself will — otherwise a fragment like "apha" that happens to end in
/// real punctuation of its own would hide that the entry it actually
/// belongs to, "about, prep.", is still incomplete), reads one or two
/// further bands down the same column (`CONTINUATION_CROP_BANDS`) and
/// appends each one's own text onto `boundary_entries`' real last
/// element — not a temporary folded copy — so `apply_column_boundary_fix`'s
/// own fold step still sees, and correctly merges, the extended text
/// exactly as it would have if the first crop had simply read further.
///
/// Stops as soon as a continuation crop's first entry is a *different*,
/// genuinely anchored headword (found elsewhere in `page_entries`):
/// that means this band has moved past the entry being extended into
/// whatever comes next for real, not that there's more of it to read.
/// An unanchored pseudo-headword, or a repeat of the very entry being
/// extended, both mean "keep going" — the continuation crop starts
/// mid-sentence just like the first one does, so the same hallucinated-
/// headword risk applies and is handled the same way.
///
/// Confirmed in a real run on "about, prep." to genuinely reach the
/// entry's true end, which a single enlarged crop couldn't do without
/// degrading — but not cleanly: each band deliberately overlaps the
/// previous one for context, and the two independent reads of that
/// shared sliver came back with slightly different punctuation (a
/// colon one time, a semicolon the next), so the text itself ends up
/// with a duplicated clause that a plain substring check can't catch
/// and trim. The two reads can also skip different amounts of a dense
/// passage, leaving a gap between what one band's own read stopped at
/// and where the next one happened to pick back up. Left as-is: a
/// stray duplicate clause is the same category of harmless redundancy
/// `decompose`'s own dedup logic already absorbs downstream, and a
/// dropped middle clause is the same already-accepted content-skipping
/// limitation this model has on any sufficiently dense entry, not
/// something this mechanism makes worse.
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
        // Real case ("about, prep."): the crop's own generation ran out
        // mid-prose, but happened to land right after a semicolon
        // separating two of the entry's sub-senses — a locally
        // legitimate-looking clause boundary indistinguishable from a
        // real full stop by punctuation alone. Only on the very first
        // attempt, also treat the crop's own *raw*, pre-fold last entry
        // being unanchored as a sign of exactly that: a real crop ends
        // on a genuine next headword, not an orphaned fragment folded
        // away by `fold_unanchored_boundary_entries`. Limited to the
        // first attempt so a later, genuinely complete continuation
        // crop can still end the loop on its own punctuation alone,
        // rather than this older signal forcing another fetch anyway.
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

/// A continuation crop's first entry means "keep going, this is still
/// the entry being extended" unless it's a *different*, genuinely
/// anchored headword — see `extend_truncated_boundary_entry`'s own doc
/// comment for why an unanchored pseudo-headword, or a repeat of the
/// target entry's own headword, don't count as moving past it.
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

/// A real, complete column-boundary crop ends on a genuine next
/// headword the page's own read also found — one `fold_unanchored_boundary_entries`
/// leaves standing on its own, rather than folding away. If the crop's
/// own *raw*, pre-fold last entry is unanchored instead, its generation
/// was still describing an ongoing, unfinished entry when the crop
/// ran out, whatever punctuation character it happened to stop on.
fn raw_last_entry_is_unanchored(page_entries: &[Entry], boundary_entries: &[Entry]) -> bool {
    boundary_entries.last().is_some_and(|last| {
        !page_entries
            .iter()
            .any(|e| e.headword.to_lowercase() == last.headword.to_lowercase())
    })
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
/// A hallucinated pseudo-headword (see `apply_column_boundary_fix` below)
/// isn't only possible as the crop's very first entry — a real run
/// produced one mid-sequence too ("apha", lifted from the Xhosa gloss
/// text of the entry right before it, not an actual headword). Fold any
/// boundary entry that doesn't correspond to a real headword found
/// elsewhere on the page back into whichever entry precedes it, rather
/// than let it stand as its own wrongly-headworded entry.
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
    // The crop sometimes starts mid-sentence with no real bold headword
    // to anchor it, and the model invents a pseudo-headword from
    // whatever word it saw first — confirmed in a real run ("ezininzi",
    // lifted mid-word from an unrelated continuation). Skip any of the
    // crop's own leading entries that don't match anything the page's
    // own read already found, rather than giving up entirely just
    // because the very first one doesn't match.
    let Some(first_matched_idx) = boundary_entries.iter().position(|b| {
        let bh = b.headword.to_lowercase();
        page_entries.iter().any(|e| e.headword.to_lowercase() == bh)
    }) else {
        // None of the crop's entries connect to anything the page's own
        // read already found — confirmed in a real run to mean the crop
        // (a tiny, information-poor sliver) simply hallucinated unrelated
        // content, not that it found something real the page's own read
        // missed. Leave page_entries alone rather than pollute it with
        // that.
        return page_entries;
    };
    let trimmed_boundary_entries = boundary_entries[first_matched_idx..].to_vec();
    // Take the count *before* folding away unanchored fragments below —
    // a real run showed this collapsing two raw crop entries ("about"
    // plus a trailing hallucinated "apha") down to one, which must not
    // be confused with the single-entry-crop case the floor just below
    // exists for (where the crop only ever had the one entry to begin
    // with).
    let raw_boundary_entry_count = trimmed_boundary_entries.len();
    let boundary_entries =
        fold_unanchored_boundary_entries(&page_entries, trimmed_boundary_entries);
    let first_headword = boundary_entries[0].headword.to_lowercase();
    // A dictionary does sometimes repeat a headword for a distinct,
    // unrelated sense elsewhere on the page — confirmed in a real run
    // ("about" appearing three times) that the *last* occurrence, not
    // the first, is the one actually adjacent to the column break this
    // crop is reading: the crop is always anchored at the tail end of
    // the page's own entries, so an earlier, unrelated recurrence of
    // the same headword is never the right anchor.
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
    // The minimum-window floor exists for the single-entry crop case:
    // confirmed in a real run that the crop can collapse a page's own
    // truncated entry plus the bogus continuation entry it spawned
    // (unrelated headword, not found by `matched_end`'s own search)
    // into one correct entry, so a floor past `matched_end` is needed
    // to sweep that bogus entry up too. With more than one boundary
    // entry, `matched_end` already has multiple real headwords to
    // search for and is a reliable, tighter bound on its own —
    // confirmed in another real run that applying the floor
    // unconditionally here swallows the next, perfectly legitimate
    // entry when it happens to immediately follow the match. This must
    // key off the *raw*, pre-fold count — confirmed in yet another real
    // run ("about" plus a trailing hallucinated "apha" folding down to
    // one entry) that keying it off the post-fold count wrongly treats
    // that case as the single-entry-crop one and swallows the next
    // legitimate entry anyway.
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

/// A real run surfaced an entry whose `raw_entry` began with an open
/// paren — a dangling sense continuation (e.g. "(encourage vice,
/// crime) ...") that a column-boundary crop had split off into its
/// own, wrongly-headworded entry ("aboard") instead of recognising it
/// as the rest of whatever entry ("abet") was still open when the
/// column broke. Confirmed across a real 68-entry corpus: every other
/// entry's `raw_entry` begins with either its own headword or a
/// part-of-speech abbreviation, never a bare '(' — so this is a safe,
/// unambiguous signal to fold the fragment back into the entry
/// immediately before it, rather than leave it standalone under a
/// headword it doesn't belong to.
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

/// A real run surfaced an entry ("absurd") whose `raw_entry` was cut off
/// mid-word, with no terminal punctuation at all — the page's own read
/// then invented a new headword from whatever word came next for what
/// is really just the rest of that same truncated entry (observed as
/// "kanga" in one run and, confirmed in a *different* run of the same
/// photo, "accelerate" — a real headword that also exists correctly
/// elsewhere on the same page, so a real-headword check alone can't
/// catch this). Checked against the real 67/68-entry corpus across four
/// separate runs: every single other entry's `raw_entry` ends in '.',
/// ';', ',' or ')' — so the absence of one of those is a safe signal
/// that an entry was cut short and the entry right after it is really
/// its own continuation, whatever headword that entry claims.
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

/// Any entry still failing `ends_with_terminal_punctuation` after
/// `merge_entries_with_no_terminal_punctuation` has already folded in
/// everything foldable genuinely has nothing left to merge with — a
/// real case of this ("about, prep.", "abrupt") is a page- or
/// crop-edge cutting the entry off mid-sentence with no more of the
/// dictionary available to read, not a mislabeling bug a merge could
/// fix. Surfacing these for a human to glance at, instead of letting
/// them ship silently indistinguishable from a complete entry, is the
/// whole point — this never tries to fix them itself.
fn truncated_entry_indices(entries: &[Entry]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !ends_with_terminal_punctuation(&e.raw_entry))
        .map(|(i, _)| i)
        .collect()
}

/// Strips the Latin accent marks a dictionary headword occasionally
/// carries on a loanword (e.g. "abbé") down to their plain base letter.
/// Deliberately a small hand-written table rather than pulling in a
/// general Unicode-normalization dependency: this book's headwords are
/// English (plus the odd accented loanword), not a script with
/// independent collation rules of its own, so this narrow table covers
/// the real case without the extra weight. Matters for
/// `headword_order_anomalies`: comparing raw codepoints instead would
/// wrongly flag "abbé" as sorting *after* "abbess" (since 'é' has a
/// higher codepoint than any plain ASCII letter), a real false positive
/// confirmed while investigating this session's "accelerate" case.
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

/// A dictionary's entries are, structurally, one long alphabetically
/// non-decreasing sequence — the same headword repeated for a distinct
/// sense is fine (not a decrease), but a *different* headword sorting
/// before the one right before it isn't. This is exactly the signal
/// that caught this session's real "accelerate" bug by hand: it sorted
/// in between "absurd" and "abundant", where it has no business being.
/// Returns the index of every entry whose headword sorts strictly
/// before the entry immediately before it, for a human to take a
/// second look at — this never auto-fixes anything itself, since (as
/// that same case showed) figuring out *which* of the two entries is
/// actually the wrong one needs real investigation, not just the fact
/// that the order broke.
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

/// The sidecar path a run's review notes (if any) are written to —
/// alongside `entries_out_file`, not inside it: the two are different
/// shapes serving different readers (every entry, for the app, vs.
/// just the suspect ones, for a human), and mixing them would force
/// every downstream reader of the main file to filter out review-only
/// noise.
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
        // Real run: the column-boundary crop split "(encourage vice,
        // crime) -khuthaza/-thele-kisa okungendawo." off into its own
        // entry headworded "aboard", when it's really the rest of
        // "abet"'s second sense, cut off by the column break.
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
        // Real run: the page's own read cut "absurd" off mid-word with
        // no closing punctuation, then invented a new headword
        // ("accelerate" — a real headword that also exists correctly
        // elsewhere on the same page) for the rest of that same entry.
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
        // There's nothing after the last entry to fold into it, even
        // when it has no terminal punctuation of its own — this just
        // means the photos ran out mid-entry, not a mislabeling bug.
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
        // Real run: "accelerate" sorted in between "absurd" and
        // "abundant", where it has no business being — the actual bug
        // was a mislabeled continuation (see
        // `merge_entries_with_no_terminal_punctuation`'s own real-run
        // comment), but the order violation alone is what a human
        // would notice first.
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
        // Real false-positive risk this check must avoid: naive
        // codepoint comparison sorts "abbé" *after* "abbess" (since
        // 'é' has a higher codepoint than any plain ASCII letter),
        // even though a real dictionary correctly sorts it before.
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
        // Real shape this guards against: a continuation crop that's
        // moved far enough down the column to reach the *next* real
        // entry shouldn't have that entry's text appended onto the one
        // being extended.
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
        // Real shape: the continuation crop starts mid-sentence too,
        // with no bold heading of its own, so it can hallucinate a
        // pseudo-headword from body text just like the first crop did
        // — that's still the same ongoing entry, not a new one.
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
        // Real case: the crop's own raw read ended "...about", "apha" —
        // "apha" is an unanchored pseudo-headword, even though the
        // merged text it's folded into happens to end in a semicolon
        // that looks like a valid stopping point on its own.
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
        // Real run: the crop's own view started mid-sentence, with no
        // real bold headword to anchor it, so the model invented
        // "ezininzi" from the first word of an unrelated continuation —
        // immediately followed by a genuinely useful "about" entry, then
        // another hallucinated pseudo-headword ("apha", actually just a
        // Xhosa word — "here" — from partway through that same entry's
        // own gloss text) for the tail continuation that also has no
        // bold headword introducing it.
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
        // "about" legitimately appears more than once on a real page for
        // distinct senses — the crop is always anchored at the tail end
        // of the page's own entries, so it's the *last* occurrence that
        // the column break actually sits next to.
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
        // A real run: a column-break crop with little real content to
        // work with hallucinated headwords unrelated to anything on the
        // page — trusting them polluted an otherwise-correct page with
        // garbage. None of the crop's own entries matching anything the
        // page's own read already has is the cheap, confirmed-necessary
        // signal for that, so the page is left alone instead.
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
            let result = apply_column_boundary_fix(page_entries, &[]);
            proptest::prop_assert_eq!(headwords_of(&result), page_headwords);
        }

        /// If none of the crop's own entries appear anywhere in the
        /// page's own read, there's nothing to safely correct —
        /// confirmed necessary in a real run (see this fn's own doc
        /// comment) that trusting such a crop pollutes a correct page
        /// with hallucinated content instead.
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
            let result = apply_column_boundary_fix(page_entries, &boundary_entries);
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

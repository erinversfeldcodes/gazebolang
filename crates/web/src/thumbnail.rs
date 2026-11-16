#![cfg(feature = "ssr")]
//! Small previews of uploaded card images. The card grid and the review
//! screen show these (a few tens of KB) instead of the full-size original
//! (often 200-800 KB); the full image loads only on a card's own page.

// The Err(String) is already the full explanation, as in storage.rs.
#![allow(clippy::missing_errors_doc)]

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageFormat, ImageReader};
use std::io::Cursor;

/// Longest edge of a thumbnail, in pixels. Enough to look sharp when a
/// card is shown at ~300-400 CSS pixels wide on a high-density screen.
pub const THUMB_MAX_DIM: u32 = 640;

/// JPEG quality of generated thumbnails.
const THUMB_QUALITY: u8 = 80;

/// An image already this small (in both pixels and bytes) is used as-is:
/// re-encoding it would only cost quality (and, for a small PNG, likely
/// grow it).
const SKIP_BELOW_BYTES: usize = 60_000;

/// Refuse to decode anything claiming more pixels than this (a
/// decompression bomb would otherwise exhaust the small machine's memory).
const MAX_PIXELS: u64 = 20_000_000;

/// A JPEG thumbnail of `bytes` no larger than [`THUMB_MAX_DIM`] on its
/// longest edge, or `Ok(None)` when the original should just be used: it's
/// already small, it isn't a format we resize (e.g. GIF, which may be
/// animated), or the re-encoded result wouldn't be smaller. `Err` means the
/// bytes aren't a decodable image at all.
///
/// Transparency is flattened onto white — JPEG has no alpha, and card
/// images are shown on a light background.
pub fn make_thumbnail(bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let reader = || ImageReader::new(Cursor::new(bytes)).with_guessed_format();
    let format = reader()
        .map_err(|e| e.to_string())?
        .format()
        .ok_or_else(|| "unrecognised image format".to_string())?;
    if !matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP
    ) {
        return Ok(None);
    }
    let (width, height) = reader()
        .map_err(|e| e.to_string())?
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("image too large to process ({width}x{height})"));
    }
    if width.max(height) <= THUMB_MAX_DIM && bytes.len() <= SKIP_BELOW_BYTES {
        return Ok(None);
    }

    let decoded = reader()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    let resized = if width.max(height) > THUMB_MAX_DIM {
        decoded.resize(THUMB_MAX_DIM, THUMB_MAX_DIM, FilterType::Triangle)
    } else {
        decoded
    };

    let rgba = resized.to_rgba8();
    let (out_width, out_height) = rgba.dimensions();
    let mut rgb = Vec::with_capacity(out_width as usize * out_height as usize * 3);
    for pixel in rgba.pixels() {
        let [red, green, blue, alpha] = pixel.0;
        let alpha = u16::from(alpha);
        // Composite over white: c*a + 255*(1-a).
        for channel in [red, green, blue] {
            let blended = (u16::from(channel) * alpha + 255 * (255 - alpha)) / 255;
            rgb.push(u8::try_from(blended).unwrap_or(255));
        }
    }

    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, THUMB_QUALITY)
        .encode(&rgb, out_width, out_height, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok((out.len() < bytes.len()).then_some(out))
}

/// What one backfill batch did.
#[derive(Debug, serde::Serialize)]
pub struct BackfillSummary {
    /// Images that now have a thumbnail row (a real thumbnail, or a marker
    /// that the original is fine as it is).
    pub processed: usize,
    /// Of those, how many got a new, smaller thumbnail.
    pub thumbnails_made: usize,
    /// Downloads/uploads that failed and will be retried by a later batch.
    pub failed: usize,
    /// Images still without a thumbnail row after this batch.
    pub remaining: i64,
}

/// Largest original the backfill will download.
const MAX_DOWNLOAD_BYTES: usize = 15 * 1024 * 1024;

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("status {}", response.status()));
    }
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(format!("{} bytes is over the limit", bytes.len()));
    }
    Ok(bytes.to_vec())
}

/// Makes thumbnails for up to `limit` already-stored card images that don't
/// have one. Safe to call repeatedly: finished images get a row (even when
/// the original is used as-is) so they're never reprocessed; transient
/// failures leave no row and are retried next time.
pub async fn backfill_thumbnails(
    storage: &crate::storage::Storage,
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<BackfillSummary, String> {
    let (urls, _) = crate::db::images_without_thumbnails(pool, storage.public_url_prefix(), limit)
        .await
        .map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let (mut processed, mut made, mut failed) = (0, 0, 0);
    for url in &urls {
        let bytes = match download(&client, url).await {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!("thumbnail backfill: downloading {url} failed: {e}");
                failed += 1;
                continue;
            }
        };
        let thumb_url = storage.make_and_upload_thumbnail(&bytes, url).await;
        if let Err(e) = crate::db::record_thumbnail(pool, url, &thumb_url).await {
            log::warn!("thumbnail backfill: recording {url} failed: {e}");
            failed += 1;
            continue;
        }
        processed += 1;
        if thumb_url != *url {
            made += 1;
        }
    }
    let (_, remaining) = crate::db::images_without_thumbnails(pool, storage.public_url_prefix(), 0)
        .await
        .map_err(|e| e.to_string())?;
    Ok(BackfillSummary {
        processed,
        thumbnails_made: made,
        failed,
        remaining,
    })
}

#[derive(Debug, serde::Deserialize)]
pub struct BackfillQuery {
    limit: Option<i64>,
}

/// `POST /api/admin/backfill-thumbnails?limit=20` — signed-in users only.
/// Processes one small batch (the machine is small; callers loop until
/// `remaining` is 0) and returns what it did.
pub async fn backfill_handler(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
    cookies: tower_cookies::Cookies,
    axum::extract::Query(query): axum::extract::Query<BackfillQuery>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    if crate::auth::require_user(&state.db_pool, &cookies)
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(storage) = &state.storage else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "image storage isn't configured",
        )
            .into_response();
    };
    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    match backfill_thumbnails(storage, &state.db_pool, limit).await {
        Ok(summary) => axum::Json(summary).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)] // truncating noise bytes is the point
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb, Rgba};

    /// Deterministic noise, so the encoded image is genuinely large.
    fn noisy_png(width: u32, height: u32) -> Vec<u8> {
        let mut seed: u32 = 12345;
        let img = ImageBuffer::from_fn(width, height, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            Rgb([(seed >> 8) as u8, (seed >> 16) as u8, (seed >> 24) as u8])
        });
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    fn dimensions(bytes: &[u8]) -> (u32, u32) {
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap()
    }

    #[test]
    fn a_large_image_becomes_a_much_smaller_jpeg_within_the_size_cap() {
        let original = noisy_png(1600, 1000);
        let thumb = make_thumbnail(&original).unwrap().expect("a thumbnail");
        assert_eq!(
            ImageReader::new(Cursor::new(&thumb))
                .with_guessed_format()
                .unwrap()
                .format(),
            Some(ImageFormat::Jpeg)
        );
        let (w, h) = dimensions(&thumb);
        assert_eq!(w, THUMB_MAX_DIM, "the longest edge is capped");
        assert_eq!(h, 400, "aspect ratio is kept (1600x1000 -> 640x400)");
        assert!(
            thumb.len() * 5 < original.len(),
            "{} vs {}",
            thumb.len(),
            original.len()
        );
    }

    #[test]
    fn a_small_image_is_used_as_is() {
        assert!(make_thumbnail(&noisy_png(40, 40)).unwrap().is_none());
    }

    #[test]
    fn transparency_is_flattened_onto_white() {
        // Fully transparent, but with noisy colour data underneath so the
        // PNG is big enough to be thumbnailed.
        let mut seed: u32 = 99;
        let img = ImageBuffer::from_fn(1200, 900, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            Rgba([
                (seed >> 8) as u8,
                (seed >> 16) as u8,
                (seed >> 24) as u8,
                0u8,
            ])
        });
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let thumb = make_thumbnail(&png).unwrap().expect("a thumbnail");
        let decoded = image::load_from_memory(&thumb).unwrap().to_rgb8();
        let p = decoded.get_pixel(10, 10);
        assert!(p.0.iter().all(|c| *c >= 250), "expected ~white, got {p:?}");
    }

    #[test]
    fn garbage_and_empty_input_are_errors_not_panics() {
        assert!(make_thumbnail(b"definitely not an image").is_err());
        assert!(make_thumbnail(&[]).is_err());
    }

    #[test]
    fn a_truncated_image_is_an_error() {
        let png = noisy_png(900, 900);
        assert!(make_thumbnail(&png[..png.len() / 2]).is_err());
    }
}

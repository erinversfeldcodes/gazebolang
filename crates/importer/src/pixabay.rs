//! Same Pixabay search the app's own `image_search.rs` calls, but
//! downloading the top hit's full-resolution image here rather than
//! handing the provider's own URL to the app to store. Confirmed live:
//! `large_image_url` is a hotlink/CDN path, not a stable asset link, and
//! every card that stored one directly (instead of re-hosting it) went
//! dark once Pixabay started 503ing those requests. Downloading the bytes
//! here and submitting them as a file upload (see
//! `app_client::create_cards`) makes this importer re-host through the
//! app's own storage exactly the way the in-app image-search picker
//! already does (`cards::import_card_image`) — there's no human in the
//! loop here to pick one, and this tool's whole premise is that a human
//! reviews/corrects the result afterward in the app itself.
use serde::Deserialize;

/// Mirrors `image_search.rs`'s own limit — no reason for a provider image
/// fetched here to be held to a looser standard than one fetched through
/// the in-app picker.
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct PixabayResponse {
    hits: Vec<PixabayHit>,
}

#[derive(Debug, Deserialize)]
struct PixabayHit {
    #[serde(rename = "largeImageURL")]
    large_image_url: String,
}

/// Searches Pixabay for `query` and downloads the top hit's image.
/// `None` on any failure along the way (no API key, no results, a
/// non-image or oversized response, network error) — every case logs its
/// own reason and the caller treats all of them identically as "no image
/// for this card."
pub fn find_image(api_key: Option<&str>, query: &str) -> Option<(Vec<u8>, String)> {
    let api_key = api_key?;
    let url = match reqwest::Url::parse_with_params(
        "https://pixabay.com/api/",
        &[
            ("key", api_key),
            ("q", query),
            ("image_type", "photo"),
            ("per_page", "3"), // Pixabay rejects per_page < 3
        ],
    ) {
        Ok(url) => url,
        Err(e) => {
            println!(
                "  pixabay search for \"{query}\" failed: couldn't build the request URL: {e}"
            );
            return None;
        }
    };
    let res = match reqwest::blocking::get(url) {
        Ok(res) => res,
        Err(e) => {
            println!("  pixabay search for \"{query}\" failed: {e}");
            return None;
        }
    };
    if !res.status().is_success() {
        println!("  pixabay search for \"{query}\" failed: {}", res.status());
        return None;
    }
    let image_url = match res.json::<PixabayResponse>() {
        Ok(body) => body.hits.into_iter().next()?.large_image_url,
        Err(e) => {
            println!("  pixabay search for \"{query}\" failed: couldn't parse the response: {e}");
            return None;
        }
    };
    fetch_image(&image_url, query)
}

fn fetch_image(url: &str, query: &str) -> Option<(Vec<u8>, String)> {
    let res = match reqwest::blocking::get(url) {
        Ok(res) => res,
        Err(e) => {
            println!("  pixabay image fetch for \"{query}\" failed: {e}");
            return None;
        }
    };
    if !res.status().is_success() {
        println!(
            "  pixabay image fetch for \"{query}\" failed: {}",
            res.status()
        );
        return None;
    }
    let content_type = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if !content_type.starts_with("image/") {
        println!(
            "  pixabay image fetch for \"{query}\" returned content-type \"{content_type}\", expected an image"
        );
        return None;
    }
    let bytes = match res.bytes() {
        Ok(bytes) => bytes,
        Err(e) => {
            println!("  pixabay image fetch for \"{query}\" failed reading the response: {e}");
            return None;
        }
    };
    if bytes.len() > MAX_IMAGE_BYTES {
        println!(
            "  pixabay image fetch for \"{query}\" returned {} bytes, over the {MAX_IMAGE_BYTES}-byte limit",
            bytes.len()
        );
        return None;
    }
    Some((bytes.to_vec(), content_type))
}

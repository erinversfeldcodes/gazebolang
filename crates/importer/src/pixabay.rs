//! Same Pixabay search the app's own `image_search.rs` calls, taking the
//! top hit's full-resolution URL directly rather than going through the
//! interactive thumbnail-then-fetch-by-id picker flow — there's no human
//! in the loop here to pick one, and this tool's whole premise is that a
//! human reviews/corrects the result afterward in the app itself.
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PixabayResponse {
    hits: Vec<PixabayHit>,
}

#[derive(Debug, Deserialize)]
struct PixabayHit {
    #[serde(rename = "largeImageURL")]
    large_image_url: String,
}

pub fn find_image_url(api_key: Option<&str>, query: &str) -> Option<String> {
    let api_key = api_key?;
    let url = reqwest::Url::parse_with_params(
        "https://pixabay.com/api/",
        &[
            ("key", api_key),
            ("q", query),
            ("image_type", "photo"),
            ("per_page", "3"), // Pixabay rejects per_page < 3
        ],
    )
    .ok()?;
    let res = reqwest::blocking::get(url).ok()?;
    if !res.status().is_success() {
        println!("  pixabay search for \"{query}\" failed: {}", res.status());
        return None;
    }
    let body: PixabayResponse = res.json().ok()?;
    body.hits.into_iter().next().map(|h| h.large_image_url)
}

#![cfg(feature = "ssr")]
// The Err(String) on each of these already is the full explanation; an
// `# Errors` doc section would just restate it.
#![allow(clippy::missing_errors_doc)]

use serde::{Deserialize, Serialize};

/// One image search result, already normalized across providers — just
/// enough to render a thumbnail and let the user pick it. `provider` +
/// `id` is everything [`ImageSearch::fetch_by_id`] needs to re-fetch a
/// trusted, fresh copy of the full-resolution image server-side; the
/// thumbnail URL itself is only ever rendered directly in the browser,
/// never fetched by this server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageResult {
    pub provider: String,
    pub id: String,
    pub thumbnail_url: String,
    pub description: String,
}

const MAX_RESULTS_PER_PROVIDER: u32 = 12;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
// reqwest's default client has no timeout at all — a slow or unreachable
// provider would otherwise hang a search indefinitely instead of just
// being treated as that one provider failing (see `search`'s own graceful
// degradation). Generous enough to cover a full-resolution image download
// in `fetch_by_id`, not just the smaller search/metadata calls.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn new_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        // reqwest sends no User-Agent at all unless one is set explicitly.
        // Pixabay's own API domain tolerates that, but fetch_by_id's
        // second request — a direct GET against the CDN host
        // `largeImageURL` actually points to, not pixabay.com/api/ —
        // doesn't: without a User-Agent it comes back 200 with a
        // text/plain body instead of the image, which fetch_by_id then
        // (correctly) refuses to treat as one.
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .expect("building the image-search HTTP client should never fail")
}

/// Image search against Pixabay. Needs its own API key
/// (`PIXABAY_API_KEY`); simply returns no results when unset, same
/// "degrade, don't fail" spirit as `Storage`/`EmailSender`.
#[derive(Clone)]
pub struct ImageSearch {
    client: reqwest::Client,
    pixabay_key: Option<String>,
    pixabay_base: String,
}

impl ImageSearch {
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            client: new_http_client(),
            pixabay_key: non_empty_env("PIXABAY_API_KEY"),
            pixabay_base: "https://pixabay.com".to_string(),
        }
    }

    #[cfg(test)]
    fn test_with(pixabay_base: String, pixabay_key: Option<String>) -> Self {
        Self {
            client: new_http_client(),
            pixabay_key,
            pixabay_base,
        }
    }

    /// Queries Pixabay. A separate method (rather than folding this logic
    /// into `search_pixabay`) just to keep the same shape this had before
    /// multiple providers were combined — easy to extend if a second
    /// provider comes back later.
    pub async fn search(&self, query: &str) -> Vec<ImageResult> {
        match self.search_pixabay(query).await {
            Ok(found) => found,
            Err(e) => {
                log::warn!("image search: pixabay request failed: {e}");
                Vec::new()
            }
        }
    }

    async fn search_pixabay(&self, query: &str) -> Result<Vec<ImageResult>, String> {
        #[derive(Deserialize)]
        struct Response {
            hits: Vec<Hit>,
        }
        #[derive(Deserialize)]
        struct Hit {
            id: u64,
            #[serde(rename = "previewURL")]
            preview_url: String,
            tags: String,
        }

        let Some(key) = &self.pixabay_key else {
            return Ok(Vec::new());
        };

        // Pixabay's API rejects per_page below 3.
        let per_page = MAX_RESULTS_PER_PROVIDER.max(3).to_string();
        let response: Response = self
            .client
            .get(format!("{}/api/", self.pixabay_base))
            .query(&[
                ("key", key.clone()),
                ("q", query.to_string()),
                ("image_type", "photo".to_string()),
                ("per_page", per_page),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;

        Ok(response
            .hits
            .into_iter()
            .map(|h| ImageResult {
                provider: "pixabay".to_string(),
                id: h.id.to_string(),
                thumbnail_url: h.preview_url,
                description: h.tags,
            })
            .collect())
    }

    /// Re-fetches a specific result's full-resolution image, bypassing the
    /// thumbnail URL entirely — server-side, from the provider's own "get
    /// by id" endpoint, never from a URL the client supplied. This is the
    /// whole SSRF mitigation: the client only ever echoes back a
    /// `(provider, id)` pair drawn from a search result this server itself
    /// just returned, so the worst a tampered client can do is ask for an
    /// id that doesn't exist — never redirect this server into fetching an
    /// arbitrary host.
    pub async fn fetch_by_id(&self, provider: &str, id: &str) -> Result<(Vec<u8>, String), String> {
        let image_url = match provider {
            "pixabay" => self.pixabay_full_url(id).await?,
            other => {
                log::warn!("image fetch: unknown provider {other:?} (id {id})");
                return Err(format!("unknown image provider: {other}"));
            }
        };

        let response = self
            .client
            .get(&image_url)
            .send()
            .await
            .inspect_err(|e| {
                log::error!("image fetch: downloading from {provider} (id {id}) failed: {e}");
            })
            .map_err(|e| e.to_string())?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        if !content_type.starts_with("image/") {
            log::warn!(
                "image fetch: {provider} (id {id}) returned content-type {content_type}, expected an image"
            );
            return Err(format!(
                "expected an image, got content-type {content_type}"
            ));
        }
        let bytes = response
            .bytes()
            .await
            .inspect_err(|e| {
                log::error!("image fetch: reading body from {provider} (id {id}) failed: {e}");
            })
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_IMAGE_BYTES {
            log::warn!(
                "image fetch: {provider} (id {id}) returned {} bytes, over the {MAX_IMAGE_BYTES}-byte limit",
                bytes.len()
            );
            return Err(format!(
                "image is larger than the {MAX_IMAGE_BYTES}-byte limit"
            ));
        }
        Ok((bytes.to_vec(), content_type))
    }

    async fn pixabay_full_url(&self, id: &str) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Response {
            hits: Vec<Hit>,
        }
        #[derive(Deserialize)]
        struct Hit {
            #[serde(rename = "largeImageURL")]
            large_image_url: String,
        }

        let Some(key) = self.pixabay_key.as_ref() else {
            log::warn!("image fetch: pixabay requested (id {id}) but PIXABAY_API_KEY isn't set");
            return Err("Pixabay isn't configured".to_string());
        };

        let response: Response = self
            .client
            .get(format!("{}/api/", self.pixabay_base))
            .query(&[("key", key.as_str()), ("id", id)])
            .send()
            .await
            .inspect_err(|e| log::error!("image fetch: pixabay metadata request for id {id} failed: {e}"))
            .map_err(|e| e.to_string())?
            .error_for_status()
            .inspect_err(|e| log::error!("image fetch: pixabay metadata request for id {id} returned an error status: {e}"))
            .map_err(|e| e.to_string())?
            .json()
            .await
            .inspect_err(|e| log::error!("image fetch: pixabay metadata response for id {id} didn't parse: {e}"))
            .map_err(|e| e.to_string())?;
        response
            .hits
            .into_iter()
            .next()
            .map(|h| h.large_image_url)
            .ok_or_else(|| {
                log::warn!("image fetch: pixabay has no hit for id {id}");
                "image not found".to_string()
            })
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn image_search_with(pixabay: &MockServer, pixabay_key: Option<&str>) -> ImageSearch {
        ImageSearch::test_with(pixabay.uri(), pixabay_key.map(str::to_string))
    }

    #[tokio::test]
    async fn search_returns_results_from_pixabay() {
        let pixabay = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/"))
            .and(query_param("q", "dog"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "hits": [{"id": 2, "previewURL": "https://pixabay.example/2.jpg", "tags": "dog, pet"}]
            })))
            .mount(&pixabay)
            .await;

        let search = image_search_with(&pixabay, Some("pixabay-key"));
        let results = search.search("dog").await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].provider, "pixabay");
    }

    #[tokio::test]
    async fn search_returns_no_results_with_no_key_configured() {
        // No mock registered — if ImageSearch called Pixabay without a
        // key, wiremock would reject the unexpected request.
        let pixabay = MockServer::start().await;

        let search = image_search_with(&pixabay, None);
        let results = search.search("dog").await;

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_degrades_to_no_results_when_pixabay_fails() {
        let pixabay = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&pixabay)
            .await;

        let search = image_search_with(&pixabay, Some("pixabay-key"));
        let results = search.search("dog").await;

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn fetch_by_id_rejects_a_non_image_response() {
        let pixabay = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/"))
            .and(query_param("id", "42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "hits": [{"largeImageURL": format!("{}/not-an-image", pixabay.uri())}]
            })))
            .mount(&pixabay)
            .await;
        Mock::given(method("GET"))
            .and(path("/not-an-image"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html></html>")
                    .insert_header("content-type", "text/html"),
            )
            .mount(&pixabay)
            .await;

        let search = image_search_with(&pixabay, Some("pixabay-key"));

        let result = search.fetch_by_id("pixabay", "42").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_by_id_rejects_an_oversized_image() {
        let pixabay = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/"))
            .and(query_param("id", "42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "hits": [{"largeImageURL": format!("{}/huge.jpg", pixabay.uri())}]
            })))
            .mount(&pixabay)
            .await;
        let oversized = vec![0u8; MAX_IMAGE_BYTES + 1];
        Mock::given(method("GET"))
            .and(path("/huge.jpg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(oversized)
                    .insert_header("content-type", "image/jpeg"),
            )
            .mount(&pixabay)
            .await;

        let search = image_search_with(&pixabay, Some("pixabay-key"));

        let result = search.fetch_by_id("pixabay", "42").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_by_id_returns_bytes_and_content_type_on_success() {
        let pixabay = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/"))
            .and(query_param("id", "42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "hits": [{"largeImageURL": format!("{}/real.jpg", pixabay.uri())}]
            })))
            .mount(&pixabay)
            .await;
        Mock::given(method("GET"))
            .and(path("/real.jpg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"fake-jpeg-bytes".to_vec())
                    .insert_header("content-type", "image/jpeg"),
            )
            .mount(&pixabay)
            .await;

        let search = image_search_with(&pixabay, Some("pixabay-key"));

        let (bytes, content_type) = search.fetch_by_id("pixabay", "42").await.unwrap();
        assert_eq!(bytes, b"fake-jpeg-bytes");
        assert_eq!(content_type, "image/jpeg");
    }

    // Deliberately NOT gated behind a "skip if unconfigured" check the way
    // storage.rs's upload_round_trips_against_configured_storage is. All
    // the other tests in this file prove the parsing/degradation logic is
    // correct against responses shaped like Pixabay's real ones; this one
    // is the only thing that actually proves the real Pixabay API is still
    // reachable and still shaped the way this code assumes. A provider
    // that's down, rate-limited, or has changed its response shape should
    // fail this test and block a merge, not be silently skipped — so a
    // missing API key fails loudly too, rather than letting the test pass
    // by doing nothing. CI must have PIXABAY_API_KEY configured as a real
    // secret for this to mean anything.
    #[tokio::test]
    async fn real_providers_all_respond_to_a_search() {
        assert!(
            non_empty_env("PIXABAY_API_KEY").is_some(),
            "PIXABAY_API_KEY must be set for this test to actually verify Pixabay connectivity"
        );

        let results = ImageSearch::from_env().search("dog").await;

        assert!(
            results.iter().any(|r| r.provider == "pixabay"),
            "expected at least one real result from pixabay, got none — \
             check API key validity, provider status, and rate limits"
        );
    }
}

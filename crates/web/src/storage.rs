#![cfg(feature = "ssr")]
// The Err(String) here is already the full explanation; an `# Errors` doc
// section would just restate it.
#![allow(clippy::missing_errors_doc)]

use s3::{AddressingStyle, Auth, Client, Credentials};

/// A random hex object key for storing a new upload — shared by every path
/// that writes into the bucket (a direct file upload, an image imported
/// from a search result), so there's one place naming the convention.
#[must_use]
pub fn random_key() -> String {
    use rand::RngExt;
    use std::fmt::Write as _;

    let bytes: [u8; 16] = rand::rng().random();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(out, "{b:02x}").unwrap();
    }
    out
}

/// Uploaded-image storage: an S3-compatible bucket — Tigris in prod/preview
/// (see fly.toml/flake.nix), Garage locally (see docker-compose.yml), so the
/// whole stack stays Docker-emulatable the way the project was set up to be
/// from day one.
#[derive(Clone)]
pub struct Storage {
    client: Client,
    bucket: String,
    public_base_url: String,
}

impl Storage {
    /// `None` when any of the required env vars are unset — card creation
    /// then just skips the upload and keeps whatever `image_url` (e.g. a
    /// pasted link) the form already had.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let bucket = std::env::var("S3_BUCKET").ok()?;
        let endpoint = std::env::var("S3_ENDPOINT").ok()?;
        let access_key = std::env::var("S3_ACCESS_KEY").ok()?;
        let secret_key = std::env::var("S3_SECRET_KEY").ok()?;
        let public_base_url = std::env::var("S3_PUBLIC_URL").unwrap_or_else(|_| endpoint.clone());
        let region = std::env::var("S3_REGION").unwrap_or_else(|_| "auto".to_string());
        // Garage (local dev) needs path-style URLs; Tigris (prod/preview)
        // works with the default auto-detection.
        let path_style = std::env::var("S3_PATH_STYLE").is_ok_and(|v| v == "true");

        // Past this point every S3_* var the user intended to set is
        // actually present, so any further failure is a real
        // misconfiguration (bad credentials, bad endpoint) worth logging —
        // unlike the `?`s above, which just mean storage wasn't configured
        // at all, the normal/expected case in local dev.
        let credentials = Credentials::new(access_key, secret_key)
            .inspect_err(|e| {
                log::error!("storage: S3_* vars are set but building credentials failed: {e}");
            })
            .ok()?;
        let mut builder = Client::builder(&endpoint)
            .inspect_err(|e| {
                log::error!("storage: S3_* vars are set but building the client failed: {e}");
            })
            .ok()?
            .region(region)
            .auth(Auth::Static(credentials));
        if path_style {
            builder = builder.addressing_style(AddressingStyle::Path);
        }
        let client = builder
            .build()
            .inspect_err(|e| {
                log::error!("storage: S3_* vars are set but building the client failed: {e}");
            })
            .ok()?;

        Some(Self {
            client,
            bucket,
            public_base_url,
        })
    }

    pub async fn upload(
        &self,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<String, String> {
        let bucket = &self.bucket;
        self.client
            .objects()
            .put(bucket, key)
            .content_type(content_type)
            .inspect_err(|e| log::error!("storage: upload to {bucket}/{key} failed: {e}"))
            .map_err(|e| e.to_string())?
            .body_bytes(bytes.to_vec())
            .send()
            .await
            .inspect_err(|e| log::error!("storage: upload to {bucket}/{key} failed: {e}"))
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "{}/{key}",
            self.public_base_url.trim_end_matches('/')
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::Storage;

    // Exercises whatever S3-compatible service S3_* points at (local Garage
    // via `just db-up && just storage-init`, or unset — in which case
    // `from_env` returns `None` and this is a no-op, same as in production
    // when storage isn't configured). Not run in CI: no S3_* vars are set
    // there, same tradeoff as the Resend-backed tests in email.rs skipping
    // without a real account.
    #[tokio::test]
    async fn upload_round_trips_against_configured_storage() {
        let Some(storage) = Storage::from_env() else {
            return;
        };
        let url = storage
            .upload("storage-test/smoke.txt", "text/plain", b"hello from garage")
            .await
            .expect("upload should succeed");
        assert!(url.ends_with("/storage-test/smoke.txt"));
    }
}

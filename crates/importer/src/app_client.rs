//! Talks to the deployed app over plain HTTP. `log_in` and
//! `save_word_definition` hit dedicated, hand-written routes
//! (`crates/web/src/main.rs`'s `importer_login`/
//! `importer_save_word_definition`) built specifically for this binary,
//! rather than the browser's own Leptos `#[server]` fns — those
//! auto-generate their wire path at build time, and that path isn't
//! even stable across build platforms for the same source (macOS and
//! Linux builds produced different hashes), making them unusable as a
//! fixed target here. `create_cards` insteadnposts straight to
//! `/cards/bulk`, the same plain route the app's own bulk-add form
//! already submits to.
use crate::decompose::Card;
use reqwest::blocking::{Client, multipart};

pub struct AppClient {
    base_url: String,
    http: Client,
    session_cookie: Option<String>,
}

impl AppClient {
    pub fn new(base_url: String) -> Result<Self, String> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            base_url,
            http,
            session_cookie: None,
        })
    }

    pub fn log_in(&mut self, email: &str, password: &str) -> Result<(), String> {
        let res = self
            .http
            .post(format!("{}/api/importer/login", self.base_url))
            .form(&[("email", email), ("password", password)])
            .send()
            .map_err(|e| e.to_string())?;
        let status = res.status();
        let session_cookie = res
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next())
            .find_map(|v| v.strip_prefix("session="))
            .map(str::to_string);
        if let (true, Some(cookie)) = (status.is_success(), session_cookie) {
            self.session_cookie = Some(cookie);
            Ok(())
        } else {
            let text = res.text().unwrap_or_default();
            Err(format!(
                "login failed ({status}): {}",
                if text.is_empty() {
                    "no session cookie in response"
                } else {
                    &text
                }
            ))
        }
    }

    fn cookie_header(&self) -> Result<String, String> {
        self.session_cookie
            .clone()
            .map(|c| format!("session={c}"))
            .ok_or_else(|| "not logged in".to_string())
    }

    pub fn create_cards(&self, cards: &[Card]) -> Result<(), String> {
        let cookie = self.cookie_header()?;
        let mut form = multipart::Form::new();
        for (i, card) in cards.iter().enumerate() {
            form = form.text(format!("target_text_{i}"), card.target_text.clone());
            if let Some(image_url) = &card.image_url {
                form = form.text(format!("image_url_{i}"), image_url.clone());
            }
        }
        let res = self
            .http
            .post(format!("{}/cards/bulk", self.base_url))
            .header(reqwest::header::COOKIE, cookie)
            .multipart(form)
            .send()
            .map_err(|e| e.to_string())?;
        let location = res
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if location.starts_with("/login") {
            return Err("session cookie is invalid or expired".to_string());
        }
        if location.contains("error=") {
            return Err(format!("the app rejected the batch: {location}"));
        }
        Ok(())
    }

    pub fn save_word_definition(&self, card: &Card) -> Result<(), String> {
        let cookie = self.cookie_header()?;
        let res = self
            .http
            .post(format!(
                "{}/api/importer/save-word-definition",
                self.base_url
            ))
            .header(reqwest::header::COOKIE, cookie)
            .form(&[
                ("target_text", card.target_text.as_str()),
                ("part_of_speech", card.part_of_speech.as_str()),
                ("notes", card.notes.as_str()),
                ("english_translation", card.english.as_str()),
                ("example_target_sentence", card.example_target.as_str()),
                ("example_gloss_sentence", card.example_gloss.as_str()),
                ("linked_words_raw", ""),
            ])
            .send()
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            println!(
                "  dictionary details save failed for \"{}\": {}",
                card.target_text,
                res.status()
            );
        }
        Ok(())
    }
}

//! Stage 3 — no model calls at all, so it never competes with either model
//! for memory. For each surviving card: looks up an image via Pixabay (if
//! an API key is configured), then creates the card and its dictionary
//! details on the real deployed app over HTTP.
use crate::app_client::AppClient;
use crate::decompose::Card;
use std::path::Path;

pub struct CreateConfig<'a> {
    pub base_url: String,
    pub auth_email: &'a str,
    pub auth_password: &'a str,
    pub pixabay_api_key: Option<&'a str>,
}

pub fn run_create(config: &CreateConfig, cards_in_file: &Path) -> Result<(), String> {
    let raw = std::fs::read_to_string(cards_in_file).map_err(|e| e.to_string())?;
    let mut cards: Vec<Card> = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if cards.is_empty() {
        println!("nothing to create — exiting.");
        return Ok(());
    }
    if config.pixabay_api_key.is_none() {
        println!("PIXABAY_API_KEY is not set — cards will be created with no image");
    }

    let mut client = AppClient::new(config.base_url.clone())?;
    client.log_in(config.auth_email, config.auth_password)?;

    println!("looking up images for {} card(s)...", cards.len());
    for card in &mut cards {
        let query = if card.image_query.is_empty() {
            &card.english
        } else {
            &card.image_query
        };
        card.image_url = crate::pixabay::find_image_url(config.pixabay_api_key, query);
        println!(
            "  {} -> {}{}",
            card.english,
            card.target_text,
            if card.image_url.is_some() {
                ""
            } else {
                " (no image found)"
            }
        );
    }

    println!("creating {} card(s)...", cards.len());
    client.create_cards(&cards)?;

    println!("saving dictionary details...");
    for card in &cards {
        client.save_word_definition(card)?;
    }

    println!("done — open {}/ to review the new cards.", config.base_url);
    Ok(())
}

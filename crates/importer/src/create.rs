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
        card.image = crate::pixabay::find_image(config.pixabay_api_key, query);
        println!(
            "  {} -> {}{}",
            card.english,
            card.target_text,
            if card.image.is_some() {
                ""
            } else {
                " (no image found)"
            }
        );
    }

    println!("creating {} image card(s)...", cards.len());
    client.create_cards(&cards, "image_to_target")?;

    // A second, audio-less card per headword alongside its image card —
    // same target_text, no media at all — so the word has somewhere to
    // receive a real audio recording later (via EditCardPage) without
    // that recording step blocking the bulk import, which can't record
    // audio itself.
    println!("creating {} placeholder audio card(s)...", cards.len());
    let placeholders: Vec<Card> = cards
        .iter()
        .map(|card| Card {
            target_text: card.target_text.clone(),
            ..Card::default()
        })
        .collect();
    client.create_cards(&placeholders, "sound_to_written")?;

    println!("saving dictionary details...");
    for card in &cards {
        client.save_word_definition(card)?;
    }

    println!("done — open {}/ to review the new cards.", config.base_url);
    Ok(())
}

/// One-off repair for cards the importer created before it re-hosted
/// images (see `pixabay.rs`'s own doc comment): every one of those still
/// has `image_url` pointing straight at Pixabay's hotlink host, which
/// started 503ing those requests. Re-searches Pixabay per broken card
/// (its dictionary entry's English translation if it has one, else its
/// own headword — same fallback order `run_create` already uses) and
/// resubmits a full `/cards/{id}` edit with the freshly re-hosted image,
/// carrying over `target_text`/`tags`/`card_type`/`two_way` unchanged —
/// `cards_update` has no "leave it alone" behavior for those columns the
/// way it does for the media URLs, so a faithful resubmit needs them.
/// Safe to re-run: a card no longer matches the broken-image query once
/// it's been fixed, so a second pass only retries ones still unresolved
/// (no image found this time either, or the update itself failed).
pub fn run_backfill_images(config: &CreateConfig) -> Result<(), String> {
    let mut client = AppClient::new(config.base_url.clone())?;
    client.log_in(config.auth_email, config.auth_password)?;

    let broken = client.list_cards_with_broken_images()?;
    if broken.is_empty() {
        println!("no cards with a broken (pixabay-hosted) image found — nothing to do.");
        return Ok(());
    }
    println!(
        "found {} card(s) with a broken image — re-hosting...",
        broken.len()
    );

    let mut fixed = 0;
    let mut unresolved = 0;
    for card in &broken {
        let query = card
            .english_translation
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&card.target_text);
        if let Some((bytes, content_type)) =
            crate::pixabay::find_image(config.pixabay_api_key, query)
        {
            match client.update_card_image(&card.id, card, &bytes, &content_type) {
                Ok(()) => {
                    println!("  {} -> re-hosted", card.target_text);
                    fixed += 1;
                }
                Err(e) => {
                    println!("  {} -> update failed: {e}", card.target_text);
                    unresolved += 1;
                }
            }
        } else {
            println!("  {} -> still no image found, left as-is", card.target_text);
            unresolved += 1;
        }
    }
    println!("done — {fixed} fixed, {unresolved} left unresolved.");
    Ok(())
}

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deck {
    pub id: Uuid,
    pub name: String,
    pub sample_word: Option<String>,
}

#[cfg(feature = "ssr")]
async fn require_user() -> Result<crate::auth::UserRow, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let cookies = leptos_axum::extract::<tower_cookies::Cookies>()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    crate::auth::require_user(&state.db_pool, &cookies)
        .await
        .map_err(ServerFnError::new)
}

#[server]
pub async fn list_decks() -> Result<Vec<Deck>, ServerFnError> {
    // Browsing decks is public; only mutating them needs auth.
    let state = expect_context::<crate::state::AppState>();
    let rows = crate::db::list_decks(&state.db_pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|r| Deck {
            id: r.id,
            name: r.name,
            sample_word: r.sample_word,
        })
        .collect())
}

/// `group_by_tag`, when set, adds every card already carrying that tag to
/// the new deck immediately — the alternative to building a deck up one
/// card at a time via each card's own "assign to deck" controls. Grouping
/// by anything other than a tag (card type, deck membership, date range,
/// ...) isn't offered: tag is the one attribute `list_cards`/the card
/// list's own filter already treat as a first-class grouping, so it's the
/// one with an existing, known set of values to list here rather than a
/// new ad-hoc query.
#[server]
pub async fn create_deck(name: String, group_by_tag: Option<String>) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let deck_id = crate::db::create_deck(&state.db_pool, user.id, &name)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if let Some(tag) = group_by_tag.filter(|t| !t.trim().is_empty()) {
        crate::db::populate_deck_by_tag(&state.db_pool, deck_id, tag.trim())
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    Ok(())
}

#[server]
pub async fn add_card_to_deck(deck_id: Uuid, card_id: Uuid) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::add_card_to_deck(&state.db_pool, deck_id, card_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

#[server]
pub async fn remove_card_from_deck(deck_id: Uuid, card_id: Uuid) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::remove_card_from_deck(&state.db_pool, deck_id, card_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

#[server]
pub async fn rename_deck(deck_id: Uuid, name: String) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::rename_deck(&state.db_pool, deck_id, &name)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

#[server]
pub async fn delete_deck(deck_id: Uuid) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::delete_deck(&state.db_pool, deck_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

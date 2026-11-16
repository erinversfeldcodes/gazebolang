use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordExample {
    pub target_sentence: String,
    pub gloss_sentence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedWord {
    pub target_text: String,
    pub link_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordDefinition {
    pub target_text: String,
    pub part_of_speech: Option<String>,
    pub notes: Option<String>,
    pub english_translation: Option<String>,
    pub examples: Vec<WordExample>,
    pub linked_words: Vec<LinkedWord>,
}

#[cfg(feature = "ssr")]
impl From<crate::db::WordDefinitionRow> for WordDefinition {
    fn from(r: crate::db::WordDefinitionRow) -> Self {
        Self {
            target_text: r.target_text,
            part_of_speech: r.part_of_speech,
            notes: r.notes,
            english_translation: r.english_translation,
            examples: r
                .examples
                .into_iter()
                .map(|e| WordExample {
                    target_sentence: e.target_sentence,
                    gloss_sentence: e.gloss_sentence,
                })
                .collect(),
            linked_words: r
                .linked_words
                .into_iter()
                .map(|l| LinkedWord {
                    target_text: l.target_text,
                    link_type: l.link_type,
                })
                .collect(),
        }
    }
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

#[cfg(feature = "ssr")]
fn non_empty_trimmed(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Same comma-separated free-text idiom `main.rs::parse_tags` already uses
/// for card tags — can't reuse that fn directly, since it's private to the
/// `web` binary and this is part of the `web` library.
#[cfg(feature = "ssr")]
fn parse_comma_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Looks up a word's dictionary entry by its exact (trimmed) spelling.
/// Public, same as `list_cards`/`get_card` — browsing is public, only
/// mutating needs auth. `Ok(None)` means "no dictionary entry yet", which
/// callers (the popup) render as a graceful empty state, not an error.
#[server]
pub async fn get_word_definition(
    target_text: String,
) -> Result<Option<WordDefinition>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let target_text = target_text.trim();
    if target_text.is_empty() {
        return Ok(None);
    }
    Ok(crate::db::get_word_definition(&state.db_pool, target_text)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .map(Into::into))
}

/// Creates or updates `target_text`'s dictionary entry — the single write
/// path shared by every caller (currently: the card form's "Dictionary
/// details" section on `NewCardPage`/`EditCardPage`). Upserts by exact
/// text: saving twice for the same spelling edits the same row rather than
/// creating a duplicate. Any `linked_words_raw` entry without its own entry
/// yet gets a bare stub `words` row created automatically — see
/// `db::resolve_or_create_word_id_in_tx`.
#[server]
pub async fn save_word_definition(
    target_text: String,
    part_of_speech: String,
    notes: String,
    english_translation: String,
    example_target_sentence: String,
    example_gloss_sentence: String,
    linked_words_raw: String,
) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let target_text = target_text.trim();
    if target_text.is_empty() {
        return Err(ServerFnError::new("target_text can't be empty"));
    }
    let examples = non_empty_trimmed(&example_target_sentence)
        .map_or_default(|t| vec![(t, non_empty_trimmed(&example_gloss_sentence))]);
    let linked = parse_comma_list(&linked_words_raw);

    crate::db::upsert_word_definition(
        &state.db_pool,
        target_text,
        user.id,
        crate::db::WordFields {
            part_of_speech: non_empty_trimmed(&part_of_speech).as_deref(),
            notes: non_empty_trimmed(&notes).as_deref(),
            english_translation: non_empty_trimmed(&english_translation).as_deref(),
        },
        &examples,
        &linked,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;

    #[test]
    fn parse_comma_list_trims_and_drops_empties() {
        assert_eq!(
            parse_comma_list(" uno , , dos,tres "),
            vec!["uno", "dos", "tres"]
        );
    }

    #[test]
    fn non_empty_trimmed_blank_is_none() {
        assert_eq!(non_empty_trimmed("   "), None);
        assert_eq!(non_empty_trimmed(" noun "), Some("noun".to_string()));
    }
}

/// One row of the dictionary index — a word that has at least one card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryEntrySummary {
    pub id: Uuid,
    pub target_text: String,
    pub part_of_speech: Option<String>,
    pub english_translation: Option<String>,
    pub card_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryPageData {
    pub entries: Vec<DictionaryEntrySummary>,
    /// Matching words across all pages, for the pager.
    pub total: i64,
}

/// Every word that has at least one card, sorted by English
/// (`english_first`) or by the target-language spelling, optionally
/// filtered by text in either. Public, like `list_cards` — browsing needs
/// no account.
#[server]
pub async fn list_dictionary(
    english_first: bool,
    search: String,
    page: i64,
    page_size: i64,
) -> Result<DictionaryPageData, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let page_size = crate::cards::clamp_page_size(page_size);
    let order = if english_first {
        crate::db::DictionaryOrder::EnglishFirst
    } else {
        crate::db::DictionaryOrder::TargetFirst
    };
    let (rows, total) = crate::db::list_dictionary_entries(
        &state.db_pool,
        order,
        Some(&search),
        page_size,
        page.max(0) * page_size,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(DictionaryPageData {
        entries: rows
            .into_iter()
            .map(|r| DictionaryEntrySummary {
                id: r.id,
                target_text: r.target_text,
                part_of_speech: r.part_of_speech,
                english_translation: r.english_translation,
                card_count: r.card_count,
            })
            .collect(),
        total,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryCard {
    pub id: Uuid,
    pub card_type: String,
    pub image_url: Option<String>,
    pub two_way: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryLink {
    pub id: Uuid,
    pub target_text: String,
    pub link_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub id: Uuid,
    pub target_text: String,
    pub part_of_speech: Option<String>,
    pub notes: Option<String>,
    pub english_translation: Option<String>,
    pub examples: Vec<WordExample>,
    pub linked_words: Vec<DictionaryLink>,
    /// Every card whose `target_text` is this word.
    pub cards: Vec<DictionaryCard>,
}

/// A word's full entry, with links to all of its cards. `Ok(None)` for an
/// unknown id.
#[server]
pub async fn get_dictionary_entry(word_id: Uuid) -> Result<Option<DictionaryEntry>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let Some(row) = crate::db::get_dictionary_entry(&state.db_pool, word_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
    else {
        return Ok(None);
    };
    Ok(Some(DictionaryEntry {
        id: row.word.id,
        target_text: row.word.target_text,
        part_of_speech: row.word.part_of_speech,
        notes: row.word.notes,
        english_translation: row.word.english_translation,
        examples: row
            .examples
            .into_iter()
            .map(|e| WordExample {
                target_sentence: e.target_sentence,
                gloss_sentence: e.gloss_sentence,
            })
            .collect(),
        linked_words: row
            .linked_words
            .into_iter()
            .map(|l| DictionaryLink {
                id: l.id,
                target_text: l.target_text,
                link_type: l.link_type,
            })
            .collect(),
        cards: row
            .cards
            .into_iter()
            .map(|c| DictionaryCard {
                id: c.id,
                card_type: c.card_type,
                image_url: c.image_url,
                two_way: c.two_way,
            })
            .collect(),
    }))
}

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

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

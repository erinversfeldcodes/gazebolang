//! Vocab/spelling test trainer: shows a card's media (whichever of
//! image/audio/video/YouTube it has) and asks the user to type the
//! correct spelling of its `target_text`. Deliberately written directly
//! against `state.db_pool` with raw `sqlx::query*` calls rather than
//! routing through `crate::db` — this feature was built in parallel with
//! two others (a minimal-pair trainer and a back-chaining game) on
//! separate worktrees, and going through the shared `db.rs` would have
//! made all three agents' work collide on one file.

// #[component] already enforces call sites use the returned view; clippy's
// pedantic must_use_candidate doesn't see through the macro expansion —
// same rationale/precedent as app.rs's own identical allow.
#![allow(clippy::must_use_candidate)]

use leptos::prelude::*;
use leptos_router::components::{A, Redirect};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpellingPrompt {
    pub card_id: Uuid,
    pub image_url: Option<String>,
    pub audio_url: Option<String>,
    pub video_url: Option<String>,
    pub youtube_url: Option<String>,
    // target_text is deliberately NEVER included here — that's the answer.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpellingResult {
    pub correct: bool,
    pub correct_answer: String,
}

#[cfg(feature = "ssr")]
#[derive(Debug, Clone, sqlx::FromRow)]
struct SpellingPromptRow {
    id: Uuid,
    image_url: Option<String>,
    audio_url: Option<String>,
    video_url: Option<String>,
    youtube_url: Option<String>,
}

#[cfg(feature = "ssr")]
impl From<SpellingPromptRow> for SpellingPrompt {
    fn from(r: SpellingPromptRow) -> Self {
        SpellingPrompt {
            card_id: r.id,
            image_url: r.image_url,
            audio_url: r.audio_url,
            video_url: r.video_url,
            youtube_url: r.youtube_url,
        }
    }
}

/// Picks a random card that has at least one non-text prompt to spell
/// from. A separate, directly-testable function (rather than inlined in
/// `spelling_test_round` below) so the unit tests exercise the exact same
/// query the server function runs, instead of a duplicated copy of it.
#[cfg(feature = "ssr")]
async fn fetch_round(pool: &sqlx::PgPool) -> sqlx::Result<Option<SpellingPromptRow>> {
    sqlx::query_as(
        "SELECT id, image_url, audio_url, video_url, youtube_url FROM cards \
         WHERE image_url IS NOT NULL OR audio_url IS NOT NULL \
            OR video_url IS NOT NULL OR youtube_url IS NOT NULL \
         ORDER BY random() LIMIT 1",
    )
    .fetch_optional(pool)
    .await
}

#[cfg(feature = "ssr")]
async fn fetch_target_text(pool: &sqlx::PgPool, card_id: Uuid) -> sqlx::Result<String> {
    sqlx::query_scalar("SELECT target_text FROM cards WHERE id = $1")
        .bind(card_id)
        .fetch_one(pool)
        .await
}

/// Case-insensitive (via `to_lowercase` rather than `eq_ignore_ascii_case`,
/// so accented/non-ASCII target languages aren't penalized either) and
/// trimmed — orthographic case/stray whitespace shouldn't fail a spelling
/// test whose target language isn't even decided yet for this app.
#[cfg(feature = "ssr")]
fn spellings_match(answer: &str, correct_answer: &str) -> bool {
    answer.trim().to_lowercase() == correct_answer.trim().to_lowercase()
}

#[cfg(feature = "ssr")]
async fn record_attempt(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    card_id: Uuid,
    correct: bool,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO spelling_test_attempts (user_id, card_id, correct) VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(card_id)
    .bind(correct)
    .execute(pool)
    .await?;
    Ok(())
}

#[server]
pub async fn spelling_test_round() -> Result<Option<SpellingPrompt>, ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let row = fetch_round(&state.db_pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(row.map(Into::into))
}

#[server]
pub async fn submit_spelling_attempt(
    card_id: Uuid,
    answer: String,
) -> Result<SpellingResult, ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();

    let correct_answer = fetch_target_text(&state.db_pool, card_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let correct = spellings_match(&answer, &correct_answer);

    record_attempt(&state.db_pool, user.id, card_id, correct)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    Ok(SpellingResult {
        correct,
        correct_answer,
    })
}

#[component]
pub fn SpellingTestPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| crate::account::current_user_name());

    // Bumped by the "Next" button below to force a fresh
    // spelling_test_round() fetch — same version-bump idiom as
    // ReviewPage's `grade.version().get()` and EditCardPage's
    // `card_refresh`.
    let round_refresh = RwSignal::new(0usize);
    let round = Resource::new(move || round_refresh.get(), |_| spelling_test_round());

    let answer = RwSignal::new(String::new());
    let feedback = RwSignal::new(None::<Result<SpellingResult, ServerFnError>>);

    let submit_answer = move |card_id: Uuid| {
        let guess = answer.get_untracked();
        leptos::task::spawn_local(async move {
            feedback.set(Some(submit_spelling_attempt(card_id, guess).await));
        });
    };

    let next_round = move |_| {
        answer.set(String::new());
        feedback.set(None);
        round_refresh.update(|n| *n += 1);
    };

    view! {
        <h1>"Spelling test"</h1>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                if auth.await.ok().flatten().is_none() {
                    return view! { <Redirect path="/login"/> }.into_any();
                }
                match round.await {
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    Ok(None) => view! {
                        <p>
                            "No cards with media yet — add an image, audio, video, or YouTube link to a card first."
                        </p>
                        <A href="/">"Back to cards"</A>
                    }
                    .into_any(),
                    Ok(Some(prompt)) => {
                        let card_id = prompt.card_id;
                        view! {
                            <div class="review-card">
                                {prompt.image_url.map(|url| view! { <img src=url alt="" /> })}
                                {prompt.audio_url.map(|url| view! { <audio controls src=url></audio> })}
                                {prompt.video_url.map(|url| view! { <video controls src=url></video> })}
                                {prompt.youtube_url.map(crate::app::youtube_embed)}
                            </div>
                            <label>
                                "Spelling"
                                <input
                                    type="text"
                                    prop:value=move || answer.get()
                                    on:input=move |ev| answer.set(event_target_value(&ev))
                                />
                            </label>
                            " "
                            <button type="button" on:click=move |_| submit_answer(card_id)>
                                "Submit"
                            </button>
                            {move || {
                                feedback.get().map(|result| match result {
                                    Ok(r) if r.correct => view! { <p>"Correct!"</p> }.into_any(),
                                    Ok(r) => view! {
                                        <p>
                                            "Not quite — the correct spelling is "
                                            {r.correct_answer}
                                            "."
                                        </p>
                                    }
                                    .into_any(),
                                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                                })
                            }}
                            {move || {
                                feedback
                                    .get()
                                    .is_some()
                                    .then(|| view! { <button type="button" on:click=next_round>"Next"</button> })
                            }}
                        }
                        .into_any()
                    }
                }
            })}
        </Transition>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn test_user(pool: &PgPool) -> Uuid {
        sqlx::query_scalar("INSERT INTO users (email, name) VALUES ($1, $2) RETURNING id")
            .bind("spelling-test@example.com")
            .bind("spelling-test")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Inserts a card directly (mirroring the literal `card_type` and
    /// `non_empty`-on-the-way-in handling `crate::db::create_card` uses,
    /// since this module intentionally doesn't call into `db.rs`).
    async fn test_card(
        pool: &PgPool,
        created_by: Uuid,
        target_text: &str,
        image_url: Option<&str>,
        audio_url: Option<&str>,
        video_url: Option<&str>,
        youtube_url: Option<&str>,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO cards (card_type, image_url, audio_url, video_url, youtube_url, target_text, created_by) \
             VALUES ('image_to_target', $1, $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(image_url)
        .bind(audio_url)
        .bind(video_url)
        .bind(youtube_url)
        .bind(target_text)
        .bind(created_by)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn logged_result(pool: &PgPool, user_id: Uuid, card_id: Uuid) -> bool {
        sqlx::query_scalar(
            "SELECT correct FROM spelling_test_attempts WHERE user_id = $1 AND card_id = $2",
        )
        .bind(user_id)
        .bind(card_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_card_with_no_media_is_never_returned(pool: PgPool) {
        let user_id = test_user(&pool).await;
        test_card(&pool, user_id, "word", None, None, None, None).await;

        assert!(fetch_round(&pool).await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_round_is_only_selected_from_cards_with_at_least_one_media_field(pool: PgPool) {
        let user_id = test_user(&pool).await;
        test_card(&pool, user_id, "no-media", None, None, None, None).await;
        let with_image = test_card(
            &pool,
            user_id,
            "has-image",
            Some("https://example.com/cat.jpg"),
            None,
            None,
            None,
        )
        .await;

        let round = fetch_round(&pool)
            .await
            .unwrap()
            .expect("a round should be offered");
        assert_eq!(round.id, with_image);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn submitting_the_exact_correct_spelling_logs_correct(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = test_card(
            &pool,
            user_id,
            "gato",
            Some("https://example.com/cat.jpg"),
            None,
            None,
            None,
        )
        .await;

        let correct_answer = fetch_target_text(&pool, card_id).await.unwrap();
        let correct = spellings_match("gato", &correct_answer);
        assert!(correct);

        record_attempt(&pool, user_id, card_id, correct)
            .await
            .unwrap();
        assert!(logged_result(&pool, user_id, card_id).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn submitting_a_wrong_spelling_logs_incorrect_and_still_returns_the_real_answer(
        pool: PgPool,
    ) {
        let user_id = test_user(&pool).await;
        let card_id = test_card(
            &pool,
            user_id,
            "gato",
            Some("https://example.com/cat.jpg"),
            None,
            None,
            None,
        )
        .await;

        let correct_answer = fetch_target_text(&pool, card_id).await.unwrap();
        let correct = spellings_match("gatto", &correct_answer);
        assert!(!correct);
        assert_eq!(correct_answer, "gato");

        record_attempt(&pool, user_id, card_id, correct)
            .await
            .unwrap();
        assert!(!logged_result(&pool, user_id, card_id).await);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_correct_spelling_with_different_case_and_surrounding_whitespace_is_still_accepted(
        pool: PgPool,
    ) {
        let user_id = test_user(&pool).await;
        let card_id = test_card(
            &pool,
            user_id,
            "Gato",
            Some("https://example.com/cat.jpg"),
            None,
            None,
            None,
        )
        .await;

        let correct_answer = fetch_target_text(&pool, card_id).await.unwrap();
        assert!(spellings_match("  GATO  ", &correct_answer));
    }

    proptest::proptest! {
        /// Whichever side of the comparison is the learner's answer vs.
        /// the stored correct text shouldn't matter — the match is a
        /// pure equality check on normalized form, so it must be
        /// symmetric for any two strings.
        #[test]
        fn spellings_match_is_symmetric(a in ".*", b in ".*") {
            proptest::prop_assert_eq!(spellings_match(&a, &b), spellings_match(&b, &a));
        }

        /// Surrounding whitespace and letter case must never change
        /// whether two spellings are judged to match.
        #[test]
        fn spellings_match_ignores_surrounding_whitespace_and_case(
            s in "[a-zA-Z ]{1,15}",
            leading in " {0,3}",
            trailing in " {0,3}",
        ) {
            let padded = format!("{leading}{}{trailing}", s.to_uppercase());
            proptest::prop_assert!(spellings_match(&s, &padded));
        }
    }
}

// #[component] already enforces call sites use the returned view; clippy's
// pedantic must_use_candidate doesn't see through the macro expansion —
// same rationale/precedent as app.rs's own identical allow.
#![allow(clippy::must_use_candidate)]

use leptos::prelude::*;
use leptos_router::components::{A, Redirect};
#[cfg(feature = "ssr")]
use rand::RngExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One drillable minimal-pair round: an audio prompt (one card's
/// `audio_url`) plus the two cards' texts as options, in an order that
/// carries no information about which one is correct — see
/// `minimal_pair_round`'s coin flips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinimalPairRound {
    pub prompt_audio_url: String,
    pub prompt_card_id: Uuid,
    pub option_a: (Uuid, String), // (card_id, target_text)
    pub option_b: (Uuid, String),
}

/// A `card_links`/`cards` join row for one minimal-pair round, before the
/// prompt-side/slot-order coin flips in `minimal_pair_round` below decide
/// which card is the prompt and which UI slot each lands in. A named
/// module-level type rather than a function-local one, matching this
/// file's other query-row structs — `a`/`b` deliberately match the SQL's
/// own join aliases.
#[cfg(feature = "ssr")]
#[derive(sqlx::FromRow)]
struct PairCandidateRow {
    card_id: Uuid,
    linked_card_id: Uuid,
    a_audio_url: String,
    a_target_text: String,
    b_audio_url: String,
    b_target_text: String,
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

// Named explicitly to avoid colliding with the `MinimalPairRound` return
// type above — the `#[server]` macro otherwise names the generated
// (here, empty) arguments struct the PascalCase of the function name,
// which for `minimal_pair_round` is exactly `MinimalPairRound`.
#[server(name = MinimalPairRoundArgs)]
pub async fn minimal_pair_round() -> Result<Option<MinimalPairRound>, ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();

    // A pair with no audio on either side isn't drillable as a
    // sound-discrimination exercise, so it's skipped entirely rather than
    // degrading to something else.
    let row: Option<PairCandidateRow> = sqlx::query_as(
        "SELECT cl.card_id, cl.linked_card_id, \
         a.audio_url AS a_audio_url, a.target_text AS a_target_text, \
         b.audio_url AS b_audio_url, b.target_text AS b_target_text \
         FROM card_links cl \
         JOIN cards a ON a.id = cl.card_id \
         JOIN cards b ON b.id = cl.linked_card_id \
         WHERE cl.link_type = 'minimal_pair' \
           AND a.audio_url IS NOT NULL AND b.audio_url IS NOT NULL \
         ORDER BY random() LIMIT 1",
    )
    .fetch_optional(&state.db_pool)
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    let Some(row) = row else {
        return Ok(None);
    };

    // Two independent coin flips: which of the two cards' audio becomes
    // the prompt, and which UI slot (option_a/option_b) each card's text
    // lands in. Independent on purpose, so the "correct" option never
    // correlates with a fixed slot or with whichever card happened to be
    // `card_id` vs `linked_card_id` in storage.
    let mut rng = rand::rng();
    let prompt_is_a: bool = rng.random();
    let swap_slots: bool = rng.random();

    let (prompt_card_id, prompt_audio_url) = if prompt_is_a {
        (row.card_id, row.a_audio_url)
    } else {
        (row.linked_card_id, row.b_audio_url)
    };

    let a_option = (row.card_id, row.a_target_text);
    let b_option = (row.linked_card_id, row.b_target_text);
    let (option_a, option_b) = if swap_slots {
        (b_option, a_option)
    } else {
        (a_option, b_option)
    };

    Ok(Some(MinimalPairRound {
        prompt_audio_url,
        prompt_card_id,
        option_a,
        option_b,
    }))
}

#[server]
// `card_a_id`/`card_b_id` mirror the table's own column names (and the
// pair's inherent a/b shape) — clearer here than any less-similar
// alternative would be.
#[allow(clippy::similar_names)]
pub async fn submit_minimal_pair_attempt(
    card_a_id: Uuid,
    card_b_id: Uuid,
    picked_card_id: Uuid,
    correct: bool,
) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    sqlx::query(
        "INSERT INTO minimal_pair_attempts \
         (user_id, card_a_id, card_b_id, picked_card_id, correct) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user.id)
    .bind(card_a_id)
    .bind(card_b_id)
    .bind(picked_card_id)
    .bind(correct)
    .execute(&state.db_pool)
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

/// Local, client-side-only record of the just-submitted attempt's outcome,
/// shown as immediate feedback without waiting on a round refetch.
#[derive(Debug, Clone)]
struct AttemptFeedback {
    correct: bool,
    other_text: String,
}

#[component]
// `card_a_id`/`card_b_id` (and their `text_a`/`text_b`,
// `text_a_for_click`/`text_b_for_click` counterparts below) mirror the
// pair's own inherent a/b shape — clearer here than any less-similar
// alternative would be.
#[allow(clippy::similar_names)]
pub fn MinimalPairTrainerPage() -> impl IntoView {
    let auth = Resource::new(|| (), |()| crate::account::current_user_name());
    let round_refresh = RwSignal::new(0usize);
    let round = Resource::new(move || round_refresh.get(), |_| minimal_pair_round());
    let submit_attempt = ServerAction::<SubmitMinimalPairAttempt>::new();
    let feedback = RwSignal::new(None::<AttemptFeedback>);

    view! {
        <h1>"Minimal pairs"</h1>
        <Transition fallback=|| view! { <p>"Loading…"</p> }>
            {move || Suspend::new(async move {
                if auth.await.ok().flatten().is_none() {
                    return view! { <Redirect path="/login"/> }.into_any();
                }
                match round.await {
                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                    Ok(None) => view! {
                        <p>
                            "No minimal pairs with audio yet — link two cards with audio as a minimal pair from either card's edit page first."
                        </p>
                        <p><A href="/">"Back to cards"</A></p>
                    }
                    .into_any(),
                    Ok(Some(r)) => {
                        let prompt_card_id = r.prompt_card_id;
                        let (card_a_id, text_a) = r.option_a.clone();
                        let (card_b_id, text_b) = r.option_b.clone();

                        let pick = move |picked_card_id: Uuid, other_text: String| {
                            let correct = picked_card_id == prompt_card_id;
                            feedback.set(Some(AttemptFeedback { correct, other_text }));
                            submit_attempt.dispatch(SubmitMinimalPairAttempt {
                                card_a_id,
                                card_b_id,
                                picked_card_id,
                                correct,
                            });
                        };

                        let text_a_for_click = text_a.clone();
                        let text_b_for_click = text_b.clone();

                        view! {
                            <div class="review-card">
                                <audio controls src=r.prompt_audio_url.clone()></audio>
                                <div class="review-grades">
                                    <button
                                        disabled=move || feedback.get().is_some()
                                        on:click=move |_| pick(card_a_id, text_b_for_click.clone())
                                    >
                                        {text_a.clone()}
                                    </button>
                                    <button
                                        disabled=move || feedback.get().is_some()
                                        on:click=move |_| pick(card_b_id, text_a_for_click.clone())
                                    >
                                        {text_b.clone()}
                                    </button>
                                </div>
                                {move || {
                                    feedback.get().map(|f| {
                                        if f.correct {
                                            view! { <p>"Correct!"</p> }.into_any()
                                        } else {
                                            view! {
                                                <p>"Not quite — it was " {f.other_text.clone()} "."</p>
                                            }
                                            .into_any()
                                        }
                                    })
                                }}
                                {move || {
                                    feedback.get().is_some().then(|| {
                                        view! {
                                            <button on:click=move |_| {
                                                feedback.set(None);
                                                round_refresh.update(|n| *n += 1);
                                            }>
                                                "Next"
                                            </button>
                                        }
                                    })
                                }}
                            </div>
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
            .bind("minimal-pair-test@example.com")
            .bind("minimal-pair-test")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn test_card(
        pool: &PgPool,
        user_id: Uuid,
        target_text: &str,
        audio_url: Option<&str>,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO cards (card_type, audio_url, target_text, created_by) \
             VALUES ('basic', $1, $2, $3) RETURNING id",
        )
        .bind(audio_url)
        .bind(target_text)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn link_as_minimal_pair(pool: &PgPool, card_id: Uuid, linked_card_id: Uuid) {
        sqlx::query("INSERT INTO card_links (card_id, linked_card_id, link_type) VALUES ($1, $2, 'minimal_pair')")
            .bind(card_id)
            .bind(linked_card_id)
            .execute(pool)
            .await
            .unwrap();
    }

    /// Mirrors `minimal_pair_round`'s query directly against the pool,
    /// without going through the `#[server]` wrapper (which needs a live
    /// Leptos server context to call `expect_context::<AppState>()`, not
    /// available in a plain `#[sqlx::test]`).
    async fn pick_a_round_row(
        pool: &PgPool,
    ) -> Option<(Uuid, Uuid, String, String, String, String)> {
        sqlx::query_as(
            "SELECT cl.card_id, cl.linked_card_id, \
             a.audio_url AS a_audio_url, a.target_text AS a_target_text, \
             b.audio_url AS b_audio_url, b.target_text AS b_target_text \
             FROM card_links cl \
             JOIN cards a ON a.id = cl.card_id \
             JOIN cards b ON b.id = cl.linked_card_id \
             WHERE cl.link_type = 'minimal_pair' \
               AND a.audio_url IS NOT NULL AND b.audio_url IS NOT NULL \
             ORDER BY random() LIMIT 1",
        )
        .fetch_optional(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn round_query_skips_pairs_missing_audio_on_either_side(pool: PgPool) {
        let user_id = test_user(&pool).await;
        // No audio on either side — not drillable.
        let silent_a = test_card(&pool, user_id, "silent-a", None).await;
        let silent_b = test_card(&pool, user_id, "silent-b", None).await;
        link_as_minimal_pair(&pool, silent_a, silent_b).await;

        // Audio on only one side — still not drillable.
        let half_a = test_card(&pool, user_id, "half-a", Some("half-a.mp3")).await;
        let half_b = test_card(&pool, user_id, "half-b", None).await;
        link_as_minimal_pair(&pool, half_a, half_b).await;

        assert!(pick_a_round_row(&pool).await.is_none());

        // Audio on both sides — now it's drillable.
        let full_a = test_card(&pool, user_id, "full-a", Some("full-a.mp3")).await;
        let full_b = test_card(&pool, user_id, "full-b", Some("full-b.mp3")).await;
        link_as_minimal_pair(&pool, full_a, full_b).await;

        let row = pick_a_round_row(&pool).await.unwrap();
        let ids = [row.0, row.1];
        assert!(ids.contains(&full_a) && ids.contains(&full_b));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn submitting_an_attempt_persists_it_with_the_right_outcome(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_a = test_card(&pool, user_id, "pair-a", Some("a.mp3")).await;
        let card_b = test_card(&pool, user_id, "pair-b", Some("b.mp3")).await;

        // A correct pick: picked_card_id matches what's asserted correct.
        sqlx::query(
            "INSERT INTO minimal_pair_attempts \
             (user_id, card_a_id, card_b_id, picked_card_id, correct) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(user_id)
        .bind(card_a)
        .bind(card_b)
        .bind(card_a)
        .bind(true)
        .execute(&pool)
        .await
        .unwrap();

        // A wrong pick on the very same pair.
        sqlx::query(
            "INSERT INTO minimal_pair_attempts \
             (user_id, card_a_id, card_b_id, picked_card_id, correct) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(user_id)
        .bind(card_a)
        .bind(card_b)
        .bind(card_b)
        .bind(false)
        .execute(&pool)
        .await
        .unwrap();

        let rows: Vec<(Uuid, bool)> = sqlx::query_as(
            "SELECT picked_card_id, correct FROM minimal_pair_attempts \
             WHERE user_id = $1 ORDER BY attempted_at",
        )
        .bind(user_id)
        .fetch_all(&pool)
        .await
        .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], (card_a, true));
        assert_eq!(rows[1], (card_b, false));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn attempt_rows_reference_the_cards_that_were_actually_in_play(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_a = test_card(&pool, user_id, "ref-a", Some("a.mp3")).await;
        let card_b = test_card(&pool, user_id, "ref-b", Some("b.mp3")).await;
        link_as_minimal_pair(&pool, card_a, card_b).await;

        sqlx::query(
            "INSERT INTO minimal_pair_attempts \
             (user_id, card_a_id, card_b_id, picked_card_id, correct) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(user_id)
        .bind(card_a)
        .bind(card_b)
        .bind(card_a)
        .bind(true)
        .execute(&pool)
        .await
        .unwrap();

        let (stored_a, stored_b, stored_picked): (Uuid, Uuid, Uuid) = sqlx::query_as(
            "SELECT card_a_id, card_b_id, picked_card_id FROM minimal_pair_attempts WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(stored_a, card_a);
        assert_eq!(stored_b, card_b);
        assert_eq!(stored_picked, card_a);
    }
}

//! The back-chaining pronunciation trainer: walk a `back_chain_step` chain
//! of cards one step at a time (see the `card_links` migration for the
//! schema this reads). Deliberately stateless — nothing is persisted, so
//! there's no migration and no auth requirement, unlike the other two
//! trainers built alongside this one. This also deliberately writes raw
//! `sqlx::query*` calls against `state.db_pool` directly rather than going
//! through `crate::db`, to avoid a shared-file dependency with that
//! parallel work.

// #[component] already enforces call sites use the returned view; clippy's
// pedantic must_use_candidate doesn't see through the macro expansion —
// same rationale/precedent as app.rs's own identical allow.
#![allow(clippy::must_use_candidate)]

use leptos::prelude::*;
use leptos::task::spawn_local;
use uuid::Uuid;

#[cfg(feature = "ssr")]
#[derive(Debug, Clone, sqlx::FromRow)]
struct CardRow {
    id: Uuid,
    image_url: Option<String>,
    audio_url: Option<String>,
    video_url: Option<String>,
    youtube_url: Option<String>,
    target_text: String,
    two_way: bool,
}

#[cfg(feature = "ssr")]
impl From<CardRow> for crate::cards::Card {
    fn from(r: CardRow) -> Self {
        // Manual field-by-field construction, matching this codebase's
        // existing convention (see cards.rs's server fns) rather than a
        // generic From impl living on crate::cards::Card itself.
        crate::cards::Card {
            id: r.id,
            image_url: r.image_url,
            audio_url: r.audio_url,
            video_url: r.video_url,
            youtube_url: r.youtube_url,
            target_text: r.target_text,
            two_way: r.two_way,
        }
    }
}

/// Cards that have an outgoing `back_chain_step` edge but no incoming
/// one — i.e. where a chain begins. Pulled out of the `#[server]` fn
/// below so the exact same query is what the unit tests in this file
/// exercise, rather than a copy that could drift from it.
#[cfg(feature = "ssr")]
async fn starting_card_rows(pool: &sqlx::PgPool) -> sqlx::Result<Vec<CardRow>> {
    sqlx::query_as(
        "SELECT DISTINCT c.id, c.image_url, c.audio_url, c.video_url, c.youtube_url, \
                c.target_text, c.two_way \
         FROM cards c \
         JOIN card_links cl ON cl.card_id = c.id AND cl.link_type = 'back_chain_step' \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM card_links cl2 \
             WHERE cl2.link_type = 'back_chain_step' AND cl2.linked_card_id = c.id \
         ) \
         ORDER BY c.target_text",
    )
    .fetch_all(pool)
    .await
}

/// The next step after `card_id` in a back-chain, or `None` at the end of
/// the chain. A card could in principle have more than one outgoing
/// `back_chain_step` edge (an accepted v1 schema gap — see the
/// `card_links` migration); `ORDER BY c.target_text LIMIT 1` is exactly
/// how that possible branch is deterministically resolved, picking the
/// same card every time rather than rejecting or detecting the branch.
#[cfg(feature = "ssr")]
async fn next_step_row(pool: &sqlx::PgPool, card_id: Uuid) -> sqlx::Result<Option<CardRow>> {
    sqlx::query_as(
        "SELECT c.id, c.image_url, c.audio_url, c.video_url, c.youtube_url, \
                c.target_text, c.two_way \
         FROM card_links cl \
         JOIN cards c ON c.id = cl.linked_card_id \
         WHERE cl.card_id = $1 AND cl.link_type = 'back_chain_step' \
         ORDER BY c.target_text LIMIT 1",
    )
    .bind(card_id)
    .fetch_optional(pool)
    .await
}

/// Cards that begin a back-chain. Public/no-auth — browsing the shared
/// card pool needs no account, same as `list_cards`/`list_decks`.
#[server]
pub async fn back_chain_starting_cards() -> Result<Vec<crate::cards::Card>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let rows = starting_card_rows(&state.db_pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// The next step after `card_id` in a back-chain, or `None` at the end of
/// the chain.
#[server]
pub async fn back_chain_next_step(
    card_id: Uuid,
) -> Result<Option<crate::cards::Card>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let row = next_step_row(&state.db_pool, card_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(row.map(Into::into))
}

#[component]
pub fn BackChainGamePage() -> impl IntoView {
    let chain: RwSignal<Vec<crate::cards::Card>> = RwSignal::new(Vec::new());
    let at_end: RwSignal<bool> = RwSignal::new(false);
    let starting_cards = Resource::new(|| (), |()| back_chain_starting_cards());

    let next_step = move |_| {
        at_end.set(false);
        let Some(last) = chain.get_untracked().last().cloned() else {
            return;
        };
        spawn_local(async move {
            match back_chain_next_step(last.id).await {
                Ok(Some(card)) => chain.update(|c| c.push(card)),
                Ok(None) => at_end.set(true),
                Err(_) => {}
            }
        });
    };

    view! {
        <h1>"Back-chain"</h1>
        {move || {
            if chain.get().is_empty() {
                view! {
                    <div class="card-section">
                        <p class="card-section-heading">"Start a chain"</p>
                        <Transition fallback=|| view! { <p>"Loading…"</p> }>
                            {move || Suspend::new(async move {
                                match starting_cards.await {
                                    Ok(cards) if cards.is_empty() => {
                                        view! { <p>"No back-chain sequences yet."</p> }.into_any()
                                    }
                                    Ok(cards) => cards
                                        .into_iter()
                                        .map(|card| {
                                            let label = card.target_text.clone();
                                            let starter = card.clone();
                                            view! {
                                                <button
                                                    type="button"
                                                    on:click=move |_| {
                                                        chain.set(vec![starter.clone()]);
                                                        at_end.set(false);
                                                    }
                                                >
                                                    {label}
                                                </button>
                                            }
                                        })
                                        .collect_view()
                                        .into_any(),
                                    Err(e) => view! { <p>"Error: " {e.to_string()}</p> }.into_any(),
                                }
                            })}
                        </Transition>
                    </div>
                }
                .into_any()
            } else {
                view! {
                    {chain.get().into_iter().map(|card| {
                        // Same shape as ReviewPage's media_view in app.rs
                        // — no autoplay, every element is manually
                        // triggered via `controls`.
                        view! {
                            <div class="review-card">
                                <p>{card.target_text}</p>
                                {card.image_url.map(|url| view! { <img src=url alt="" /> })}
                                {card.audio_url.map(|url| view! { <audio controls src=url></audio> })}
                                {card.video_url.map(|url| view! { <video controls src=url></video> })}
                                {card.youtube_url.map(|url| view! {
                                    <iframe src=url width="560" height="315" allowfullscreen=true></iframe>
                                })}
                            </div>
                        }
                    }).collect_view()}
                    <div>
                        {move || if at_end.get() {
                            view! { <p>"End of chain."</p> }.into_any()
                        } else {
                            view! {
                                <button type="button" on:click=next_step>"Next step"</button>
                            }.into_any()
                        }}
                        " "
                        <button
                            type="button"
                            on:click=move |_| {
                                chain.set(Vec::new());
                                at_end.set(false);
                            }
                        >
                            "Start over"
                        </button>
                    </div>
                }
                .into_any()
            }
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn test_user(pool: &PgPool) -> Uuid {
        sqlx::query_scalar("INSERT INTO users (email, name) VALUES ($1, $2) RETURNING id")
            .bind("back-chain-test@example.com")
            .bind("back-chain-test")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn card_with_text(pool: &PgPool, user_id: Uuid, target_text: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO cards (card_type, target_text, created_by) \
             VALUES ('image_to_target', $1, $2) RETURNING id",
        )
        .bind(target_text)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn link_back_chain_step(pool: &PgPool, earlier: Uuid, later: Uuid) {
        sqlx::query(
            "INSERT INTO card_links (card_id, linked_card_id, link_type) \
             VALUES ($1, $2, 'back_chain_step')",
        )
        .bind(earlier)
        .bind(later)
        .execute(pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_card_that_starts_a_chain_is_found_as_a_starting_card(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let first = card_with_text(&pool, user_id, "na").await;
        let second = card_with_text(&pool, user_id, "nation").await;
        link_back_chain_step(&pool, first, second).await;

        let rows = starting_card_rows(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, first);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_card_with_an_incoming_edge_is_not_a_starting_card_even_with_its_own_outgoing_edge(
        pool: PgPool,
    ) {
        let user_id = test_user(&pool).await;
        let first = card_with_text(&pool, user_id, "na").await;
        let middle = card_with_text(&pool, user_id, "nation").await;
        let last = card_with_text(&pool, user_id, "an explanation").await;
        link_back_chain_step(&pool, first, middle).await;
        link_back_chain_step(&pool, middle, last).await;

        let rows = starting_card_rows(&pool).await.unwrap();

        // `middle` has both an incoming and an outgoing edge, but the
        // incoming one disqualifies it as a starting card.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, first);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn next_step_returns_the_next_card_and_none_at_the_end_of_the_chain(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let first = card_with_text(&pool, user_id, "na").await;
        let second = card_with_text(&pool, user_id, "nation").await;
        link_back_chain_step(&pool, first, second).await;

        let next = next_step_row(&pool, first).await.unwrap();
        assert_eq!(next.unwrap().id, second);

        let end = next_step_row(&pool, second).await.unwrap();
        assert!(end.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_card_with_two_outgoing_edges_still_returns_exactly_one_deterministic_result(
        pool: PgPool,
    ) {
        let user_id = test_user(&pool).await;
        let branching = card_with_text(&pool, user_id, "branch").await;
        let option_a = card_with_text(&pool, user_id, "a-branch").await;
        let option_b = card_with_text(&pool, user_id, "b-branch").await;
        link_back_chain_step(&pool, branching, option_a).await;
        link_back_chain_step(&pool, branching, option_b).await;

        let first_call = next_step_row(&pool, branching).await.unwrap().unwrap();
        let second_call = next_step_row(&pool, branching).await.unwrap().unwrap();

        // "a-branch" sorts before "b-branch", so option_a is the
        // deterministic pick both times.
        assert_eq!(first_call.id, option_a);
        assert_eq!(second_call.id, option_a);
        assert_eq!(first_call.id, second_call.id);
    }
}

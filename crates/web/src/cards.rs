use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: Uuid,
    pub card_type: String,
    pub image_url: Option<String>,
    pub audio_url: Option<String>,
    pub video_url: Option<String>,
    pub youtube_url: Option<String>,
    pub target_text: String,
    /// The known-language sentence, `sentence_translation` cards only —
    /// `None` for every other card type.
    pub source_text: Option<String>,
    pub two_way: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardDetail {
    pub id: Uuid,
    pub card_type: String,
    pub image_url: Option<String>,
    pub audio_url: Option<String>,
    pub video_url: Option<String>,
    pub youtube_url: Option<String>,
    pub target_text: String,
    pub source_text: Option<String>,
    pub two_way: bool,
    pub tags: Vec<String>,
    pub deck_ids: Vec<Uuid>,
    pub page_ids: Vec<Uuid>,
    pub linked_cards: Vec<CardLink>,
}

/// The known card types: (value stored in the DB, UI label). Same
/// "flat TEXT, validated against one shared list" convention as
/// `LINK_TYPES` just below — a card type doesn't need its own Rust enum
/// any more than a link type does, since nothing branches on it in SQL,
/// only in which form fields are shown and how review renders a card.
pub const CARD_TYPES: &[(&str, &str)] = &[
    ("image_to_target", "Image → written"),
    ("sound_to_written", "Audio → written"),
    ("image_to_audio", "Image → audio"),
    ("sentence_translation", "Sentence translation"),
];

/// A typed relationship to another card (minimal pair, back-chain step,
/// sound-spelling correspondence — see `LINK_TYPES`). `is_source` is only
/// meaningful for the directional type: true means this card was stored as
/// the earlier step relative to `linked_card_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardLink {
    pub linked_card_id: Uuid,
    pub target_text: String,
    pub link_type: String,
    pub is_source: bool,
}

/// The known card-relationship types: (value stored in the DB, UI label,
/// whether order between the two cards is meaningful). Shared between
/// `add_card_link`'s validation and `app.rs`'s `CardLinksSection` UI so
/// there's exactly one place that knows what a valid `link_type` is.
pub const LINK_TYPES: &[(&str, &str, bool)] = &[
    ("minimal_pair", "Minimal pair", false),
    ("back_chain_step", "Back-chain step", true),
    ("sound_spelling", "Sound-spelling correspondence", false),
];

/// Which side of a review-queue card is the prompt. See
/// `crate::db::Direction` for why this is a queue-slot property, not a
/// card property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectionInput {
    Forward,
    Reverse,
}

#[cfg(feature = "ssr")]
impl From<crate::db::Direction> for DirectionInput {
    fn from(direction: crate::db::Direction) -> Self {
        match direction {
            crate::db::Direction::Forward => DirectionInput::Forward,
            crate::db::Direction::Reverse => DirectionInput::Reverse,
        }
    }
}

#[cfg(feature = "ssr")]
impl From<DirectionInput> for crate::db::Direction {
    fn from(direction: DirectionInput) -> Self {
        match direction {
            DirectionInput::Forward => crate::db::Direction::Forward,
            DirectionInput::Reverse => crate::db::Direction::Reverse,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCard {
    pub id: Uuid,
    pub card_type: String,
    pub image_url: Option<String>,
    pub audio_url: Option<String>,
    pub video_url: Option<String>,
    pub youtube_url: Option<String>,
    pub target_text: String,
    pub source_text: Option<String>,
    pub direction: DirectionInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewQueueView {
    pub cards: Vec<ReviewCard>,
    pub due_review_count: i64,
    pub new_available_today: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum GradeInput {
    Again,
    Hard,
    Good,
    Easy,
}

#[cfg(feature = "ssr")]
impl From<GradeInput> for domain::srs::Grade {
    fn from(grade: GradeInput) -> Self {
        match grade {
            GradeInput::Again => domain::srs::Grade::Again,
            GradeInput::Hard => domain::srs::Grade::Hard,
            GradeInput::Good => domain::srs::Grade::Good,
            GradeInput::Easy => domain::srs::Grade::Easy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewOrderInput {
    DueDate,
    Random,
    HardestFirst,
}

#[cfg(feature = "ssr")]
impl From<ReviewOrderInput> for crate::db::ReviewOrder {
    fn from(order: ReviewOrderInput) -> Self {
        match order {
            ReviewOrderInput::DueDate => crate::db::ReviewOrder::DueDate,
            ReviewOrderInput::Random => crate::db::ReviewOrder::Random,
            ReviewOrderInput::HardestFirst => crate::db::ReviewOrder::HardestFirst,
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

#[server]
pub async fn list_cards(
    tag: Option<String>,
    deck_id: Option<Uuid>,
) -> Result<Vec<Card>, ServerFnError> {
    // Browsing the shared card pool is public; only mutating it needs auth.
    let state = expect_context::<crate::state::AppState>();
    let rows = crate::db::list_cards(&state.db_pool, tag.as_deref(), deck_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|r| Card {
            id: r.id,
            card_type: r.card_type,
            image_url: r.image_url,
            audio_url: r.audio_url,
            video_url: r.video_url,
            youtube_url: r.youtube_url,
            target_text: r.target_text,
            source_text: r.source_text,
            two_way: r.two_way,
        })
        .collect())
}

#[server]
pub async fn list_tags() -> Result<Vec<String>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    crate::db::list_tags(&state.db_pool)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn get_card(card_id: Uuid) -> Result<Option<CardDetail>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let Some(card) = crate::db::get_card(&state.db_pool, card_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
    else {
        return Ok(None);
    };
    // Four independent reads, each keyed only on `card_id` — none depends
    // on another's result. Previously awaited one at a time (5 serialized
    // Neon round trips per get_card call, holding a pool connection for
    // the sum of all five instead of the slowest one); under concurrent
    // load that compounds directly into exactly the kind of latency that
    // blows a client-side e2e timeout. Running them concurrently cuts
    // get_card's wall-clock cost to roughly one round trip.
    let (tags, deck_ids, page_ids, linked_cards) = tokio::try_join!(
        crate::db::card_tags(&state.db_pool, card_id),
        crate::db::card_deck_ids(&state.db_pool, card_id),
        crate::db::card_page_ids(&state.db_pool, card_id),
        crate::db::linked_cards(&state.db_pool, card_id),
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    let linked_cards = linked_cards
        .into_iter()
        .map(|l| CardLink {
            linked_card_id: l.linked_card_id,
            target_text: l.target_text,
            link_type: l.link_type,
            is_source: l.is_source,
        })
        .collect();
    Ok(Some(CardDetail {
        id: card.id,
        card_type: card.card_type,
        source_text: card.source_text,
        image_url: card.image_url,
        audio_url: card.audio_url,
        video_url: card.video_url,
        youtube_url: card.youtube_url,
        target_text: card.target_text,
        two_way: card.two_way,
        tags,
        deck_ids,
        page_ids,
        linked_cards,
    }))
}

#[server]
pub async fn add_card_link(
    card_id: Uuid,
    linked_card_id: Uuid,
    link_type: String,
) -> Result<(), ServerFnError> {
    require_user().await?;
    if card_id == linked_card_id {
        return Err(ServerFnError::new("a card can't be linked to itself"));
    }
    if !LINK_TYPES.iter().any(|(value, _, _)| *value == link_type) {
        return Err(ServerFnError::new("unknown link type"));
    }
    let state = expect_context::<crate::state::AppState>();
    crate::db::add_card_link(&state.db_pool, card_id, linked_card_id, &link_type)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn remove_card_link(
    card_id: Uuid,
    linked_card_id: Uuid,
    link_type: String,
) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::remove_card_link(&state.db_pool, card_id, linked_card_id, &link_type)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn search_cards(
    query: String,
    exclude_card_id: Uuid,
) -> Result<Vec<Card>, ServerFnError> {
    // Public, same as list_cards/get_card: browsing the shared card pool
    // needs no auth, only mutating it does.
    let state = expect_context::<crate::state::AppState>();
    let rows = crate::db::search_cards(&state.db_pool, &query, exclude_card_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|r| Card {
            id: r.id,
            card_type: r.card_type,
            image_url: r.image_url,
            audio_url: r.audio_url,
            video_url: r.video_url,
            youtube_url: r.youtube_url,
            target_text: r.target_text,
            source_text: r.source_text,
            two_way: r.two_way,
        })
        .collect())
}

/// A client-facing image search result — see
/// `crate::image_search::ImageResult`, which this mirrors field for field.
/// A separate type so the server function's generated client stub doesn't
/// need `#[cfg(feature = "ssr")]` on the whole module just to name the
/// ssr-only `image_search` type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSearchResult {
    pub provider: String,
    pub id: String,
    pub thumbnail_url: String,
    pub description: String,
}

#[cfg(feature = "ssr")]
impl From<crate::image_search::ImageResult> for ImageSearchResult {
    fn from(r: crate::image_search::ImageResult) -> Self {
        Self {
            provider: r.provider,
            id: r.id,
            thumbnail_url: r.thumbnail_url,
            description: r.description,
        }
    }
}

#[server]
pub async fn search_card_images(query: String) -> Result<Vec<ImageSearchResult>, ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    Ok(state
        .image_search
        .search(&query)
        .await
        .into_iter()
        .map(Into::into)
        .collect())
}

/// Fetches a specific search result's full-resolution image server-side
/// (re-querying the provider by id, never the thumbnail URL the browser
/// already has — see `ImageSearch::fetch_by_id`) and uploads it to our own
/// storage, so a card's image is always something we host, the same as a
/// directly uploaded file. Returns the resulting URL for the new/edit card
/// form to carry as a plain hidden field (see `parse_card_multipart` in
/// main.rs).
#[server]
pub async fn import_card_image(provider: String, id: String) -> Result<String, ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let (bytes, content_type) = state
        .image_search
        .fetch_by_id(&provider, &id)
        .await
        .map_err(ServerFnError::new)?;
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(|| ServerFnError::new("image storage isn't configured"))?;
    let key = format!("cards/{}", crate::storage::random_key());
    storage
        .upload(&key, &content_type, &bytes)
        .await
        .map_err(ServerFnError::new)
}

#[server]
pub async fn delete_card(card_id: Uuid) -> Result<(), ServerFnError> {
    require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::delete_card(&state.db_pool, card_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

#[server]
pub async fn review_queue(
    deck_id: Option<Uuid>,
    tag: Option<String>,
    order: ReviewOrderInput,
) -> Result<ReviewQueueView, ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let queue = crate::db::review_queue(
        &state.db_pool,
        user.id,
        deck_id,
        tag.as_deref(),
        order.into(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(ReviewQueueView {
        cards: queue
            .cards
            .into_iter()
            .map(|r| ReviewCard {
                id: r.id,
                card_type: r.card_type,
                source_text: r.source_text,
                image_url: r.image_url,
                audio_url: r.audio_url,
                video_url: r.video_url,
                youtube_url: r.youtube_url,
                target_text: r.target_text,
                direction: r.direction.into(),
            })
            .collect(),
        due_review_count: queue.due_review_count,
        new_available_today: queue.new_available_today,
    })
}

#[server]
pub async fn grade_card(
    card_id: Uuid,
    grade: GradeInput,
    direction: DirectionInput,
) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::record_review(
        &state.db_pool,
        user.id,
        card_id,
        grade.into(),
        direction.into(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

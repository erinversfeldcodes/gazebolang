#![cfg(feature = "ssr")]
// These just propagate sqlx::Error as-is; there's nothing to document
// beyond what sqlx::Result already says.
#![allow(clippy::missing_errors_doc)]

use domain::srs::{Grade, ReviewState};
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CardRow {
    pub id: Uuid,
    pub image_url: Option<String>,
    pub audio_url: Option<String>,
    pub video_url: Option<String>,
    /// A `YouTube` embed URL (`https://www.youtube.com/embed/{id}`),
    /// already normalized at write time — never a raw `watch?v=`/`youtu.be`
    /// link, so rendering it is just `<iframe src=...>` with no further
    /// parsing needed. Independent of `video_url`: a card links a `YouTube`
    /// video instead of an uploaded/recorded one, not alongside it, though
    /// nothing stops both being set if a caller really wants that.
    pub youtube_url: Option<String>,
    pub target_text: String,
    pub two_way: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeckRow {
    pub id: Uuid,
    pub name: String,
    /// One card's `target_text` from this deck, for the Decks page's
    /// hover-peek animation — `None` for an empty deck.
    pub sample_word: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewOrder {
    DueDate,
    Random,
    HardestFirst,
}

/// Which side of a card is shown first. A two-way card is reviewed in
/// both directions — each scheduled and graded independently (Anki's
/// "Basic and reversed" behavior) — so this isn't a card property, it's
/// a property of one specific review-queue slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Media (image/audio/video) shown first, `target_text` is the answer.
    Forward,
    /// `target_text` shown first, media is the answer.
    Reverse,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Direction::Forward => "forward",
            Direction::Reverse => "reverse",
        }
    }

    fn from_db(s: &str) -> Self {
        match s {
            "reverse" => Direction::Reverse,
            _ => Direction::Forward,
        }
    }
}

/// A card as it appears in one specific review-queue slot: besides the
/// card's own content, carries which direction this slot is — the same
/// two-way card can appear as two independent slots, one per direction.
#[derive(Debug, Clone)]
pub struct ReviewQueueCard {
    pub id: Uuid,
    pub image_url: Option<String>,
    pub audio_url: Option<String>,
    pub video_url: Option<String>,
    pub youtube_url: Option<String>,
    pub target_text: String,
    pub direction: Direction,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ReviewQueueCardRow {
    id: Uuid,
    image_url: Option<String>,
    audio_url: Option<String>,
    video_url: Option<String>,
    youtube_url: Option<String>,
    target_text: String,
    direction: String,
}

impl From<ReviewQueueCardRow> for ReviewQueueCard {
    fn from(row: ReviewQueueCardRow) -> Self {
        ReviewQueueCard {
            id: row.id,
            image_url: row.image_url,
            audio_url: row.audio_url,
            video_url: row.video_url,
            youtube_url: row.youtube_url,
            target_text: row.target_text,
            direction: Direction::from_db(&row.direction),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReviewQueue {
    pub cards: Vec<ReviewQueueCard>,
    pub due_review_count: i64,
    pub new_available_today: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HardCardRow {
    pub id: Uuid,
    pub target_text: String,
    pub difficulty: f32,
    pub lapses: i32,
}

#[derive(Debug, Clone, Default)]
pub struct UserStats {
    pub total_cards: i64,
    pub due_now: i64,
    pub reviews_today: i64,
    pub current_streak_days: i64,
    pub hardest_cards: Vec<HardCardRow>,
}

/// Fields that make up a card's own content, as opposed to its tags or
/// deck memberships. A plain struct rather than more positional
/// arguments: `create_card`/`update_card` would otherwise take two bools
/// and three optional-URL strings in a row, which is exactly the kind of
/// signature that's easy to miscall without the compiler catching it.
/// `Default` means a test that only cares about `target_text` can write
/// `NewCard { target_text: "word", ..Default::default() }` instead of
/// spelling out every unused field.
#[derive(Debug, Clone, Copy, Default)]
pub struct NewCard<'a> {
    pub image_url: &'a str,
    pub audio_url: &'a str,
    pub video_url: &'a str,
    /// Must already be a normalized `https://www.youtube.com/embed/{id}`
    /// URL — callers are responsible for parsing/validating whatever the
    /// user actually pasted before this point (see
    /// `main.rs::youtube_embed_url`).
    pub youtube_url: &'a str,
    pub target_text: &'a str,
    pub two_way: bool,
}

const CARD_COLUMNS: &str =
    "c.id, c.image_url, c.audio_url, c.video_url, c.youtube_url, c.target_text, c.two_way";

// --- cards --------------------------------------------------------------

pub async fn list_cards(
    pool: &PgPool,
    tag: Option<&str>,
    deck_id: Option<Uuid>,
) -> sqlx::Result<Vec<CardRow>> {
    match (tag, deck_id) {
        (None, None) => {
            let sql = format!("SELECT {CARD_COLUMNS} FROM cards c ORDER BY c.created_at DESC");
            sqlx::query_as(sqlx::AssertSqlSafe(sql))
                .fetch_all(pool)
                .await
        }
        (Some(tag), None) => {
            let sql = format!(
                "SELECT {CARD_COLUMNS} FROM cards c \
                 JOIN card_tags t ON t.card_id = c.id AND t.tag = $1 \
                 ORDER BY c.created_at DESC"
            );
            sqlx::query_as(sqlx::AssertSqlSafe(sql))
                .bind(tag)
                .fetch_all(pool)
                .await
        }
        (None, Some(deck_id)) => {
            let sql = format!(
                "SELECT {CARD_COLUMNS} FROM cards c \
                 JOIN deck_cards dc ON dc.card_id = c.id AND dc.deck_id = $1 \
                 ORDER BY c.created_at DESC"
            );
            sqlx::query_as(sqlx::AssertSqlSafe(sql))
                .bind(deck_id)
                .fetch_all(pool)
                .await
        }
        (Some(tag), Some(deck_id)) => {
            let sql = format!(
                "SELECT {CARD_COLUMNS} FROM cards c \
                 JOIN card_tags t ON t.card_id = c.id AND t.tag = $1 \
                 JOIN deck_cards dc ON dc.card_id = c.id AND dc.deck_id = $2 \
                 ORDER BY c.created_at DESC"
            );
            sqlx::query_as(sqlx::AssertSqlSafe(sql))
                .bind(tag)
                .bind(deck_id)
                .fetch_all(pool)
                .await
        }
    }
}

pub async fn list_tags(pool: &PgPool) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("SELECT DISTINCT tag FROM card_tags ORDER BY tag")
        .fetch_all(pool)
        .await
}

pub async fn get_card(pool: &PgPool, card_id: Uuid) -> sqlx::Result<Option<CardRow>> {
    let sql = format!("SELECT {CARD_COLUMNS} FROM cards c WHERE c.id = $1");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(card_id)
        .fetch_optional(pool)
        .await
}

pub async fn card_tags(pool: &PgPool, card_id: Uuid) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("SELECT tag FROM card_tags WHERE card_id = $1 ORDER BY tag")
        .bind(card_id)
        .fetch_all(pool)
        .await
}

/// The deck every new card lands in automatically. Found by name rather
/// than a fixed id: simpler than seeding one in a migration (which would
/// need a `created_by` user that doesn't exist yet at migration time), and
/// self-healing if someone ever deletes it — the next card creation just
/// makes a fresh one.
async fn ensure_default_deck_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    created_by: Uuid,
) -> sqlx::Result<Uuid> {
    if let Some(id) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM decks WHERE name = 'Default'")
        .fetch_optional(&mut **tx)
        .await?
    {
        return Ok(id);
    }
    sqlx::query_scalar("INSERT INTO decks (name, created_by) VALUES ('Default', $1) RETURNING id")
        .bind(created_by)
        .fetch_one(&mut **tx)
        .await
}

/// `""` means "no media" at the API boundary (matching the pre-existing
/// convention for `image_url` that `audio_url`/`video_url` now follow too), but
/// must become SQL NULL on the way in — otherwise `Option<String>` reads
/// back as `Some("")`, not `None`, and the view's `.map()` on it renders
/// an empty, controls-only `<audio>`/`<video>` player for a card that was
/// never given one.
fn non_empty(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

pub async fn create_card(
    pool: &PgPool,
    created_by: Uuid,
    card: NewCard<'_>,
    tags: &[String],
) -> sqlx::Result<Uuid> {
    let mut tx = pool.begin().await?;

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO cards (card_type, image_url, audio_url, video_url, youtube_url, target_text, two_way, created_by) \
         VALUES ('image_to_target', $1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(non_empty(card.image_url))
    .bind(non_empty(card.audio_url))
    .bind(non_empty(card.video_url))
    .bind(non_empty(card.youtube_url))
    .bind(card.target_text)
    .bind(card.two_way)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await?;

    set_card_tags_in_tx(&mut tx, id, tags).await?;

    let default_deck_id = ensure_default_deck_in_tx(&mut tx, created_by).await?;
    sqlx::query("INSERT INTO deck_cards (deck_id, card_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(default_deck_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(id)
}

pub async fn update_card(
    pool: &PgPool,
    card_id: Uuid,
    card: NewCard<'_>,
    tags: &[String],
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "UPDATE cards SET image_url = $1, audio_url = $2, video_url = $3, youtube_url = $4, \
         target_text = $5, two_way = $6 WHERE id = $7",
    )
    .bind(non_empty(card.image_url))
    .bind(non_empty(card.audio_url))
    .bind(non_empty(card.video_url))
    .bind(non_empty(card.youtube_url))
    .bind(card.target_text)
    .bind(card.two_way)
    .bind(card_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM card_tags WHERE card_id = $1")
        .bind(card_id)
        .execute(&mut *tx)
        .await?;
    set_card_tags_in_tx(&mut tx, card_id, tags).await?;

    tx.commit().await?;
    Ok(())
}

async fn set_card_tags_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    card_id: Uuid,
    tags: &[String],
) -> sqlx::Result<()> {
    for tag in tags {
        sqlx::query("INSERT INTO card_tags (card_id, tag) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(card_id)
            .bind(tag)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

pub async fn delete_card(pool: &PgPool, card_id: Uuid) -> sqlx::Result<()> {
    // card_tags, deck_cards, and review_state all cascade on cards.id.
    sqlx::query("DELETE FROM cards WHERE id = $1")
        .bind(card_id)
        .execute(pool)
        .await?;
    Ok(())
}

// --- decks ----------------------------------------------------------------

pub async fn create_deck(pool: &PgPool, created_by: Uuid, name: &str) -> sqlx::Result<Uuid> {
    sqlx::query_scalar("INSERT INTO decks (name, created_by) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(created_by)
        .fetch_one(pool)
        .await
}

pub async fn list_decks(pool: &PgPool) -> sqlx::Result<Vec<DeckRow>> {
    sqlx::query_as(
        "SELECT d.id, d.name, \
         (SELECT c.target_text FROM deck_cards dc JOIN cards c ON c.id = dc.card_id \
          WHERE dc.deck_id = d.id ORDER BY c.created_at LIMIT 1) AS sample_word \
         FROM decks d ORDER BY d.name",
    )
    .fetch_all(pool)
    .await
}

pub async fn rename_deck(pool: &PgPool, deck_id: Uuid, name: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE decks SET name = $1 WHERE id = $2")
        .bind(name)
        .bind(deck_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_deck(pool: &PgPool, deck_id: Uuid) -> sqlx::Result<()> {
    // deck_cards cascades on decks.id.
    sqlx::query("DELETE FROM decks WHERE id = $1")
        .bind(deck_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn add_card_to_deck(pool: &PgPool, deck_id: Uuid, card_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO deck_cards (deck_id, card_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(deck_id)
        .bind(card_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_card_from_deck(
    pool: &PgPool,
    deck_id: Uuid,
    card_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM deck_cards WHERE deck_id = $1 AND card_id = $2")
        .bind(deck_id)
        .bind(card_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn card_deck_ids(pool: &PgPool, card_id: Uuid) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar("SELECT deck_id FROM deck_cards WHERE card_id = $1")
        .bind(card_id)
        .fetch_all(pool)
        .await
}

// --- review state / scheduling ---------------------------------------------

type ReviewStateRow = (Option<f32>, Option<f32>, f32, i32, i32);
type ReviewStateWithLastReviewedRow = (
    Option<f32>,
    Option<f32>,
    f32,
    i32,
    i32,
    Option<OffsetDateTime>,
);

pub async fn get_review_state(
    pool: &PgPool,
    user_id: Uuid,
    card_id: Uuid,
    direction: Direction,
) -> sqlx::Result<ReviewState> {
    let row: Option<ReviewStateRow> = sqlx::query_as(
        "SELECT stability, difficulty, interval_days, repetitions, lapses FROM review_state \
         WHERE user_id = $1 AND card_id = $2 AND direction = $3",
    )
    .bind(user_id)
    .bind(card_id)
    .bind(direction.as_str())
    .fetch_optional(pool)
    .await?;

    Ok(match row {
        Some((stability, difficulty, interval_days, repetitions, lapses)) => ReviewState {
            stability,
            difficulty,
            interval_days,
            repetitions: repetitions.cast_unsigned(),
            lapses: lapses.cast_unsigned(),
        },
        None => ReviewState::default(),
    })
}

/// Schedules `card_id`'s next review (in `direction`) for `user_id` and
/// logs the grading event, atomically. Returns the resulting state (useful
/// for tests and for any caller that wants to show the user what just
/// happened).
pub async fn record_review(
    pool: &PgPool,
    user_id: Uuid,
    card_id: Uuid,
    grade: Grade,
    direction: Direction,
) -> sqlx::Result<ReviewState> {
    // Fetched alongside last_reviewed_at (rather than via get_review_state)
    // because FSRS needs actual elapsed time since the last review, not
    // just the stored state — a card reviewed late schedules differently
    // than one reviewed exactly on time.
    let row: Option<ReviewStateWithLastReviewedRow> = sqlx::query_as(
        "SELECT stability, difficulty, interval_days, repetitions, lapses, last_reviewed_at \
             FROM review_state WHERE user_id = $1 AND card_id = $2 AND direction = $3",
    )
    .bind(user_id)
    .bind(card_id)
    .bind(direction.as_str())
    .fetch_optional(pool)
    .await?;

    let (current, last_reviewed_at) = match row {
        Some((stability, difficulty, interval_days, repetitions, lapses, last_reviewed_at)) => (
            ReviewState {
                stability,
                difficulty,
                interval_days,
                repetitions: repetitions.cast_unsigned(),
                lapses: lapses.cast_unsigned(),
            },
            last_reviewed_at,
        ),
        None => (ReviewState::default(), None),
    };

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let elapsed_days = last_reviewed_at.map_or(0, |t| {
        (OffsetDateTime::now_utc() - t).whole_days().max(0) as u32
    });

    let next = current.schedule(grade, elapsed_days);

    // Truncating fractional seconds is intentional, not accidental loss.
    #[allow(clippy::cast_possible_truncation)]
    let due_at =
        OffsetDateTime::now_utc() + time::Duration::seconds((next.interval_days * 86_400.0) as i64);

    let mut tx = pool.begin().await?;

    // introduced_at is only ever set by the VALUES on first insert; the
    // ON CONFLICT branch deliberately omits it, so it's never overwritten.
    sqlx::query(
        "INSERT INTO review_state \
            (user_id, card_id, direction, stability, difficulty, interval_days, repetitions, lapses, due_at, last_reviewed_at, introduced_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now()) \
         ON CONFLICT (user_id, card_id, direction) DO UPDATE SET \
            stability = EXCLUDED.stability, \
            difficulty = EXCLUDED.difficulty, \
            interval_days = EXCLUDED.interval_days, \
            repetitions = EXCLUDED.repetitions, \
            lapses = EXCLUDED.lapses, \
            due_at = EXCLUDED.due_at, \
            last_reviewed_at = EXCLUDED.last_reviewed_at",
    )
    .bind(user_id)
    .bind(card_id)
    .bind(direction.as_str())
    .bind(next.stability)
    .bind(next.difficulty)
    .bind(next.interval_days)
    .bind(next.repetitions.cast_signed())
    .bind(next.lapses.cast_signed())
    .bind(due_at)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO review_log (user_id, card_id, direction, grade) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(card_id)
    .bind(direction.as_str())
    .bind(grade_label(grade))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(next)
}

fn grade_label(grade: Grade) -> &'static str {
    match grade {
        Grade::Again => "again",
        Grade::Hard => "hard",
        Grade::Good => "good",
        Grade::Easy => "easy",
    }
}

async fn due_review_cards(
    pool: &PgPool,
    user_id: Uuid,
    deck_id: Option<Uuid>,
    order: ReviewOrder,
) -> sqlx::Result<Vec<ReviewQueueCard>> {
    // `order_clause` is one of three fixed literals selected by a closed
    // Rust enum, never user input, so interpolating it isn't a SQL
    // injection risk the way interpolating a value would be.
    let order_clause = match order {
        ReviewOrder::DueDate => "rs.due_at ASC",
        ReviewOrder::Random => "random()",
        // Higher FSRS difficulty means a harder card (unlike the old SM-2
        // ease_factor, where lower meant harder), so this sorts descending.
        ReviewOrder::HardestFirst => "rs.difficulty DESC",
    };

    let rows: Vec<ReviewQueueCardRow> = if let Some(deck_id) = deck_id {
        let sql = format!(
            "SELECT c.id, c.image_url, c.audio_url, c.video_url, c.youtube_url, c.target_text, rs.direction \
             FROM cards c \
             JOIN review_state rs ON rs.card_id = c.id AND rs.user_id = $1 \
             JOIN deck_cards dc ON dc.card_id = c.id AND dc.deck_id = $2 \
             WHERE rs.due_at <= now() ORDER BY {order_clause}"
        );
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(user_id)
            .bind(deck_id)
            .fetch_all(pool)
            .await?
    } else {
        let sql = format!(
            "SELECT c.id, c.image_url, c.audio_url, c.video_url, c.youtube_url, c.target_text, rs.direction \
             FROM cards c \
             JOIN review_state rs ON rs.card_id = c.id AND rs.user_id = $1 \
             WHERE rs.due_at <= now() ORDER BY {order_clause}"
        );
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(user_id)
            .fetch_all(pool)
            .await?
    };
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Candidates never reviewed in a given direction: the existing
/// `media→target_text` direction for every card, plus (for two-way cards
/// only) the additional `target_text→media` direction. Each direction is
/// independently "new" or not, so a two-way card can offer up to two new
/// slots.
async fn new_cards(
    pool: &PgPool,
    user_id: Uuid,
    deck_id: Option<Uuid>,
    limit: i64,
) -> sqlx::Result<Vec<ReviewQueueCard>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows: Vec<ReviewQueueCardRow> = if let Some(deck_id) = deck_id {
        sqlx::query_as(
            "SELECT id, image_url, audio_url, video_url, youtube_url, target_text, direction FROM ( \
                 SELECT c.id, c.image_url, c.audio_url, c.video_url, c.youtube_url, c.target_text, \
                        'forward' AS direction, c.created_at AS sort_key \
                 FROM cards c \
                 JOIN deck_cards dc ON dc.card_id = c.id AND dc.deck_id = $3 \
                 LEFT JOIN review_state rs ON rs.card_id = c.id AND rs.user_id = $1 AND rs.direction = 'forward' \
                 WHERE rs.card_id IS NULL \
                 UNION ALL \
                 SELECT c.id, c.image_url, c.audio_url, c.video_url, c.youtube_url, c.target_text, \
                        'reverse' AS direction, c.created_at AS sort_key \
                 FROM cards c \
                 JOIN deck_cards dc ON dc.card_id = c.id AND dc.deck_id = $3 \
                 LEFT JOIN review_state rs ON rs.card_id = c.id AND rs.user_id = $1 AND rs.direction = 'reverse' \
                 WHERE c.two_way AND rs.card_id IS NULL \
             ) combined ORDER BY sort_key ASC LIMIT $2",
        )
        .bind(user_id)
        .bind(limit)
        .bind(deck_id)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, image_url, audio_url, video_url, youtube_url, target_text, direction FROM ( \
                 SELECT c.id, c.image_url, c.audio_url, c.video_url, c.youtube_url, c.target_text, \
                        'forward' AS direction, c.created_at AS sort_key \
                 FROM cards c \
                 LEFT JOIN review_state rs ON rs.card_id = c.id AND rs.user_id = $1 AND rs.direction = 'forward' \
                 WHERE rs.card_id IS NULL \
                 UNION ALL \
                 SELECT c.id, c.image_url, c.audio_url, c.video_url, c.youtube_url, c.target_text, \
                        'reverse' AS direction, c.created_at AS sort_key \
                 FROM cards c \
                 LEFT JOIN review_state rs ON rs.card_id = c.id AND rs.user_id = $1 AND rs.direction = 'reverse' \
                 WHERE c.two_way AND rs.card_id IS NULL \
             ) combined ORDER BY sort_key ASC LIMIT $2",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    };
    Ok(rows.into_iter().map(Into::into).collect())
}

async fn new_cards_introduced_today(pool: &PgPool, user_id: Uuid) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM review_state \
         WHERE user_id = $1 AND introduced_at::date = current_date",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

pub async fn daily_new_card_limit(pool: &PgPool, user_id: Uuid) -> sqlx::Result<Option<i32>> {
    sqlx::query_scalar("SELECT daily_new_card_limit FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
}

pub async fn set_daily_new_card_limit(
    pool: &PgPool,
    user_id: Uuid,
    limit: Option<i32>,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET daily_new_card_limit = $1 WHERE id = $2")
        .bind(limit)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The cards due for review right now, plus up to the user's remaining
/// daily allowance of never-before-seen cards. Review cards (already in
/// the user's rotation) are never capped — only the introduction of
/// brand-new ones is.
pub async fn review_queue(
    pool: &PgPool,
    user_id: Uuid,
    deck_id: Option<Uuid>,
    order: ReviewOrder,
) -> sqlx::Result<ReviewQueue> {
    let review_cards = due_review_cards(pool, user_id, deck_id, order).await?;
    let limit = daily_new_card_limit(pool, user_id).await?;
    let introduced_today = new_cards_introduced_today(pool, user_id).await?;

    let remaining_new = match limit {
        Some(limit) => (i64::from(limit) - introduced_today).max(0),
        None => 1000, // sane hard cap so an "unlimited" setting can't blow up one query
    };
    let new = new_cards(pool, user_id, deck_id, remaining_new).await?;

    #[allow(clippy::cast_possible_wrap)] // card counts are nowhere near i64::MAX
    let due_review_count = review_cards.len() as i64;
    #[allow(clippy::cast_possible_wrap)]
    let new_available_today = new.len() as i64;

    let mut cards = review_cards;
    cards.extend(new);

    Ok(ReviewQueue {
        cards,
        due_review_count,
        new_available_today,
    })
}

// --- stats ------------------------------------------------------------------

pub async fn user_stats(pool: &PgPool, user_id: Uuid) -> sqlx::Result<UserStats> {
    let total_cards: i64 = sqlx::query_scalar("SELECT count(*) FROM cards")
        .fetch_one(pool)
        .await?;
    let due_now: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM review_state WHERE user_id = $1 AND due_at <= now()",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    let reviews_today: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM review_log WHERE user_id = $1 AND graded_at::date = current_date",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    let current_streak_days = streak_days(pool, user_id).await?;
    let hardest_cards = sqlx::query_as(
        "SELECT c.id, c.target_text, rs.difficulty, rs.lapses FROM review_state rs \
         JOIN cards c ON c.id = rs.card_id \
         WHERE rs.user_id = $1 AND rs.lapses > 0 \
         ORDER BY rs.lapses DESC, rs.difficulty DESC LIMIT 10",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(UserStats {
        total_cards,
        due_now,
        reviews_today,
        current_streak_days,
        hardest_cards,
    })
}

/// Consecutive days (ending today) with at least one review logged. Zero
/// if nothing's been reviewed yet today, even with a long run before that
/// — simple to reason about, at the cost of not giving credit for "still
/// have time left today".
async fn streak_days(pool: &PgPool, user_id: Uuid) -> sqlx::Result<i64> {
    let days: Vec<time::Date> = sqlx::query_scalar(
        "SELECT DISTINCT graded_at::date FROM review_log \
         WHERE user_id = $1 ORDER BY graded_at::date DESC LIMIT 365",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut streak: i64 = 0;
    let mut expected = OffsetDateTime::now_utc().date();
    for day in days {
        if day == expected {
            streak += 1;
            expected = expected.previous_day().unwrap_or(expected);
        } else {
            break;
        }
    }
    Ok(streak)
}

// --- dictionary (words) -----------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WordRow {
    pub id: Uuid,
    pub target_text: String,
    pub part_of_speech: Option<String>,
    pub notes: Option<String>,
    pub english_translation: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WordExampleRow {
    pub target_sentence: String,
    pub gloss_sentence: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkedWordRow {
    pub target_text: String,
    pub link_type: String,
}

#[derive(Debug, Clone)]
pub struct WordDefinitionRow {
    pub target_text: String,
    pub part_of_speech: Option<String>,
    pub notes: Option<String>,
    pub english_translation: Option<String>,
    pub examples: Vec<WordExampleRow>,
    pub linked_words: Vec<LinkedWordRow>,
}

async fn get_word_by_text(pool: &PgPool, target_text: &str) -> sqlx::Result<Option<WordRow>> {
    sqlx::query_as(
        "SELECT id, target_text, part_of_speech, notes, english_translation \
         FROM words WHERE target_text = $1",
    )
    .bind(target_text)
    .fetch_optional(pool)
    .await
}

async fn word_examples(pool: &PgPool, word_id: Uuid) -> sqlx::Result<Vec<WordExampleRow>> {
    sqlx::query_as(
        "SELECT target_sentence, gloss_sentence FROM word_examples \
         WHERE word_id = $1 ORDER BY created_at",
    )
    .bind(word_id)
    .fetch_all(pool)
    .await
}

async fn linked_words(pool: &PgPool, word_id: Uuid) -> sqlx::Result<Vec<LinkedWordRow>> {
    sqlx::query_as(
        "SELECT w.target_text, wl.link_type FROM word_links wl \
         JOIN words w ON w.id = CASE WHEN wl.word_id = $1 THEN wl.linked_word_id ELSE wl.word_id END \
         WHERE wl.word_id = $1 OR wl.linked_word_id = $1 \
         ORDER BY w.target_text",
    )
    .bind(word_id)
    .fetch_all(pool)
    .await
}

/// Looks up a word's dictionary entry by its exact (trimmed) spelling.
/// `Ok(None)` means there's no entry yet — a normal, expected outcome for
/// most words, not an error condition.
pub async fn get_word_definition(
    pool: &PgPool,
    target_text: &str,
) -> sqlx::Result<Option<WordDefinitionRow>> {
    let Some(word) = get_word_by_text(pool, target_text).await? else {
        return Ok(None);
    };
    let examples = word_examples(pool, word.id).await?;
    let linked_words = linked_words(pool, word.id).await?;
    Ok(Some(WordDefinitionRow {
        target_text: word.target_text,
        part_of_speech: word.part_of_speech,
        notes: word.notes,
        english_translation: word.english_translation,
        examples,
        linked_words,
    }))
}

/// Finds or creates a bare `words` row for `target_text` — used only to get
/// a satisfiable `word_id` for a `word_links` row when a linked word doesn't
/// have its own dictionary entry yet. The `DO UPDATE` is a deliberate no-op,
/// present purely so `RETURNING id` still fires on conflict — this must
/// never touch an existing entry's `part_of_speech`/`notes` the way the real
/// upsert below does.
async fn resolve_or_create_word_id_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    target_text: &str,
    created_by: Uuid,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO words (target_text, created_by) VALUES ($1, $2) \
         ON CONFLICT (target_text) DO UPDATE SET target_text = EXCLUDED.target_text \
         RETURNING id",
    )
    .bind(target_text)
    .bind(created_by)
    .fetch_one(&mut **tx)
    .await
}

/// The headword-level fields `upsert_word_definition` writes, grouped so
/// adding one more (as `english_translation` just did) doesn't trip
/// clippy's `too_many_arguments` on the function itself — these three are
/// already a natural unit (the word's own optional metadata), distinct
/// from the child-table data (`examples`/`linked_word_texts`) and the
/// identity/context args (`target_text`/`created_by`) around them.
#[derive(Default)]
pub struct WordFields<'a> {
    pub part_of_speech: Option<&'a str>,
    pub notes: Option<&'a str>,
    pub english_translation: Option<&'a str>,
}

/// Creates or updates `target_text`'s dictionary entry, replacing its
/// examples and linked words wholesale — the same delete-then-reinsert idiom
/// `update_card`/`set_card_tags_in_tx` already use for tags. `created_by` is
/// only ever applied on first insert (the `ON CONFLICT` branch never
/// touches it). The single write path for the card form's "Dictionary
/// details" section.
pub async fn upsert_word_definition(
    pool: &PgPool,
    target_text: &str,
    created_by: Uuid,
    fields: WordFields<'_>,
    examples: &[(String, Option<String>)],
    linked_word_texts: &[String],
) -> sqlx::Result<Uuid> {
    let mut tx = pool.begin().await?;

    let word_id: Uuid = sqlx::query_scalar(
        "INSERT INTO words (target_text, part_of_speech, notes, english_translation, created_by) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (target_text) DO UPDATE SET \
            part_of_speech = EXCLUDED.part_of_speech, \
            notes = EXCLUDED.notes, \
            english_translation = EXCLUDED.english_translation, \
            updated_at = now() \
         RETURNING id",
    )
    .bind(target_text)
    .bind(fields.part_of_speech)
    .bind(fields.notes)
    .bind(fields.english_translation)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM word_examples WHERE word_id = $1")
        .bind(word_id)
        .execute(&mut *tx)
        .await?;
    for (target_sentence, gloss_sentence) in examples {
        sqlx::query(
            "INSERT INTO word_examples (word_id, target_sentence, gloss_sentence) \
             VALUES ($1, $2, $3)",
        )
        .bind(word_id)
        .bind(target_sentence)
        .bind(gloss_sentence.as_deref())
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("DELETE FROM word_links WHERE word_id = $1 OR linked_word_id = $1")
        .bind(word_id)
        .execute(&mut *tx)
        .await?;
    for linked_text in linked_word_texts {
        if linked_text == target_text {
            continue; // a word can't be related to itself
        }
        let linked_id = resolve_or_create_word_id_in_tx(&mut tx, linked_text, created_by).await?;
        sqlx::query(
            "INSERT INTO word_links (word_id, linked_word_id, link_type) \
             VALUES ($1, $2, 'related') ON CONFLICT DO NOTHING",
        )
        .bind(word_id)
        .bind(linked_id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(word_id)
}

// --- wiki pages -------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PageRow {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub slug: String,
    pub title: String,
    pub position: i32,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PageContentRow {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub slug: String,
    pub title: String,
    pub content_json: String,
    pub content_markdown: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PageRevisionRow {
    pub id: Uuid,
    pub author_id: Uuid,
    pub title: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PageRevisionContentRow {
    pub title: String,
    pub content_json: String,
    pub content_markdown: String,
}

/// Whole tree in one query — no content, just enough for the sidebar.
/// `parent_id NULLS FIRST` groups every root page together ahead of any
/// children, `position` orders siblings (including the NULL-parent group)
/// within that.
pub async fn list_page_tree(pool: &PgPool) -> sqlx::Result<Vec<PageRow>> {
    sqlx::query_as(
        "SELECT id, parent_id, slug, title, position FROM pages \
         ORDER BY parent_id NULLS FIRST, position",
    )
    .fetch_all(pool)
    .await
}

pub async fn get_page(pool: &PgPool, page_id: Uuid) -> sqlx::Result<Option<PageContentRow>> {
    sqlx::query_as(
        "SELECT p.id, p.parent_id, p.slug, p.title, \
                r.content_json::text AS content_json, r.content_markdown \
         FROM pages p JOIN page_revisions r ON r.id = p.current_revision_id \
         WHERE p.id = $1",
    )
    .bind(page_id)
    .fetch_optional(pool)
    .await
}

/// Inserts the page and its first (empty) revision together, then points
/// the page at it — the two tables reference each other, so this can't be
/// done as a single INSERT; see the migration's comment on
/// `current_revision_id`.
pub async fn create_page(
    pool: &PgPool,
    created_by: Uuid,
    parent_id: Option<Uuid>,
    title: &str,
    slug: &str,
) -> sqlx::Result<Uuid> {
    let mut tx = pool.begin().await?;
    // IS NOT DISTINCT FROM, not `=`: a NULL parent_id (root-level pages)
    // would never match plain `= $1` under SQL's three-valued logic.
    let next_position: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM pages \
         WHERE parent_id IS NOT DISTINCT FROM $1",
    )
    .bind(parent_id)
    .fetch_one(&mut *tx)
    .await?;
    let page_id: Uuid = sqlx::query_scalar(
        "INSERT INTO pages (parent_id, slug, title, position, created_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(parent_id)
    .bind(slug)
    .bind(title)
    .bind(next_position)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await?;
    let revision_id: Uuid = sqlx::query_scalar(
        "INSERT INTO page_revisions (page_id, author_id, title, content_json, content_markdown) \
         VALUES ($1, $2, $3, '{}'::jsonb, '') RETURNING id",
    )
    .bind(page_id)
    .bind(created_by)
    .bind(title)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE pages SET current_revision_id = $1, search_text = $2 WHERE id = $3")
        .bind(revision_id)
        .bind(title)
        .bind(page_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(page_id)
}

/// Always a new revision, never an edit in place — `pages.title` moves to
/// match (it's the fast/denormalized "current" copy the tree listing reads,
/// see `list_page_tree`), but every past title stays on its own revision row.
pub async fn save_page_revision(
    pool: &PgPool,
    page_id: Uuid,
    author_id: Uuid,
    title: &str,
    content_json: &str,
    content_markdown: &str,
) -> sqlx::Result<Uuid> {
    let mut tx = pool.begin().await?;
    let revision_id: Uuid = sqlx::query_scalar(
        "INSERT INTO page_revisions (page_id, author_id, title, content_json, content_markdown) \
         VALUES ($1, $2, $3, $4::jsonb, $5) RETURNING id",
    )
    .bind(page_id)
    .bind(author_id)
    .bind(title)
    .bind(content_json)
    .bind(content_markdown)
    .fetch_one(&mut *tx)
    .await?;
    // search_text is just title+markdown concatenated, not a tsvector
    // itself — pages_search_idx builds that from it with to_tsvector at
    // query/index time, so this stays a plain string with no FTS-specific
    // handling here.
    let search_text = format!("{title} {content_markdown}");
    sqlx::query(
        "UPDATE pages SET title = $1, current_revision_id = $2, search_text = $3 WHERE id = $4",
    )
    .bind(title)
    .bind(revision_id)
    .bind(&search_text)
    .bind(page_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(revision_id)
}

pub async fn rename_page(
    pool: &PgPool,
    page_id: Uuid,
    title: &str,
    slug: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE pages SET title = $1, slug = $2 WHERE id = $3")
        .bind(title)
        .bind(slug)
        .bind(page_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn move_page(
    pool: &PgPool,
    page_id: Uuid,
    new_parent_id: Option<Uuid>,
    new_position: i32,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE pages SET parent_id = $1, position = $2 WHERE id = $3")
        .bind(new_parent_id)
        .bind(new_position)
        .bind(page_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Reparents a page to the end of its new siblings — the same
/// MAX(position)+1-in-a-transaction pattern `create_page` uses, so a moved
/// page doesn't collide with or shuffle the position of pages already in
/// the target parent.
pub async fn move_page_to_end(
    pool: &PgPool,
    page_id: Uuid,
    new_parent_id: Option<Uuid>,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    let next_position: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM pages \
         WHERE parent_id IS NOT DISTINCT FROM $1",
    )
    .bind(new_parent_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE pages SET parent_id = $1, position = $2 WHERE id = $3")
        .bind(new_parent_id)
        .bind(next_position)
        .bind(page_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn page_has_children(pool: &PgPool, page_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pages WHERE parent_id = $1)")
        .bind(page_id)
        .fetch_one(pool)
        .await
}

pub async fn delete_page(pool: &PgPool, page_id: Uuid) -> sqlx::Result<()> {
    // Callers must check page_has_children first — a page with children
    // isn't deletable (see wiki.rs's delete_page server fn). pages.id is
    // referenced by current_revision_id's own FK target table, not the
    // other way round, so no self-referential cleanup is needed here
    // beyond what ON DELETE CASCADE already does for page_revisions/page_tags.
    sqlx::query("DELETE FROM pages WHERE id = $1")
        .bind(page_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_page_revisions(
    pool: &PgPool,
    page_id: Uuid,
) -> sqlx::Result<Vec<PageRevisionRow>> {
    sqlx::query_as(
        "SELECT id, author_id, title, created_at FROM page_revisions \
         WHERE page_id = $1 ORDER BY created_at DESC",
    )
    .bind(page_id)
    .fetch_all(pool)
    .await
}

pub async fn get_revision(
    pool: &PgPool,
    revision_id: Uuid,
) -> sqlx::Result<Option<PageRevisionContentRow>> {
    sqlx::query_as(
        "SELECT title, content_json::text, content_markdown \
         FROM page_revisions WHERE id = $1",
    )
    .bind(revision_id)
    .fetch_optional(pool)
    .await
}

/// `content_markdown` of the revision immediately before `revision_id`, for
/// diffing — `None` if `revision_id` is the page's first revision.
pub async fn previous_revision_markdown(
    pool: &PgPool,
    page_id: Uuid,
    revision_id: Uuid,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT content_markdown FROM page_revisions \
         WHERE page_id = $1 AND created_at < (SELECT created_at FROM page_revisions WHERE id = $2) \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(page_id)
    .bind(revision_id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PageSearchRow {
    pub id: Uuid,
    pub slug: String,
    pub title: String,
    /// A highlighted snippet around the match, built by `ts_headline` —
    /// comes back with `<b>...</b>` already wrapping the matched terms.
    pub snippet: String,
}

pub async fn search_pages(pool: &PgPool, query: &str) -> sqlx::Result<Vec<PageSearchRow>> {
    sqlx::query_as(
        "SELECT id, slug, title, \
                ts_headline('english', search_text, plainto_tsquery('english', $1)) AS snippet \
         FROM pages \
         WHERE to_tsvector('english', search_text) @@ plainto_tsquery('english', $1) \
         ORDER BY ts_rank(to_tsvector('english', search_text), plainto_tsquery('english', $1)) DESC \
         LIMIT 20",
    )
    .bind(query)
    .fetch_all(pool)
    .await
}

// --- card <-> page links ------------------------------------------------
// Mirrors deck_cards/add_card_to_deck exactly: a plain many-to-many join,
// add/remove by pair rather than a replace-the-whole-set upsert.

pub async fn add_page_to_card(pool: &PgPool, card_id: Uuid, page_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO card_pages (card_id, page_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(card_id)
        .bind(page_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_page_from_card(
    pool: &PgPool,
    card_id: Uuid,
    page_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM card_pages WHERE card_id = $1 AND page_id = $2")
        .bind(card_id)
        .bind(page_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn card_page_ids(pool: &PgPool, card_id: Uuid) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar("SELECT page_id FROM card_pages WHERE card_id = $1")
        .bind(card_id)
        .fetch_all(pool)
        .await
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkedCardRow {
    pub id: Uuid,
    pub target_text: String,
}

/// The cards that cite `page_id` — the wiki page's side of the link,
/// rendered as a "Cards using this" backlink list so the connection is
/// visible from the page, not just from the card's edit form.
pub async fn cards_for_page(pool: &PgPool, page_id: Uuid) -> sqlx::Result<Vec<LinkedCardRow>> {
    sqlx::query_as(
        "SELECT c.id, c.target_text FROM cards c \
         JOIN card_pages cp ON cp.card_id = c.id \
         WHERE cp.page_id = $1 \
         ORDER BY c.target_text",
    )
    .bind(page_id)
    .fetch_all(pool)
    .await
}

// --- card <-> card typed links -------------------------------------------
// Unlike deck_cards/card_pages, storage order is never canonicalized by
// sorting the two UUIDs: back_chain_step is directional (card_id = earlier
// step, linked_card_id = later step), and canonicalizing would destroy
// that order. A reverse-order duplicate of the same (pair, link_type) is
// blocked instead by the card_links_unordered_pair_idx expression unique
// index (see the migration), so a plain `ON CONFLICT DO NOTHING` is
// race-free without an application-level existence check.

pub async fn add_card_link(
    pool: &PgPool,
    card_id: Uuid,
    linked_card_id: Uuid,
    link_type: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO card_links (card_id, linked_card_id, link_type) \
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(card_id)
    .bind(linked_card_id)
    .bind(link_type)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_card_link(
    pool: &PgPool,
    card_id: Uuid,
    linked_card_id: Uuid,
    link_type: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM card_links WHERE link_type = $3 \
         AND ((card_id = $1 AND linked_card_id = $2) OR (card_id = $2 AND linked_card_id = $1))",
    )
    .bind(card_id)
    .bind(linked_card_id)
    .bind(link_type)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CardLinkRow {
    pub linked_card_id: Uuid,
    pub target_text: String,
    pub link_type: String,
    /// True when the queried card was stored as `card_id` rather than
    /// `linked_card_id` — i.e. it's the earlier step for a directional
    /// type like back_chain_step. Meaningless but harmless for the
    /// symmetric types (minimal_pair, sound_spelling).
    pub is_source: bool,
}

pub async fn linked_cards(pool: &PgPool, card_id: Uuid) -> sqlx::Result<Vec<CardLinkRow>> {
    sqlx::query_as(
        "SELECT CASE WHEN cl.card_id = $1 THEN cl.linked_card_id ELSE cl.card_id END AS linked_card_id, \
                c.target_text, cl.link_type, (cl.card_id = $1) AS is_source \
         FROM card_links cl \
         JOIN cards c ON c.id = (CASE WHEN cl.card_id = $1 THEN cl.linked_card_id ELSE cl.card_id END) \
         WHERE cl.card_id = $1 OR cl.linked_card_id = $1 \
         ORDER BY cl.link_type, c.target_text",
    )
    .bind(card_id)
    .fetch_all(pool)
    .await
}

/// Cards a link-picker can search by text — reuses `CardRow`/`CARD_COLUMNS`
/// rather than a bare `{id, target_text}` projection: `cards.target_text`
/// has no uniqueness constraint (unlike `words.target_text`), so two
/// different cards can share the same text, and the picker needs the media
/// fields too to tell them apart.
pub async fn search_cards(
    pool: &PgPool,
    query: &str,
    exclude_card_id: Uuid,
) -> sqlx::Result<Vec<CardRow>> {
    let sql = format!(
        "SELECT {CARD_COLUMNS} FROM cards c \
         WHERE c.target_text ILIKE $1 AND c.id != $2 \
         ORDER BY c.target_text LIMIT 10"
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(format!("%{query}%"))
        .bind(exclude_card_id)
        .fetch_all(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_user(pool: &PgPool) -> Uuid {
        sqlx::query_scalar("INSERT INTO users (email, name) VALUES ($1, $2) RETURNING id")
            .bind("db-test@example.com")
            .bind("db-test")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn card_with_text(target_text: &str) -> NewCard<'_> {
        NewCard {
            target_text,
            ..Default::default()
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn created_card_appears_in_list_with_its_tags(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(
            &pool,
            user_id,
            NewCard {
                image_url: "https://example.com/cat.jpg",
                target_text: "gato",
                ..Default::default()
            },
            &["animals".to_string(), "beginner".to_string()],
        )
        .await
        .unwrap();

        let cards = list_cards(&pool, None, None).await.unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, card_id);
        assert_eq!(cards[0].target_text, "gato");

        let tags = card_tags(&pool, card_id).await.unwrap();
        assert_eq!(tags, vec!["animals".to_string(), "beginner".to_string()]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn list_cards_filters_by_tag(pool: PgPool) {
        let user_id = test_user(&pool).await;
        create_card(
            &pool,
            user_id,
            card_with_text("gato"),
            &["animals".to_string()],
        )
        .await
        .unwrap();
        create_card(
            &pool,
            user_id,
            card_with_text("trabajo"),
            &["work".to_string()],
        )
        .await
        .unwrap();

        let animals = list_cards(&pool, Some("animals"), None).await.unwrap();
        assert_eq!(animals.len(), 1);
        assert_eq!(animals[0].target_text, "gato");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_card_replaces_fields_and_tags(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(
            &pool,
            user_id,
            NewCard {
                image_url: "old.jpg",
                target_text: "old",
                ..Default::default()
            },
            &["old-tag".to_string()],
        )
        .await
        .unwrap();

        update_card(
            &pool,
            card_id,
            NewCard {
                image_url: "new.jpg",
                target_text: "new",
                ..Default::default()
            },
            &["new-tag".to_string()],
        )
        .await
        .unwrap();

        let card = get_card(&pool, card_id).await.unwrap().unwrap();
        assert_eq!(card.target_text, "new");
        assert_eq!(card.image_url, Some("new.jpg".to_string()));
        assert_eq!(card_tags(&pool, card_id).await.unwrap(), vec!["new-tag"]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_card_can_attach_audio_and_video_and_mark_two_way(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();

        update_card(
            &pool,
            card_id,
            NewCard {
                audio_url: "audio.mp3",
                video_url: "video.mp4",
                youtube_url: "https://www.youtube.com/embed/dQw4w9WgXcQ",
                target_text: "word",
                two_way: true,
                ..Default::default()
            },
            &[],
        )
        .await
        .unwrap();

        let card = get_card(&pool, card_id).await.unwrap().unwrap();
        assert_eq!(card.audio_url, Some("audio.mp3".to_string()));
        assert_eq!(card.video_url, Some("video.mp4".to_string()));
        assert_eq!(
            card.youtube_url,
            Some("https://www.youtube.com/embed/dQw4w9WgXcQ".to_string())
        );
        assert!(card.two_way);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_card_removes_it_from_the_list(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();

        delete_card(&pool, card_id).await.unwrap();

        assert!(get_card(&pool, card_id).await.unwrap().is_none());
        assert!(list_cards(&pool, None, None).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deck_scopes_card_list_and_review_queue(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let in_deck = create_card(&pool, user_id, card_with_text("in-deck"), &[])
            .await
            .unwrap();
        let _out_of_deck = create_card(&pool, user_id, card_with_text("out-of-deck"), &[])
            .await
            .unwrap();

        let deck_id = create_deck(&pool, user_id, "My Deck").await.unwrap();
        add_card_to_deck(&pool, deck_id, in_deck).await.unwrap();

        let cards = list_cards(&pool, None, Some(deck_id)).await.unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, in_deck);

        let queue = review_queue(&pool, user_id, Some(deck_id), ReviewOrder::DueDate)
            .await
            .unwrap();
        assert_eq!(queue.cards.len(), 1);
        assert_eq!(queue.cards[0].id, in_deck);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn removing_a_card_from_a_deck_excludes_it_again(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();
        let deck_id = create_deck(&pool, user_id, "Deck").await.unwrap();
        add_card_to_deck(&pool, deck_id, card_id).await.unwrap();

        remove_card_from_deck(&pool, deck_id, card_id)
            .await
            .unwrap();

        assert!(
            list_cards(&pool, None, Some(deck_id))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn creating_a_card_adds_it_to_the_default_deck(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();

        let decks = list_decks(&pool).await.unwrap();
        assert_eq!(decks.len(), 1);
        assert_eq!(decks[0].name, "Default");

        let cards = list_cards(&pool, None, Some(decks[0].id)).await.unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, card_id);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn two_cards_share_the_same_default_deck(pool: PgPool) {
        let user_id = test_user(&pool).await;
        create_card(&pool, user_id, card_with_text("one"), &[])
            .await
            .unwrap();
        create_card(&pool, user_id, card_with_text("two"), &[])
            .await
            .unwrap();

        // One Default deck, not a fresh one per card.
        assert_eq!(list_decks(&pool).await.unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn renaming_a_deck_changes_its_name(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let deck_id = create_deck(&pool, user_id, "Old Name").await.unwrap();

        rename_deck(&pool, deck_id, "New Name").await.unwrap();

        let decks = list_decks(&pool).await.unwrap();
        assert_eq!(decks.len(), 1);
        assert_eq!(decks[0].name, "New Name");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deleting_a_deck_removes_it_and_its_card_memberships(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();
        let deck_id = create_deck(&pool, user_id, "Temp").await.unwrap();
        add_card_to_deck(&pool, deck_id, card_id).await.unwrap();

        delete_deck(&pool, deck_id).await.unwrap();

        assert!(
            list_decks(&pool)
                .await
                .unwrap()
                .iter()
                .all(|d| d.id != deck_id)
        );
        // The card itself survives — only its membership in the deleted
        // deck is gone.
        assert!(get_card(&pool, card_id).await.unwrap().is_some());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_review_state_defaults_when_never_graded(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();

        let state = get_review_state(&pool, user_id, card_id, Direction::Forward)
            .await
            .unwrap();
        assert_eq!(state, ReviewState::default());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn record_review_schedules_and_logs_in_one_go(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();

        let next = record_review(&pool, user_id, card_id, Grade::Good, Direction::Forward)
            .await
            .unwrap();
        assert_eq!(next.repetitions, 1);
        assert!(next.interval_days >= 1.0);

        let logged: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM review_log WHERE user_id = $1 AND card_id = $2 AND grade = 'good'",
        )
        .bind(user_id)
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(logged, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn record_review_upserts_rather_than_duplicates_state(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();

        record_review(&pool, user_id, card_id, Grade::Good, Direction::Forward)
            .await
            .unwrap();
        record_review(&pool, user_id, card_id, Grade::Good, Direction::Forward)
            .await
            .unwrap();

        let row_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM review_state WHERE user_id = $1 AND card_id = $2",
        )
        .bind(user_id)
        .bind(card_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row_count, 1);

        let state = get_review_state(&pool, user_id, card_id, Direction::Forward)
            .await
            .unwrap();
        assert_eq!(state.repetitions, 2);
        // The exact FSRS-fitted interval is covered by crates/domain's
        // own tests; this test's job is only to confirm the upsert kept
        // one row and kept scheduling forward, not duplicated it.
        assert!(state.interval_days >= 1.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn two_way_cards_schedule_each_direction_independently(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(
            &pool,
            user_id,
            NewCard {
                target_text: "word",
                two_way: true,
                ..Default::default()
            },
            &[],
        )
        .await
        .unwrap();

        // Before any review, a two-way card offers two independent "new"
        // slots — one per direction — not one.
        let before = review_queue(&pool, user_id, None, ReviewOrder::DueDate)
            .await
            .unwrap();
        assert_eq!(before.new_available_today, 2);

        record_review(&pool, user_id, card_id, Grade::Good, Direction::Forward)
            .await
            .unwrap();

        // Grading the forward direction doesn't touch the reverse one:
        // it's still new, and forward isn't due again immediately.
        let after_forward = review_queue(&pool, user_id, None, ReviewOrder::DueDate)
            .await
            .unwrap();
        assert_eq!(after_forward.new_available_today, 1);
        assert_eq!(after_forward.due_review_count, 0);

        let forward_state = get_review_state(&pool, user_id, card_id, Direction::Forward)
            .await
            .unwrap();
        let reverse_state = get_review_state(&pool, user_id, card_id, Direction::Reverse)
            .await
            .unwrap();
        assert_eq!(forward_state.repetitions, 1);
        assert_eq!(reverse_state, ReviewState::default());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn non_two_way_cards_only_ever_offer_the_forward_direction(pool: PgPool) {
        let user_id = test_user(&pool).await;
        create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();

        let queue = review_queue(&pool, user_id, None, ReviewOrder::DueDate)
            .await
            .unwrap();
        assert_eq!(queue.new_available_today, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn grading_removes_card_from_due_review_until_its_next_interval(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();

        // Before any review, it's a "new" card, not a "due review" one.
        let before = review_queue(&pool, user_id, None, ReviewOrder::DueDate)
            .await
            .unwrap();
        assert_eq!(before.due_review_count, 0);
        assert_eq!(before.new_available_today, 1);

        record_review(&pool, user_id, card_id, Grade::Good, Direction::Forward)
            .await
            .unwrap();

        let after = review_queue(&pool, user_id, None, ReviewOrder::DueDate)
            .await
            .unwrap();
        assert!(
            after.cards.is_empty(),
            "a card just graded Good is scheduled a day out, not due again immediately"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn daily_new_card_limit_caps_how_many_new_cards_appear(pool: PgPool) {
        let user_id = test_user(&pool).await;
        set_daily_new_card_limit(&pool, user_id, Some(1))
            .await
            .unwrap();

        create_card(&pool, user_id, card_with_text("new-one"), &[])
            .await
            .unwrap();
        create_card(&pool, user_id, card_with_text("new-two"), &[])
            .await
            .unwrap();

        let queue = review_queue(&pool, user_id, None, ReviewOrder::DueDate)
            .await
            .unwrap();
        assert_eq!(
            queue.new_available_today, 1,
            "only 1 of the 2 new cards fits under today's limit"
        );
        assert_eq!(queue.cards.len(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn review_cards_are_not_limited_by_the_new_card_cap(pool: PgPool) {
        let user_id = test_user(&pool).await;
        set_daily_new_card_limit(&pool, user_id, Some(0))
            .await
            .unwrap(); // no new cards allowed today at all

        let card_id = create_card(&pool, user_id, card_with_text("word"), &[])
            .await
            .unwrap();
        record_review(&pool, user_id, card_id, Grade::Again, Direction::Forward)
            .await
            .unwrap(); // introduces it, schedules a ~10-minute relearning step

        // Fast-forward past the relearning step without waiting on it.
        sqlx::query(
            "UPDATE review_state SET due_at = now() - interval '1 minute' \
             WHERE user_id = $1 AND card_id = $2",
        )
        .bind(user_id)
        .bind(card_id)
        .execute(&pool)
        .await
        .unwrap();

        let queue = review_queue(&pool, user_id, None, ReviewOrder::DueDate)
            .await
            .unwrap();
        assert_eq!(
            queue.due_review_count, 1,
            "a card already in review isn't capped by the new-card limit"
        );
        assert_eq!(queue.new_available_today, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn daily_new_card_limit_counts_cards_already_introduced_today(pool: PgPool) {
        let user_id = test_user(&pool).await;
        set_daily_new_card_limit(&pool, user_id, Some(1))
            .await
            .unwrap();

        let first = create_card(&pool, user_id, card_with_text("first"), &[])
            .await
            .unwrap();
        create_card(&pool, user_id, card_with_text("second"), &[])
            .await
            .unwrap();

        record_review(&pool, user_id, first, Grade::Good, Direction::Forward)
            .await
            .unwrap();

        let queue = review_queue(&pool, user_id, None, ReviewOrder::DueDate)
            .await
            .unwrap();
        assert_eq!(
            queue.new_available_today, 0,
            "today's one new-card slot was already used"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn relapsing_counts_as_a_lapse_and_is_visible_in_stats(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("hard-word"), &[])
            .await
            .unwrap();

        record_review(&pool, user_id, card_id, Grade::Good, Direction::Forward)
            .await
            .unwrap();
        record_review(&pool, user_id, card_id, Grade::Again, Direction::Forward)
            .await
            .unwrap();

        let stats = user_stats(&pool, user_id).await.unwrap();
        assert_eq!(stats.hardest_cards.len(), 1);
        assert_eq!(stats.hardest_cards[0].id, card_id);
        assert_eq!(stats.hardest_cards[0].lapses, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn user_stats_counts_reviews_logged_today(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let a = create_card(&pool, user_id, card_with_text("a"), &[])
            .await
            .unwrap();
        let b = create_card(&pool, user_id, card_with_text("b"), &[])
            .await
            .unwrap();

        record_review(&pool, user_id, a, Grade::Good, Direction::Forward)
            .await
            .unwrap();
        record_review(&pool, user_id, b, Grade::Easy, Direction::Forward)
            .await
            .unwrap();

        let stats = user_stats(&pool, user_id).await.unwrap();
        assert_eq!(stats.reviews_today, 2);
        assert_eq!(stats.current_streak_days, 1);
        assert_eq!(stats.total_cards, 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn upsert_word_definition_creates_a_new_word_with_examples_and_links(pool: PgPool) {
        let user_id = test_user(&pool).await;

        upsert_word_definition(
            &pool,
            "perro",
            user_id,
            WordFields {
                part_of_speech: Some("noun"),
                notes: Some("the common word for dog"),
                ..Default::default()
            },
            &[(
                "El perro corre.".to_string(),
                Some("The dog runs.".to_string()),
            )],
            &["can".to_string()],
        )
        .await
        .unwrap();

        let def = get_word_definition(&pool, "perro").await.unwrap().unwrap();
        assert_eq!(def.part_of_speech, Some("noun".to_string()));
        assert_eq!(def.notes, Some("the common word for dog".to_string()));
        assert_eq!(def.examples.len(), 1);
        assert_eq!(def.examples[0].target_sentence, "El perro corre.");
        assert_eq!(def.linked_words.len(), 1);
        assert_eq!(def.linked_words[0].target_text, "can");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn upsert_word_definition_on_same_text_updates_rather_than_duplicates(pool: PgPool) {
        let user_id = test_user(&pool).await;

        upsert_word_definition(
            &pool,
            "casa",
            user_id,
            WordFields {
                part_of_speech: Some("noun"),
                ..Default::default()
            },
            &[],
            &[],
        )
        .await
        .unwrap();
        upsert_word_definition(
            &pool,
            "casa",
            user_id,
            WordFields {
                part_of_speech: Some("noun (updated)"),
                ..Default::default()
            },
            &[],
            &[],
        )
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM words WHERE target_text = $1")
            .bind("casa")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);

        let def = get_word_definition(&pool, "casa").await.unwrap().unwrap();
        assert_eq!(def.part_of_speech, Some("noun (updated)".to_string()));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_word_definition_returns_none_for_unknown_text(pool: PgPool) {
        assert!(
            get_word_definition(&pool, "nonexistent")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn linked_words_are_symmetric_regardless_of_which_side_was_saved(pool: PgPool) {
        let user_id = test_user(&pool).await;

        upsert_word_definition(
            &pool,
            "a",
            user_id,
            WordFields::default(),
            &[],
            &["b".to_string()],
        )
        .await
        .unwrap();

        let def_b = get_word_definition(&pool, "b").await.unwrap().unwrap();
        assert_eq!(def_b.linked_words.len(), 1);
        assert_eq!(def_b.linked_words[0].target_text, "a");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn re_saving_a_words_links_replaces_the_previous_set(pool: PgPool) {
        let user_id = test_user(&pool).await;

        upsert_word_definition(
            &pool,
            "a",
            user_id,
            WordFields::default(),
            &[],
            &["b".to_string(), "c".to_string()],
        )
        .await
        .unwrap();
        upsert_word_definition(
            &pool,
            "a",
            user_id,
            WordFields::default(),
            &[],
            &["c".to_string()],
        )
        .await
        .unwrap();

        let def_a = get_word_definition(&pool, "a").await.unwrap().unwrap();
        assert_eq!(def_a.linked_words.len(), 1);
        assert_eq!(def_a.linked_words[0].target_text, "c");

        let def_b = get_word_definition(&pool, "b").await.unwrap().unwrap();
        assert!(def_b.linked_words.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn linking_to_a_word_with_no_existing_entry_creates_a_satisfiable_stub_row(pool: PgPool) {
        let user_id = test_user(&pool).await;

        upsert_word_definition(
            &pool,
            "a",
            user_id,
            WordFields::default(),
            &[],
            &["stub".to_string()],
        )
        .await
        .unwrap();

        // The stub has no part_of_speech/notes of its own yet, but it
        // exists and is linked.
        let def_stub = get_word_definition(&pool, "stub").await.unwrap().unwrap();
        assert_eq!(def_stub.part_of_speech, None);
        assert_eq!(def_stub.linked_words[0].target_text, "a");

        // Giving the stub its own real entry later updates it in place
        // rather than creating a second `words` row.
        upsert_word_definition(
            &pool,
            "stub",
            user_id,
            WordFields {
                part_of_speech: Some("noun"),
                ..Default::default()
            },
            &[],
            &[],
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM words WHERE target_text = $1")
            .bind("stub")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_word_is_never_linked_to_itself(pool: PgPool) {
        let user_id = test_user(&pool).await;

        upsert_word_definition(
            &pool,
            "a",
            user_id,
            WordFields::default(),
            &[],
            &["a".to_string()],
        )
        .await
        .unwrap();

        let def_a = get_word_definition(&pool, "a").await.unwrap().unwrap();
        assert!(def_a.linked_words.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn linking_a_card_to_a_page_makes_it_findable_from_either_side(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("corro"), &[])
            .await
            .unwrap();
        let page_id = create_page(&pool, user_id, None, "Present tense", "present-tense")
            .await
            .unwrap();

        add_page_to_card(&pool, card_id, page_id).await.unwrap();

        assert_eq!(card_page_ids(&pool, card_id).await.unwrap(), vec![page_id]);
        let linked = cards_for_page(&pool, page_id).await.unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].target_text, "corro");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn removing_a_page_from_a_card_excludes_it_again(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_id = create_card(&pool, user_id, card_with_text("corro"), &[])
            .await
            .unwrap();
        let page_id = create_page(&pool, user_id, None, "Present tense", "present-tense")
            .await
            .unwrap();
        add_page_to_card(&pool, card_id, page_id).await.unwrap();

        remove_page_from_card(&pool, card_id, page_id)
            .await
            .unwrap();

        assert_eq!(
            card_page_ids(&pool, card_id).await.unwrap(),
            Vec::<Uuid>::new()
        );
        assert!(cards_for_page(&pool, page_id).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_link_is_findable_from_either_card_regardless_of_insert_order(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_a = create_card(&pool, user_id, card_with_text("pan"), &[])
            .await
            .unwrap();
        let card_b = create_card(&pool, user_id, card_with_text("pin"), &[])
            .await
            .unwrap();

        add_card_link(&pool, card_a, card_b, "minimal_pair")
            .await
            .unwrap();

        let from_a = linked_cards(&pool, card_a).await.unwrap();
        assert_eq!(from_a.len(), 1);
        assert_eq!(from_a[0].linked_card_id, card_b);
        assert_eq!(from_a[0].target_text, "pin");

        let from_b = linked_cards(&pool, card_b).await.unwrap();
        assert_eq!(from_b.len(), 1);
        assert_eq!(from_b[0].linked_card_id, card_a);
        assert_eq!(from_b[0].target_text, "pan");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn adding_the_same_link_twice_is_a_no_op(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_a = create_card(&pool, user_id, card_with_text("pan"), &[])
            .await
            .unwrap();
        let card_b = create_card(&pool, user_id, card_with_text("pin"), &[])
            .await
            .unwrap();

        add_card_link(&pool, card_a, card_b, "minimal_pair")
            .await
            .unwrap();
        add_card_link(&pool, card_a, card_b, "minimal_pair")
            .await
            .unwrap();

        assert_eq!(linked_cards(&pool, card_a).await.unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn adding_the_reverse_order_of_an_existing_link_does_not_create_a_duplicate_row(
        pool: PgPool,
    ) {
        let user_id = test_user(&pool).await;
        let card_a = create_card(&pool, user_id, card_with_text("pan"), &[])
            .await
            .unwrap();
        let card_b = create_card(&pool, user_id, card_with_text("pin"), &[])
            .await
            .unwrap();

        add_card_link(&pool, card_a, card_b, "minimal_pair")
            .await
            .unwrap();
        // Same unordered pair/type, opposite column order — the
        // card_links_unordered_pair_idx expression unique index should
        // block this as a duplicate, not create a second row.
        add_card_link(&pool, card_b, card_a, "minimal_pair")
            .await
            .unwrap();

        assert_eq!(linked_cards(&pool, card_a).await.unwrap().len(), 1);
        assert_eq!(linked_cards(&pool, card_b).await.unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn the_same_pair_can_hold_two_different_link_types_simultaneously(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_a = create_card(&pool, user_id, card_with_text("pan"), &[])
            .await
            .unwrap();
        let card_b = create_card(&pool, user_id, card_with_text("pin"), &[])
            .await
            .unwrap();

        add_card_link(&pool, card_a, card_b, "minimal_pair")
            .await
            .unwrap();
        add_card_link(&pool, card_a, card_b, "sound_spelling")
            .await
            .unwrap();

        let links = linked_cards(&pool, card_a).await.unwrap();
        assert_eq!(links.len(), 2);
        let mut link_types: Vec<_> = links.iter().map(|l| l.link_type.clone()).collect();
        link_types.sort();
        assert_eq!(link_types, vec!["minimal_pair", "sound_spelling"]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn removing_a_link_removes_it_regardless_of_which_order_it_was_inserted_in(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_a = create_card(&pool, user_id, card_with_text("pan"), &[])
            .await
            .unwrap();
        let card_b = create_card(&pool, user_id, card_with_text("pin"), &[])
            .await
            .unwrap();
        add_card_link(&pool, card_a, card_b, "minimal_pair")
            .await
            .unwrap();

        // Removed via the reverse order from how it was inserted.
        remove_card_link(&pool, card_b, card_a, "minimal_pair")
            .await
            .unwrap();

        assert!(linked_cards(&pool, card_a).await.unwrap().is_empty());
        assert!(linked_cards(&pool, card_b).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deleting_either_card_in_a_link_cascades_the_link_row(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let card_a = create_card(&pool, user_id, card_with_text("pan"), &[])
            .await
            .unwrap();
        let card_b = create_card(&pool, user_id, card_with_text("pin"), &[])
            .await
            .unwrap();
        let card_c = create_card(&pool, user_id, card_with_text("ton"), &[])
            .await
            .unwrap();
        let card_d = create_card(&pool, user_id, card_with_text("tan"), &[])
            .await
            .unwrap();
        // card_a is stored as card_id in this row...
        add_card_link(&pool, card_a, card_b, "minimal_pair")
            .await
            .unwrap();
        // ...and card_d is stored as linked_card_id in this one.
        add_card_link(&pool, card_c, card_d, "minimal_pair")
            .await
            .unwrap();

        delete_card(&pool, card_a).await.unwrap();
        delete_card(&pool, card_d).await.unwrap();

        assert!(linked_cards(&pool, card_b).await.unwrap().is_empty());
        assert!(linked_cards(&pool, card_c).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn linked_cards_reports_which_side_is_the_source_for_a_directed_link(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let earlier_step = create_card(&pool, user_id, card_with_text("na"), &[])
            .await
            .unwrap();
        let later_step = create_card(&pool, user_id, card_with_text("nation"), &[])
            .await
            .unwrap();

        add_card_link(&pool, earlier_step, later_step, "back_chain_step")
            .await
            .unwrap();

        let from_earlier = linked_cards(&pool, earlier_step).await.unwrap();
        assert_eq!(from_earlier.len(), 1);
        assert!(from_earlier[0].is_source);

        let from_later = linked_cards(&pool, later_step).await.unwrap();
        assert_eq!(from_later.len(), 1);
        assert!(!from_later[0].is_source);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn search_cards_matches_by_substring_and_excludes_the_given_card(pool: PgPool) {
        let user_id = test_user(&pool).await;
        let pan = create_card(&pool, user_id, card_with_text("pan"), &[])
            .await
            .unwrap();
        let pin = create_card(&pool, user_id, card_with_text("pin"), &[])
            .await
            .unwrap();
        create_card(&pool, user_id, card_with_text("gato"), &[])
            .await
            .unwrap();

        let results = search_cards(&pool, "p", pan).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, pin);
    }
}

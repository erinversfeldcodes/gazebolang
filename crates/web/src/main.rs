#[cfg(feature = "ssr")]
use web::state::AppState;

#[cfg(feature = "ssr")]
async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> axum::http::StatusCode {
    match sqlx::query("SELECT 1").execute(&state.db_pool).await {
        Ok(_) => axum::http::StatusCode::OK,
        Err(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Hit by CI right after every deploy (prod and PR previews) to prove the
/// Resend integration actually works end to end — not a liveness probe, so
/// it's deliberately not wired into fly.toml's automatic health checks,
/// since it sends a real email on every call.
#[cfg(feature = "ssr")]
async fn email_check(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::http::StatusCode {
    let expected_token = match std::env::var("INTERNAL_CHECK_TOKEN") {
        Ok(token) if !token.is_empty() => token,
        _ => return axum::http::StatusCode::UNAUTHORIZED,
    };
    let provided_token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if provided_token != Some(expected_token.as_str()) {
        return axum::http::StatusCode::UNAUTHORIZED;
    }

    let Ok(recipient) = std::env::var("TEST_EMAIL_RECIPIENT") else {
        log::info!("email-check: TEST_EMAIL_RECIPIENT not set, skipping");
        return axum::http::StatusCode::OK;
    };

    match state.email_sender.send_test_email(&recipient).await {
        Ok(()) => {
            log::info!("email-check: test email sent to {recipient}");
            axum::http::StatusCode::OK
        }
        Err(e) => {
            log::error!("email-check: failed to send test email: {e}");
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// Same shape and purpose as `email_check`: hit by CI right after every
/// deploy to prove the configured S3-compatible storage (Tigris in
/// prod/preview) is actually reachable and writable, not just that env
/// vars are set. Not a liveness probe, so not wired into fly.toml's
/// automatic health checks — it does a real write on every call.
#[cfg(feature = "ssr")]
async fn storage_check(
    axum::extract::State(state): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::http::StatusCode {
    let expected_token = match std::env::var("INTERNAL_CHECK_TOKEN") {
        Ok(token) if !token.is_empty() => token,
        _ => return axum::http::StatusCode::UNAUTHORIZED,
    };
    let provided_token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if provided_token != Some(expected_token.as_str()) {
        return axum::http::StatusCode::UNAUTHORIZED;
    }

    let Some(storage) = &state.storage else {
        log::info!("storage-check: no S3_* env vars set, skipping");
        return axum::http::StatusCode::OK;
    };

    match storage
        .upload(
            "internal/storage-check.txt",
            "text/plain",
            b"gazebo storage-check",
        )
        .await
    {
        Ok(url) => {
            log::info!("storage-check: upload succeeded, reachable at {url}");
            axum::http::StatusCode::OK
        }
        Err(e) => {
            log::error!("storage-check: upload failed: {e}");
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// Uploads one `image`/`audio`/`video` multipart field to storage.
/// Best-effort, same as before this was extracted: with no storage
/// configured (e.g. local dev without the S3_* env vars set), an attached
/// file is silently skipped rather than failing the whole submission —
/// the pasted-URL path (image only) still works either way.
#[cfg(feature = "ssr")]
async fn upload_media_field(
    state: &AppState,
    field: axum::extract::multipart::Field<'_>,
    kind: &str,
) -> Result<Option<String>, (axum::http::StatusCode, String)> {
    let content_type = field.content_type().map(str::to_string);
    let data = field
        .bytes()
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()))?;
    if data.is_empty() {
        return Ok(None);
    }
    match (&state.storage, content_type) {
        (Some(storage), Some(content_type)) => {
            let key = format!("cards/{}", web::storage::random_key());
            let url = storage
                .upload(&key, &content_type, &data)
                .await
                .map_err(|e| {
                    log::error!("card {kind} upload failed: {e}");
                    (axum::http::StatusCode::BAD_GATEWAY, e)
                })?;
            Ok(Some(url))
        }
        (None, _) => {
            log::warn!(
                "card {kind} upload attached but no storage is configured — dropping it silently"
            );
            Ok(None)
        }
        (Some(_), None) => {
            log::warn!("card {kind} upload attached with no content-type — dropping it");
            Ok(None)
        }
    }
}

/// Extracts a `YouTube` video ID from any of the common share-link shapes
/// users actually paste (`youtube.com/watch?v=...`, `youtu.be/...`, and
/// an already-canonical `youtube.com/embed/...`), rejecting anything else
/// outright rather than silently storing an unplayable link. Deliberately
/// not a full URL parser — just enough to cover what a "share" or "copy
/// link" button on youtube.com actually produces.
#[cfg(feature = "ssr")]
fn extract_youtube_video_id(input: &str) -> Option<&str> {
    let without_scheme = input
        .trim()
        .strip_prefix("https://")
        .or_else(|| input.trim().strip_prefix("http://"))?;
    let without_host_prefix = without_scheme
        .strip_prefix("www.")
        .or_else(|| without_scheme.strip_prefix("m."))
        .unwrap_or(without_scheme);

    let id = if let Some(rest) = without_host_prefix.strip_prefix("youtu.be/") {
        rest.split(['?', '&', '#']).next()?
    } else if let Some(rest) = without_host_prefix.strip_prefix("youtube.com/embed/") {
        rest.split(['?', '&', '#']).next()?
    } else {
        let rest = without_host_prefix.strip_prefix("youtube.com/watch")?;
        rest.strip_prefix('?')?
            .split('&')
            .find_map(|pair| pair.strip_prefix("v="))?
            .split(['&', '#'])
            .next()?
    };

    // Real YouTube video IDs are always exactly 11 URL-safe characters.
    let is_plausible_id = id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    is_plausible_id.then_some(id)
}

/// Normalizes whatever `YouTube` share link a user pasted into the one
/// canonical embed-URL form the review/edit UI actually renders
/// (`<iframe src=...>`), or an error message if it isn't recognizably a
/// `YouTube` link at all.
#[cfg(feature = "ssr")]
fn youtube_embed_url(input: &str) -> Result<String, String> {
    extract_youtube_video_id(input)
        .map(|id| format!("https://www.youtube.com/embed/{id}"))
        .ok_or_else(|| "that doesn't look like a YouTube video link".to_string())
}

// Every bool here is an independent, orthogonal form flag (a value and its
// own "remove this" checkbox, per media field) rather than a set of states
// that could collapse into an enum, so the usual fix for this lint doesn't
// apply cleanly.
#[cfg(feature = "ssr")]
#[allow(clippy::struct_excessive_bools)]
struct CardFormFields {
    target_text: String,
    // The known-language sentence — only meaningful for (and only ever
    // rendered on the form for) `sentence_translation` cards.
    source_text: String,
    tags_raw: String,
    // Validated against `web::cards::CARD_TYPES` in `parse_card_multipart`
    // — always a known value by the time a caller sees this field, never
    // arbitrary user text.
    card_type: String,
    two_way: bool,
    image_url: Option<String>,
    audio_url: Option<String>,
    video_url: Option<String>,
    // `Some` only once validated by `youtube_embed_url` — an invalid
    // pasted link fails the whole submission (see parse_card_multipart)
    // rather than being silently dropped or stored unplayable.
    youtube_url: Option<String>,
    // Only meaningful on the edit form, which is the only one that ever
    // renders these checkboxes (there's nothing to remove from a brand
    // new card) — a new upload/link for the same field always wins over a
    // remove request, since submitting both at once only makes sense as
    // "replace", not "replace, then immediately delete what I just set".
    remove_image: bool,
    remove_audio: bool,
    remove_video: bool,
    remove_youtube: bool,
    // Only ever populated by NewCardPage's deck checkboxes — EditCardPage's
    // form never emits a `deck_id` field (deck membership there is toggled
    // immediately via its own server actions, not submitted with this
    // form), so `cards_update` always sees this empty.
    deck_ids: Vec<uuid::Uuid>,
}

/// Pulls every field out of a card create/edit form submission: text
/// fields, the `two_way` checkbox, whichever of image/audio/video were
/// attached (each independent and optional — a card can have any
/// combination, including none), and the edit form's "remove" checkboxes.
#[cfg(feature = "ssr")]
async fn parse_card_multipart(
    state: &AppState,
    multipart: &mut axum::extract::Multipart,
) -> Result<CardFormFields, (axum::http::StatusCode, String)> {
    let mut fields = CardFormFields {
        target_text: String::new(),
        source_text: String::new(),
        tags_raw: String::new(),
        card_type: "image_to_target".to_string(),
        two_way: false,
        image_url: None,
        audio_url: None,
        video_url: None,
        youtube_url: None,
        remove_image: false,
        remove_audio: false,
        remove_video: false,
        remove_youtube: false,
        deck_ids: Vec::new(),
    };

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "target_text" => {
                fields.target_text = field.text().await.unwrap_or_default().trim().to_string();
            }
            "source_text" => {
                fields.source_text = field.text().await.unwrap_or_default().trim().to_string();
            }
            "tags" => fields.tags_raw = field.text().await.unwrap_or_default(),
            "card_type" => {
                let raw = field.text().await.unwrap_or_default();
                if web::cards::CARD_TYPES
                    .iter()
                    .any(|(value, _)| *value == raw)
                {
                    fields.card_type = raw;
                }
            }
            // A checkbox only appears in the multipart body at all when
            // checked, so this field's mere presence means true.
            "two_way" => fields.two_way = true,
            "remove_image" => fields.remove_image = true,
            "remove_audio" => fields.remove_audio = true,
            "remove_video" => fields.remove_video = true,
            "remove_youtube" => fields.remove_youtube = true,
            "image" => fields.image_url = upload_media_field(state, field, "image").await?,
            // Set once a search result has been imported (see
            // cards::import_card_image) and carried into the form as a
            // hidden field placed after the "image" file input, so this
            // arm runs later and wins if the user somehow set both.
            "image_url" => {
                let raw = field.text().await.unwrap_or_default();
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    fields.image_url = Some(trimmed.to_string());
                }
            }
            // One multipart field per checked checkbox, all sharing this
            // name — not a single overwritable slot — so a card can be
            // assigned to any number of decks at creation time.
            "deck_id" => {
                let raw = field.text().await.unwrap_or_default();
                if let Ok(deck_id) = raw.trim().parse() {
                    fields.deck_ids.push(deck_id);
                }
            }
            "audio" => fields.audio_url = upload_media_field(state, field, "audio").await?,
            "video" => fields.video_url = upload_media_field(state, field, "video").await?,
            "youtube_url" => {
                let raw = field.text().await.unwrap_or_default();
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    fields.youtube_url = Some(
                        youtube_embed_url(trimmed)
                            .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?,
                    );
                }
            }
            _ => {}
        }
    }

    Ok(fields)
}

#[cfg(feature = "ssr")]
fn parse_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Percent-encodes `s` for safe embedding as one query-string value in a
/// `Location` redirect header — just enough for a round trip through
/// `leptos_router`'s `use_query_map()` on the other end (which decodes it
/// back automatically, same as any URL query string), not a
/// general-purpose encoder.
#[cfg(feature = "ssr")]
fn percent_encode_query_value(s: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// A redirect back to `path` carrying `message` as a `?error=` query param.
/// `NewCardPage`/`BulkAddPage`/`EditCardPage` read it back and render it
/// with the same `"Error: "` paragraph every other form in the app already
/// uses — replaces this file's previous bare `(StatusCode, String)`
/// responses, which rendered as unstyled plain text outside the whole app
/// shell (no nav, no theme) on exactly the three pages most likely to hit
/// a real validation/upload failure.
#[cfg(feature = "ssr")]
fn error_redirect(path: &str, message: &str) -> axum::response::Redirect {
    axum::response::Redirect::to(&format!(
        "{path}?error={}",
        percent_encode_query_value(message)
    ))
}

/// Plain multipart form POST, not a Leptos server function — file uploads
/// need `multipart/form-data`, which is far simpler to handle as an
/// ordinary Axum route (a classic full-page form submit) than to wire
/// through Leptos's server-action/WASM machinery. Always redirects rather
/// than returning an error response directly (see `error_redirect`) so a
/// failure still lands the user back inside the styled app, not on a bare
/// status-code page.
#[cfg(feature = "ssr")]
async fn cards_new(
    axum::extract::State(state): axum::extract::State<AppState>,
    cookies: tower_cookies::Cookies,
    mut multipart: axum::extract::Multipart,
) -> axum::response::Redirect {
    let Some(user) = web::auth::current_user(&state.db_pool, &cookies).await else {
        // Can only happen bypassing the UI entirely — NewCardPage itself
        // already redirects here when signed out.
        return axum::response::Redirect::to("/login");
    };

    let fields = match parse_card_multipart(&state, &mut multipart).await {
        Ok(fields) => fields,
        Err((_, message)) => return error_redirect("/new", &message),
    };

    let result = web::db::create_card(
        &state.db_pool,
        user.id,
        web::db::NewCard {
            card_type: &fields.card_type,
            image_url: fields.image_url.as_deref().unwrap_or(""),
            audio_url: fields.audio_url.as_deref().unwrap_or(""),
            video_url: fields.video_url.as_deref().unwrap_or(""),
            youtube_url: fields.youtube_url.as_deref().unwrap_or(""),
            target_text: &fields.target_text,
            source_text: &fields.source_text,
            two_way: fields.two_way,
        },
        &parse_tags(&fields.tags_raw),
    )
    .await;
    let card_id = match result {
        Ok(id) => id,
        Err(e) => return error_redirect("/new", &e.to_string()),
    };

    // Best-effort, same as `upload_media_field` above: the card itself is
    // already created at this point, so a deck-membership failure here
    // (only plausible on a genuine DB/connectivity issue — `deck_id`s come
    // from checkboxes for decks `list_decks` just returned, not user input)
    // logs and moves on rather than showing an "error" for a card that in
    // fact exists.
    for deck_id in fields.deck_ids {
        if let Err(e) = web::db::add_card_to_deck(&state.db_pool, deck_id, card_id).await {
            log::error!("failed to assign new card {card_id} to deck {deck_id}: {e}");
        }
    }

    axum::response::Redirect::to("/")
}

/// One parsed-and-ready-to-create entry from the bulk-add page — a target
/// word/phrase plus whichever image (uploaded file or search import) was
/// attached to it. The English translation used to search for that image
/// never reaches here at all — see `ImageSearchPicker`'s doc comment — so
/// there's nothing to carry for it.
#[cfg(feature = "ssr")]
struct BulkDraftFields {
    target_text: String,
    image_url: Option<String>,
}

/// Parses `BulkAddPage`'s submission: one `<div>` per draft, each
/// contributing its own `target_text_{i}`/`image_{i}`/`image_url_{i}`
/// fields (indexed rather than one of each, since this form carries an
/// unbounded number of cards at once — see `cards_bulk`). Field order
/// within the multipart body isn't assumed; a `BTreeMap` keyed by index
/// collects everything first, then gets flattened once the whole body has
/// been read.
#[cfg(feature = "ssr")]
async fn parse_bulk_multipart(
    state: &AppState,
    multipart: &mut axum::extract::Multipart,
) -> Result<Vec<BulkDraftFields>, (axum::http::StatusCode, String)> {
    let mut target_texts: std::collections::BTreeMap<usize, String> =
        std::collections::BTreeMap::new();
    let mut image_urls: std::collections::BTreeMap<usize, String> =
        std::collections::BTreeMap::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        // "image_url_" must be checked before "image_" — it's the more
        // specific prefix, and would otherwise also match the latter.
        if let Some(idx) = name
            .strip_prefix("target_text_")
            .and_then(|s| s.parse().ok())
        {
            target_texts.insert(idx, field.text().await.unwrap_or_default());
        } else if let Some(idx) = name.strip_prefix("image_url_").and_then(|s| s.parse().ok()) {
            let raw = field.text().await.unwrap_or_default();
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                image_urls.insert(idx, trimmed.to_string());
            }
        } else if let Some(idx) = name.strip_prefix("image_").and_then(|s| s.parse().ok())
            && let Some(url) = upload_media_field(state, field, "image").await?
        {
            image_urls.insert(idx, url);
        }
    }

    Ok(target_texts
        .into_iter()
        .filter_map(|(idx, target_text)| {
            let target_text = target_text.trim().to_string();
            (!target_text.is_empty()).then(|| BulkDraftFields {
                target_text,
                image_url: image_urls.get(&idx).cloned(),
            })
        })
        .collect())
}

#[cfg(feature = "ssr")]
#[derive(serde::Deserialize)]
struct ImporterLoginForm {
    email: String,
    password: String,
}

/// Plain, stable login route for the `importer` CLI tool
/// (`crates/importer/src/app_client.rs`), as a URL-encoded POST rather
/// than a Leptos `#[server]` fn call. The browser's own login goes
/// through `account::log_in`'s server-fn wire format instead, whose
/// path is auto-generated at build time and — confirmed in a real run
/// — isn't even stable across build platforms for the same source
/// (macOS vs. Linux produced different hashes), making it unusable as
/// a fixed target for a native binary. This route's path never
/// changes, so `importer` doesn't need to reverse-engineer anything.
#[cfg(feature = "ssr")]
async fn importer_login(
    axum::extract::State(state): axum::extract::State<AppState>,
    cookies: tower_cookies::Cookies,
    axum::Form(form): axum::Form<ImporterLoginForm>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match web::auth::log_in(&state.db_pool, &form.email, &form.password).await {
        Ok(session_token) => {
            cookies.add(web::auth::build_session_cookie(session_token));
            axum::http::StatusCode::OK.into_response()
        }
        Err(message) => (axum::http::StatusCode::UNAUTHORIZED, message).into_response(),
    }
}

#[cfg(feature = "ssr")]
#[derive(serde::Deserialize)]
struct ImporterSaveWordDefinitionForm {
    target_text: String,
    #[serde(default)]
    part_of_speech: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    english_translation: String,
    #[serde(default)]
    example_target_sentence: String,
    #[serde(default)]
    example_gloss_sentence: String,
    #[serde(default)]
    linked_words_raw: String,
}

/// Plain, stable counterpart to `importer_login` for
/// `dictionary::save_word_definition` — see that fn's own doc comment
/// for why. Field-level behavior (trimming, blank-to-`None`, the
/// comma-separated linked-words list) mirrors the server-fn version
/// exactly; `non_empty_trimmed` is duplicated rather than shared across
/// the bin/lib boundary, the same way `parse_tags`/`parse_comma_list`
/// already are for the same reason (see that fn's own comment).
#[cfg(feature = "ssr")]
async fn importer_save_word_definition(
    axum::extract::State(state): axum::extract::State<AppState>,
    cookies: tower_cookies::Cookies,
    axum::Form(form): axum::Form<ImporterSaveWordDefinitionForm>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    fn non_empty_trimmed(s: &str) -> Option<String> {
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
    }

    let Some(user) = web::auth::current_user(&state.db_pool, &cookies).await else {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    };
    let target_text = form.target_text.trim();
    if target_text.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "target_text can't be empty",
        )
            .into_response();
    }
    let examples = non_empty_trimmed(&form.example_target_sentence)
        .map(|t| vec![(t, non_empty_trimmed(&form.example_gloss_sentence))])
        .unwrap_or_default();
    let linked = parse_tags(&form.linked_words_raw);

    let result = web::db::upsert_word_definition(
        &state.db_pool,
        target_text,
        user.id,
        web::db::WordFields {
            part_of_speech: non_empty_trimmed(&form.part_of_speech).as_deref(),
            notes: non_empty_trimmed(&form.notes).as_deref(),
            english_translation: non_empty_trimmed(&form.english_translation).as_deref(),
        },
        &examples,
        &linked,
    )
    .await;
    match result {
        Ok(_) => axum::http::StatusCode::OK.into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Creates every draft from `BulkAddPage` in one submission. Stops at the
/// first failure — same bail-on-first-error semantics `cards_new` already
/// uses — rather than inventing partial-success UX for what should be a
/// rare failure mode (mainly a DB connectivity issue, since a storage
/// upload failure already degrades to "no image" silently upstream).
/// Drafts created before a later failure stay created; this isn't
/// transactional.
#[cfg(feature = "ssr")]
async fn cards_bulk(
    axum::extract::State(state): axum::extract::State<AppState>,
    cookies: tower_cookies::Cookies,
    mut multipart: axum::extract::Multipart,
) -> axum::response::Redirect {
    let Some(user) = web::auth::current_user(&state.db_pool, &cookies).await else {
        return axum::response::Redirect::to("/login");
    };

    let drafts = match parse_bulk_multipart(&state, &mut multipart).await {
        Ok(drafts) => drafts,
        Err((_, message)) => return error_redirect("/bulk-add", &message),
    };

    for draft in drafts {
        let result = web::db::create_card(
            &state.db_pool,
            user.id,
            web::db::NewCard {
                image_url: draft.image_url.as_deref().unwrap_or(""),
                target_text: &draft.target_text,
                ..Default::default()
            },
            &[],
        )
        .await;
        if let Err(e) = result {
            return error_redirect("/bulk-add", &e.to_string());
        }
    }

    axum::response::Redirect::to("/")
}

#[cfg(feature = "ssr")]
async fn cards_update(
    axum::extract::State(state): axum::extract::State<AppState>,
    cookies: tower_cookies::Cookies,
    axum::extract::Path(card_id): axum::extract::Path<uuid::Uuid>,
    mut multipart: axum::extract::Multipart,
) -> axum::response::Redirect {
    if web::auth::current_user(&state.db_pool, &cookies)
        .await
        .is_none()
    {
        return axum::response::Redirect::to("/login");
    }

    let edit_page = format!("/cards/{card_id}");
    // EditCardPage's own fetch already renders "Card not found."/
    // "Error: ..." for either case — no need to repeat it via ?error=.
    let Ok(Some(existing)) = web::db::get_card(&state.db_pool, card_id).await else {
        return axum::response::Redirect::to(&edit_page);
    };

    let fields = match parse_card_multipart(&state, &mut multipart).await {
        Ok(fields) => fields,
        Err((_, message)) => return error_redirect(&edit_page, &message),
    };
    // A new upload wins over a remove request (see CardFormFields); a
    // remove request wins over keeping the existing value; otherwise the
    // existing value is kept as-is.
    let image_url = fields.image_url.unwrap_or_else(|| {
        if fields.remove_image {
            String::new()
        } else {
            existing.image_url.unwrap_or_default()
        }
    });
    let audio_url = fields.audio_url.unwrap_or_else(|| {
        if fields.remove_audio {
            String::new()
        } else {
            existing.audio_url.unwrap_or_default()
        }
    });
    let video_url = fields.video_url.unwrap_or_else(|| {
        if fields.remove_video {
            String::new()
        } else {
            existing.video_url.unwrap_or_default()
        }
    });
    let youtube_url = fields.youtube_url.unwrap_or_else(|| {
        if fields.remove_youtube {
            String::new()
        } else {
            existing.youtube_url.unwrap_or_default()
        }
    });

    let result = web::db::update_card(
        &state.db_pool,
        card_id,
        web::db::NewCard {
            card_type: &fields.card_type,
            image_url: &image_url,
            audio_url: &audio_url,
            video_url: &video_url,
            youtube_url: &youtube_url,
            target_text: &fields.target_text,
            source_text: &fields.source_text,
            two_way: fields.two_way,
        },
        &parse_tags(&fields.tags_raw),
    )
    .await;
    if let Err(e) = result {
        return error_redirect(&edit_page, &e.to_string());
    }

    axum::response::Redirect::to(&edit_page)
}

/// Axum's default multipart body limit (2MB) comfortably fits an image
/// but not a voice clip or a short video, so card uploads need a higher
/// ceiling. Applied to the whole router rather than scoped to just the
/// card routes — every other route's payloads are tiny text/JSON, so a
/// generous shared ceiling costs nothing there.
#[cfg(feature = "ssr")]
const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

#[cfg(feature = "ssr")]
async fn auth_logout(
    axum::extract::State(state): axum::extract::State<AppState>,
    cookies: tower_cookies::Cookies,
) -> axum::response::Redirect {
    if let Some(cookie) = cookies.get(web::auth::SESSION_COOKIE) {
        web::auth::delete_session(&state.db_pool, cookie.value()).await;
    }
    cookies.remove(tower_cookies::Cookie::new(web::auth::SESSION_COOKIE, ""));
    axum::response::Redirect::to("/login")
}

// Neon's compute suspends after a few idle minutes on the plan this project
// runs on (free — scale-to-zero can't be disabled there), and the very
// first connection after it resumes can transiently fail auth before
// settling, confirmed against a real preview deploy via Neon's own
// operations log. Retrying absorbs that without masking a genuinely wrong
// DATABASE_URL, which still exhausts these attempts and panics as before.
#[cfg(feature = "ssr")]
async fn connect_with_retry(database_url: &str) -> sqlx::PgPool {
    const MAX_ATTEMPTS: u32 = 6;
    let mut attempt = 0;
    loop {
        attempt += 1;
        // A single EditCardPage load alone fires 4 concurrent resources
        // (auth, card, decks, pages), each needing its own pooled
        // connection at once — 5 total leaves next to no headroom for a
        // second concurrent visitor on this 4-user app, and connections
        // queue (not error) when the pool's exhausted, which is exactly
        // what a Playwright run against the preview stack surfaced as a
        // slow-but-silent request rather than a crash.
        match sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url)
            .await
        {
            Ok(pool) => return pool,
            Err(err) if attempt < MAX_ATTEMPTS => {
                log::warn!(
                    "database connection attempt {attempt}/{MAX_ATTEMPTS} failed, retrying: {err}"
                );
                tokio::time::sleep(std::time::Duration::from_secs(u64::from(attempt) * 2)).await;
            }
            Err(err) => {
                panic!("failed to connect to the database after {MAX_ATTEMPTS} attempts: {err}")
            }
        }
    }
}

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use leptos::prelude::*;
    use leptos_axum::{LeptosRoutes, generate_route_list};
    use web::app::{App, shell};

    env_logger::init();

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let addr = leptos_options.site_addr;
    let routes = generate_route_list(App);

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let db_pool = connect_with_retry(&database_url).await;

    // Embedded at compile time (no runtime file access needed, so this
    // works unchanged in the minimal Docker runtime image) and applied on
    // every boot. Neither `preview.yml` nor `deploy.yml` ever ran
    // migrations against the real Neon branches they target — only
    // `ci.yml`'s ephemeral local Postgres got them — so staging and
    // production both still had zero tables despite "successful" deploys.
    // Idempotent via sqlx's own tracking table, so this is safe to run
    // alongside that pre-existing CI step too.
    sqlx::migrate!("../../migrations")
        .run(&db_pool)
        .await
        .expect("failed to run database migrations");

    let state = AppState {
        leptos_options: leptos_options.clone(),
        db_pool,
        email_sender: web::email::EmailSender::from_env(),
        storage: web::storage::Storage::from_env(),
        image_search: web::image_search::ImageSearch::from_env(),
    };

    let app = Router::new()
        .leptos_routes(&state, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .route("/health", axum::routing::get(health))
        .route("/auth/logout", axum::routing::get(auth_logout))
        .route("/internal/email-check", axum::routing::get(email_check))
        .route("/internal/storage-check", axum::routing::get(storage_check))
        .route("/api/importer/login", axum::routing::post(importer_login))
        .route(
            "/api/importer/save-word-definition",
            axum::routing::post(importer_save_word_definition),
        )
        .route("/cards/new", axum::routing::post(cards_new))
        .route("/cards/bulk", axum::routing::post(cards_bulk))
        .route("/cards/{id}", axum::routing::post(cards_update))
        .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
        .layer(tower_cookies::CookieManagerLayer::new())
        .layer(axum::extract::DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

#[cfg(not(feature = "ssr"))]
fn main() {}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::{percent_encode_query_value, youtube_embed_url};

    #[test]
    fn accepts_a_watch_url() {
        assert_eq!(
            youtube_embed_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap(),
            "https://www.youtube.com/embed/dQw4w9WgXcQ"
        );
    }

    #[test]
    fn accepts_a_watch_url_with_v_not_in_first_position() {
        assert_eq!(
            youtube_embed_url("https://youtube.com/watch?list=PLxyz&v=dQw4w9WgXcQ&t=30s").unwrap(),
            "https://www.youtube.com/embed/dQw4w9WgXcQ"
        );
    }

    #[test]
    fn accepts_a_shortened_youtu_be_url() {
        assert_eq!(
            youtube_embed_url("https://youtu.be/dQw4w9WgXcQ?t=10").unwrap(),
            "https://www.youtube.com/embed/dQw4w9WgXcQ"
        );
    }

    #[test]
    fn accepts_an_already_canonical_embed_url() {
        assert_eq!(
            youtube_embed_url("https://www.youtube.com/embed/dQw4w9WgXcQ").unwrap(),
            "https://www.youtube.com/embed/dQw4w9WgXcQ"
        );
    }

    #[test]
    fn accepts_a_mobile_subdomain_url() {
        assert_eq!(
            youtube_embed_url("https://m.youtube.com/watch?v=dQw4w9WgXcQ").unwrap(),
            "https://www.youtube.com/embed/dQw4w9WgXcQ"
        );
    }

    #[test]
    fn rejects_a_non_youtube_url() {
        assert!(youtube_embed_url("https://vimeo.com/123456").is_err());
    }

    #[test]
    fn rejects_a_watch_url_with_no_video_id() {
        assert!(youtube_embed_url("https://www.youtube.com/watch?list=PLxyz").is_err());
    }

    #[test]
    fn rejects_plain_text_that_is_not_a_url_at_all() {
        assert!(youtube_embed_url("not a url").is_err());
    }

    #[test]
    fn percent_encode_leaves_unreserved_characters_alone() {
        assert_eq!(percent_encode_query_value("abc-XYZ_0.9~"), "abc-XYZ_0.9~");
    }

    #[test]
    fn percent_encode_escapes_spaces_and_url_reserved_characters() {
        assert_eq!(
            percent_encode_query_value("that doesn't look like a YouTube video link"),
            "that%20doesn%27t%20look%20like%20a%20YouTube%20video%20link"
        );
        assert_eq!(percent_encode_query_value("a&b=c#d%e"), "a%26b%3Dc%23d%25e");
    }

    #[test]
    fn percent_encode_escapes_multi_byte_utf8_one_byte_at_a_time() {
        // "é" is the two-byte UTF-8 sequence 0xC3 0xA9.
        assert_eq!(percent_encode_query_value("café"), "caf%C3%A9");
    }

    /// Decodes `%XX` escapes back to raw bytes, for round-tripping
    /// `percent_encode_query_value`'s own output — deliberately minimal
    /// (this file has no decoder of its own to reuse), not a general
    /// URL-decoding utility.
    fn percent_decode(s: &str) -> Vec<u8> {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        out
    }

    proptest::proptest! {
        /// The entire point of percent-encoding: decoding the output
        /// must always reconstruct the exact original bytes, for any
        /// string — not just the specific reserved/multi-byte examples
        /// already covered above.
        #[test]
        fn percent_encode_query_value_round_trips_through_percent_decode(s in ".*") {
            let encoded = percent_encode_query_value(&s);
            let decoded = percent_decode(&encoded);
            proptest::prop_assert_eq!(decoded, s.as_bytes().to_vec());
        }

        /// Every byte in the output is either one of the explicitly
        /// unreserved characters, verbatim, or part of a `%XX` escape
        /// — never anything else that could be misread as a query
        /// delimiter.
        #[test]
        fn percent_encode_query_value_only_emits_unreserved_chars_or_percent_escapes(s in ".*") {
            let encoded = percent_encode_query_value(&s);
            let bytes = encoded.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let b = bytes[i];
                let is_unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
                if is_unreserved {
                    i += 1;
                } else {
                    proptest::prop_assert_eq!(b, b'%', "unexpected raw byte {} in {:?}", b, encoded);
                    proptest::prop_assert!(i + 2 < bytes.len(), "truncated %-escape at end of {:?}", encoded);
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                    proptest::prop_assert!(
                        u8::from_str_radix(hex, 16).is_ok(),
                        "invalid escape in {:?}",
                        encoded
                    );
                    i += 3;
                }
            }
        }
    }
}

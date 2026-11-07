use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardCard {
    pub id: Uuid,
    pub target_text: String,
    pub difficulty: f32,
    pub lapses: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub total_cards: i64,
    pub due_now: i64,
    pub reviews_today: i64,
    pub current_streak_days: i64,
    pub hardest_cards: Vec<HardCard>,
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
async fn session_cookies() -> Result<tower_cookies::Cookies, ServerFnError> {
    leptos_axum::extract::<tower_cookies::Cookies>()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Mints an invite and emails the link. Any signed-in user may invite —
/// see `auth::create_invite`'s doc comment for why.
#[server]
pub async fn create_invite(email: String) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let base_url =
        std::env::var("APP_BASE_URL").unwrap_or_else(|_| "http://localhost:3000".to_string());
    let token = crate::auth::create_invite(&state.db_pool, user.id, &email)
        .await
        .map_err(ServerFnError::new)?;
    let link = format!("{base_url}/signup?token={token}");
    if let Err(e) = state.email_sender.send_invite(&email, &link).await {
        return Err(ServerFnError::new(format!(
            "invite created, but the email failed to send: {e}"
        )));
    }
    Ok(())
}

/// Creates a new account by redeeming an invite token. This is now the
/// *only* way a new account gets created — open self-serve signup no
/// longer exists (see `auth::sign_up_via_invite`'s doc comment).
#[server]
pub async fn sign_up_via_invite(
    token: String,
    name: String,
    password: String,
    confirm_password: String,
) -> Result<(), ServerFnError> {
    if password != confirm_password {
        return Err(ServerFnError::new("passwords do not match"));
    }
    let state = expect_context::<crate::state::AppState>();
    let session_token = crate::auth::sign_up_via_invite(&state.db_pool, &token, &name, &password)
        .await
        .map_err(ServerFnError::new)?;
    session_cookies()
        .await?
        .add(crate::auth::build_session_cookie(session_token));
    Ok(())
}

#[server]
pub async fn log_in(email: String, password: String) -> Result<(), ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let session_token = crate::auth::log_in(&state.db_pool, &email, &password)
        .await
        .map_err(ServerFnError::new)?;
    session_cookies()
        .await?
        .add(crate::auth::build_session_cookie(session_token));
    Ok(())
}

#[server]
pub async fn request_password_reset(email: String) -> Result<(), ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let base_url =
        std::env::var("APP_BASE_URL").unwrap_or_else(|_| "http://localhost:3000".to_string());
    crate::auth::request_password_reset(&state.db_pool, &state.email_sender, &base_url, &email)
        .await
        .map_err(ServerFnError::new)
}

#[server]
pub async fn confirm_password_reset(
    token: String,
    password: String,
    confirm_password: String,
) -> Result<(), ServerFnError> {
    if password != confirm_password {
        return Err(ServerFnError::new("passwords do not match"));
    }
    let state = expect_context::<crate::state::AppState>();
    crate::auth::confirm_password_reset(&state.db_pool, &token, &password)
        .await
        .map_err(ServerFnError::new)
}

#[server]
pub async fn current_user_name() -> Result<Option<String>, ServerFnError> {
    let state = expect_context::<crate::state::AppState>();
    let cookies = leptos_axum::extract::<tower_cookies::Cookies>()
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(crate::auth::current_user(&state.db_pool, &cookies)
        .await
        .map(|u| u.name))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub email: String,
}

#[server]
pub async fn current_profile() -> Result<Profile, ServerFnError> {
    let user = require_user().await?;
    Ok(Profile {
        name: user.name,
        email: user.email,
    })
}

#[server]
pub async fn update_name(name: String) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::auth::update_name(&state.db_pool, user.id, &name)
        .await
        .map_err(ServerFnError::new)
}

#[server]
pub async fn update_email(
    current_password: String,
    new_email: String,
) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::auth::update_email(&state.db_pool, user.id, &current_password, &new_email)
        .await
        .map_err(ServerFnError::new)
}

/// Revokes every session for the account server-side (same as redeeming a
/// password-reset token), so the cookie on this request is cleared too —
/// otherwise the browser would keep sending a token that no longer
/// resolves to anyone.
#[server]
pub async fn change_password(
    current_password: String,
    new_password: String,
    confirm_password: String,
) -> Result<(), ServerFnError> {
    if new_password != confirm_password {
        return Err(ServerFnError::new("passwords do not match"));
    }
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::auth::change_password(&state.db_pool, user.id, &current_password, &new_password)
        .await
        .map_err(ServerFnError::new)?;
    session_cookies()
        .await?
        .remove(tower_cookies::Cookie::new(crate::auth::SESSION_COOKIE, ""));
    Ok(())
}

#[server]
pub async fn get_daily_new_card_limit() -> Result<Option<i32>, ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::daily_new_card_limit(&state.db_pool, user.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[server]
pub async fn set_daily_new_card_limit(limit: Option<i32>) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    crate::db::set_daily_new_card_limit(&state.db_pool, user.id, limit)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

#[server]
pub async fn user_stats() -> Result<Stats, ServerFnError> {
    let user = require_user().await?;
    let state = expect_context::<crate::state::AppState>();
    let db_stats = crate::db::user_stats(&state.db_pool, user.id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(Stats {
        total_cards: db_stats.total_cards,
        due_now: db_stats.due_now,
        reviews_today: db_stats.reviews_today,
        current_streak_days: db_stats.current_streak_days,
        hardest_cards: db_stats
            .hardest_cards
            .into_iter()
            .map(|c| HardCard {
                id: c.id,
                target_text: c.target_text,
                difficulty: c.difficulty,
                lapses: c.lapses,
            })
            .collect(),
    })
}

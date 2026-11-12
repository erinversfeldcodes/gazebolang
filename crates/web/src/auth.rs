#![cfg(feature = "ssr")]
// Error messages here are already the full user-facing explanation; an
// `# Errors` doc section would just restate them.
#![allow(clippy::missing_errors_doc)]

use argon2::Argon2;
use password_hash::rand_core::OsRng;
use password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::RngExt;
use sqlx::PgPool;
use std::sync::OnceLock;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Hard cap on signups, matching the 4-friend trust model this app is built
/// for. Enforced here rather than as a SQL constraint, since expressing
/// "at most N rows" declaratively needs a trigger — more machinery than
/// this warrants.
pub const MAX_USERS: i64 = 4;

/// Floor on password length at signup and reset. Not a strength meter —
/// just enough to rule out trivially-empty or single-character passwords.
pub const MIN_PASSWORD_LEN: usize = 8;

pub const SESSION_COOKIE: &str = "session";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub email: String,
    pub name: String,
}

fn generate_token() -> String {
    use std::fmt::Write as _;

    let bytes: [u8; 32] = rand::rng().random();
    let mut token = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(token, "{b:02x}").unwrap();
    }
    token
}

/// Builds the session cookie with the exact attributes every sign-in path
/// (signup, login) needs: `path=/`, `http_only`, `secure`, `SameSite=Lax`,
/// a 30-day max age.
#[must_use]
pub fn build_session_cookie(token: String) -> tower_cookies::Cookie<'static> {
    tower_cookies::Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(tower_cookies::cookie::SameSite::Lax)
        .max_age(tower_cookies::cookie::time::Duration::days(30))
        .build()
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| e.to_string())
}

fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Runs [`hash_password`] on a blocking-pool thread rather than inline on
/// the async task — Argon2's whole point is to be slow, and every signup
/// and password change pays that cost. Inline, it ties up a Tokio runtime
/// thread for the duration; on the preview app's single-shared-vCPU Fly
/// machine, concurrent signups/logins during the E2E suite could pile up
/// behind each other badly enough to blow past even a generous test
/// timeout.
async fn hash_password_blocking(password: &str) -> Result<String, String> {
    let password = password.to_string();
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| e.to_string())?
}

/// [`hash_password_blocking`]'s counterpart for [`verify_password`], paid
/// on every login attempt. A panic in the blocking task (never expected)
/// degrades to "didn't verify" rather than propagating, same as any other
/// non-match here.
async fn verify_password_blocking(password: &str, hash: &str) -> bool {
    let password = password.to_string();
    let hash = hash.to_string();
    tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .unwrap_or(false)
}

/// A syntactically valid Argon2 hash with no corresponding real password,
/// computed once per process and reused for every login attempt where the
/// email doesn't match a user, or matches a user who has never set a
/// password. Verifying against it pays the same Argon2 cost a real
/// failed-password attempt would, so a nonexistent/passwordless account
/// can't be distinguished from a wrong password by response timing.
fn dummy_password_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        hash_password("dummy-password-used-only-for-constant-time-login-checks")
            .expect("hashing a fixed dummy password should never fail")
    })
}

fn validate_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        ));
    }
    Ok(())
}

/// Minimal shape check, not a full RFC 5322 validator — just enough to
/// catch obvious typos ("bob", "bob@") before they hit the database.
fn validate_email(email: &str) -> Result<(), String> {
    let domain = email.split_once('@').map(|(_, domain)| domain);
    match domain {
        Some(domain) if !domain.is_empty() && domain.contains('.') => Ok(()),
        _ => Err("please enter a valid email address".to_string()),
    }
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
}

async fn start_session(pool: &PgPool, user_id: Uuid) -> Result<String, String> {
    let session_token = generate_token();
    let expires_at = OffsetDateTime::now_utc() + Duration::days(30);
    sqlx::query("INSERT INTO sessions (token, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(&session_token)
        .bind(user_id)
        .bind(expires_at)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(session_token)
}

/// Creates a new account and signs it in. Rejects a duplicate email, a
/// too-short password, and (for a genuinely new email) the 4-user cap.
/// Returns the new session token on success.
pub async fn sign_up(
    pool: &PgPool,
    email: &str,
    name: &str,
    password: &str,
) -> Result<String, String> {
    let email = email.trim().to_lowercase();
    let name = name.trim();
    validate_email(&email)?;
    if name.is_empty() {
        return Err("please enter a name".to_string());
    }
    validate_password(password)?;

    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
    if existing.is_some() {
        return Err("an account with that email already exists".to_string());
    }

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    if count >= MAX_USERS {
        log::warn!("auth: sign-up rejected at the {MAX_USERS}-user cap for {email}");
        return Err("This app is limited to 4 users and is already full.".to_string());
    }

    let password_hash = hash_password_blocking(password).await?;

    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, password_hash) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&email)
    .bind(name)
    .bind(&password_hash)
    .fetch_one(pool)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            "an account with that email already exists".to_string()
        } else {
            e.to_string()
        }
    })?;

    start_session(pool, user_id).await
}

/// Verifies an email/password pair and starts a session on success.
/// Always returns the same generic error for "no such email", "wrong
/// password", and "account exists but has no password set yet" (the
/// pre-password-reset state of a magic-link-only user) — never revealing
/// which case applies, and always paying the same Argon2 cost via
/// [`dummy_password_hash`] so the three cases can't be told apart by
/// timing either.
pub async fn log_in(pool: &PgPool, email: &str, password: &str) -> Result<String, String> {
    let email = email.trim().to_lowercase();
    let invalid = || "invalid email or password".to_string();

    let row: Option<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE email = $1")
            .bind(&email)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;

    // `user_id` is Some only when there's a real password hash to check
    // against. A user with no password set yet (the pre-reset state of a
    // migrated magic-link account) is treated exactly like "no such
    // account" — both fall through to the dummy hash below, so the two
    // cases can't be told apart by response content or timing.
    let (user_id, hash): (Option<Uuid>, String) = match row {
        Some((id, Some(hash))) => (Some(id), hash),
        Some((_, None)) | None => (None, dummy_password_hash().to_string()),
    };

    if !verify_password_blocking(password, &hash).await {
        log::warn!("auth: failed login attempt for {email}");
        return Err(invalid());
    }

    match user_id {
        Some(id) => start_session(pool, id).await,
        // Only reachable if the submitted password happens to equal the
        // dummy password verbatim (astronomically unlikely) — treat it
        // the same as any other non-match rather than ever signing in.
        None => Err(invalid()),
    }
}

/// Starts a password-reset attempt. Always succeeds from the caller's
/// point of view regardless of whether `email` matches a user — the
/// caller must show the same generic "if an account exists..." message
/// either way, so account existence is never revealed. A single-use,
/// 24-hour token is only actually minted (and emailed) when the email
/// does match.
pub async fn request_password_reset(
    pool: &PgPool,
    email_sender: &crate::email::EmailSender,
    base_url: &str,
    email: &str,
) -> Result<(), String> {
    let email = email.trim().to_lowercase();
    let user_id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    let Some(user_id) = user_id else {
        log::info!("auth: password reset requested for an email with no account");
        return Ok(());
    };

    let token = generate_token();
    let expires_at = OffsetDateTime::now_utc() + Duration::hours(24);
    sqlx::query(
        "INSERT INTO password_reset_tokens (token, user_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(&token)
    .bind(user_id)
    .bind(expires_at)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    let link = format!("{base_url}/reset-password?token={token}");
    if let Err(e) = email_sender.send_password_reset(&email, &link).await {
        // Logged, not propagated: surfacing this as an error would let a
        // caller distinguish "no such account" (silent Ok above) from
        // "account exists but the email bounced", which is exactly the
        // existence leak this function exists to prevent.
        log::error!("auth: failed to send password-reset email to {email}: {e}");
    }
    Ok(())
}

/// Redeems a password-reset token: sets a new password, marks the token
/// used, and revokes every existing session for that user (a reset should
/// log out every other active session, not just the one performing it).
/// This is also how the app's one magic-link-only production user gets
/// their first-ever password: for them `password_hash` starts out `NULL`,
/// which [`log_in`] always treats as a non-match, never a panic.
pub async fn confirm_password_reset(
    pool: &PgPool,
    token: &str,
    new_password: &str,
) -> Result<(), String> {
    validate_password(new_password)?;

    let row: Option<(Uuid, OffsetDateTime, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT user_id, expires_at, used_at FROM password_reset_tokens WHERE token = $1",
    )
    .bind(token)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    let Some((user_id, expires_at, used_at)) = row else {
        log::warn!("auth: password reset attempted with an unknown token");
        return Err("this password reset link is invalid".to_string());
    };
    if used_at.is_some() {
        log::warn!("auth: password reset attempted with an already-used token");
        return Err("this password reset link has already been used".to_string());
    }
    if expires_at < OffsetDateTime::now_utc() {
        log::warn!("auth: password reset attempted with an expired token");
        return Err("this password reset link has expired".to_string());
    }

    let password_hash = hash_password_blocking(new_password).await?;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(&password_hash)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("UPDATE password_reset_tokens SET used_at = now() WHERE token = $1")
        .bind(token)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

/// How long a minted invite stays redeemable. More generous than
/// `confirm_password_reset`'s 24-hour window — this isn't rotating a
/// compromised credential, just getting a friend through signup whenever
/// they get around to it.
const INVITE_EXPIRY_HOURS: i64 = 24 * 7;

/// Mints an invite for `email` from `inviter_id` — any signed-in user may
/// invite, matching this app's small-trusted-group model (no admin/role
/// concept exists, or is needed). Re-checks the same email-uniqueness and
/// `MAX_USERS` conditions [`sign_up`] used to check at its own signup
/// time, since that open path no longer exists — an invite is now the
/// only way a new account gets created. [`sign_up_via_invite`] re-checks
/// `MAX_USERS` again at redemption time, since several outstanding
/// invites could otherwise overrun the cap between mint and redeem — the
/// same check-then-act race [`sign_up`]'s own cap check already accepted
/// as fine for 4 trusted users.
pub async fn create_invite(pool: &PgPool, inviter_id: Uuid, email: &str) -> Result<String, String> {
    let email = email.trim().to_lowercase();
    validate_email(&email)?;

    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
    if existing.is_some() {
        return Err("an account with that email already exists".to_string());
    }

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    if count >= MAX_USERS {
        return Err("This app is limited to 4 users and is already full.".to_string());
    }

    let token = generate_token();
    let expires_at = OffsetDateTime::now_utc() + Duration::hours(INVITE_EXPIRY_HOURS);
    sqlx::query(
        "INSERT INTO invites (token, email, invited_by, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(&token)
    .bind(&email)
    .bind(inviter_id)
    .bind(expires_at)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(token)
}

/// Creates a new account by redeeming an invite token and signs it in.
/// Binds the new account to the invite's own stored email — never an
/// arbitrary form field, since the whole point of an invite is that the
/// email is already decided — otherwise does exactly what the old open
/// [`sign_up`] did: validate the name/password, hash the password, insert
/// the user, start a session.
pub async fn sign_up_via_invite(
    pool: &PgPool,
    token: &str,
    name: &str,
    password: &str,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("please enter a name".to_string());
    }
    validate_password(password)?;

    let row: Option<(String, OffsetDateTime, Option<OffsetDateTime>)> =
        sqlx::query_as("SELECT email, expires_at, used_at FROM invites WHERE token = $1")
            .bind(token)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;

    let Some((email, expires_at, used_at)) = row else {
        return Err("this invite link is invalid".to_string());
    };
    if used_at.is_some() {
        return Err("this invite link has already been used".to_string());
    }
    if expires_at < OffsetDateTime::now_utc() {
        return Err("this invite link has expired".to_string());
    }

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    if count >= MAX_USERS {
        return Err("This app is limited to 4 users and is already full.".to_string());
    }

    let password_hash = hash_password_blocking(password).await?;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, password_hash) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&email)
    .bind(name)
    .bind(&password_hash)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            "an account with that email already exists".to_string()
        } else {
            e.to_string()
        }
    })?;
    sqlx::query("UPDATE invites SET used_at = now() WHERE token = $1")
        .bind(token)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    start_session(pool, user_id).await
}

/// Checks `password` against the signed-in user's actual password hash,
/// falling back to [`dummy_password_hash`] when there isn't one yet (the
/// pre-reset state of a migrated magic-link account) — same timing-safe
/// non-match as [`log_in`], just keyed by user id instead of email since
/// the caller already has an authenticated session.
async fn verify_current_password(
    pool: &PgPool,
    user_id: Uuid,
    password: &str,
) -> Result<(), String> {
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?
        .flatten();
    let hash = hash.unwrap_or_else(|| dummy_password_hash().to_string());

    if verify_password_blocking(password, &hash).await {
        Ok(())
    } else {
        Err("incorrect password".to_string())
    }
}

/// Updates the signed-in user's display name. No current-password check —
/// unlike email/password below, this isn't a credential change, just
/// cosmetic.
pub async fn update_name(pool: &PgPool, user_id: Uuid, new_name: &str) -> Result<(), String> {
    let name = new_name.trim();
    if name.is_empty() {
        return Err("please enter a name".to_string());
    }
    sqlx::query("UPDATE users SET name = $1 WHERE id = $2")
        .bind(name)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Changes the signed-in user's email, after confirming their current
/// password — this is credential-adjacent (it's what logins and password
/// resets go to), so it isn't allowed on session trust alone. Doesn't
/// re-verify the new address by sending a confirmation link, matching this
/// app's small-trusted-group model rather than a public signup's.
pub async fn update_email(
    pool: &PgPool,
    user_id: Uuid,
    current_password: &str,
    new_email: &str,
) -> Result<(), String> {
    verify_current_password(pool, user_id, current_password).await?;
    let email = new_email.trim().to_lowercase();
    validate_email(&email)?;

    sqlx::query("UPDATE users SET email = $1 WHERE id = $2")
        .bind(&email)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                "an account with that email already exists".to_string()
            } else {
                e.to_string()
            }
        })?;
    Ok(())
}

/// Changes the signed-in user's password, after confirming their current
/// one. Revokes every session for the account afterward, including the one
/// making this change — same as redeeming a password-reset token — so the
/// caller should send the user back to sign in with the new password.
pub async fn change_password(
    pool: &PgPool,
    user_id: Uuid,
    current_password: &str,
    new_password: &str,
) -> Result<(), String> {
    verify_current_password(pool, user_id, current_password).await?;
    validate_password(new_password)?;
    let password_hash = hash_password_blocking(new_password).await?;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(&password_hash)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

pub async fn user_for_session(pool: &PgPool, session_token: &str) -> Option<UserRow> {
    sqlx::query_as::<_, UserRow>(
        "SELECT u.id, u.email, u.name FROM sessions s \
         JOIN users u ON u.id = s.user_id \
         WHERE s.token = $1 AND s.expires_at > now()",
    )
    .bind(session_token)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

pub async fn delete_session(pool: &PgPool, session_token: &str) {
    let _ = sqlx::query("DELETE FROM sessions WHERE token = $1")
        .bind(session_token)
        .execute(pool)
        .await;
}

/// Reads the session cookie and resolves it to the signed-in user, if any.
pub async fn current_user(pool: &PgPool, cookies: &tower_cookies::Cookies) -> Option<UserRow> {
    let token = cookies.get(SESSION_COOKIE)?;
    user_for_session(pool, token.value()).await
}

/// Like [`current_user`], but fails loudly — for server functions that
/// require a signed-in user rather than treating one as optional.
pub async fn require_user(
    pool: &PgPool,
    cookies: &tower_cookies::Cookies,
) -> Result<UserRow, String> {
    current_user(pool, cookies)
        .await
        .ok_or_else(|| "not signed in".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn password_hash_for(pool: &PgPool, email: &str) -> Option<String> {
        sqlx::query_scalar("SELECT password_hash FROM users WHERE email = $1")
            .bind(email)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn reset_token_for(pool: &PgPool, email: &str) -> String {
        sqlx::query_scalar(
            "SELECT prt.token FROM password_reset_tokens prt \
             JOIN users u ON u.id = prt.user_id \
             WHERE u.email = $1 ORDER BY prt.created_at DESC LIMIT 1",
        )
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sign_up_creates_user_and_signs_them_in(pool: PgPool) {
        let session_token = sign_up(&pool, "New@Example.com", "Nina", "correct-horse-battery")
            .await
            .unwrap();

        let user = user_for_session(&pool, &session_token).await.unwrap();
        assert_eq!(user.email, "new@example.com", "email should be lowercased");
        assert_eq!(user.name, "Nina");
        assert!(password_hash_for(&pool, "new@example.com").await.is_some());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sign_up_rejects_a_short_password(pool: PgPool) {
        let result = sign_up(&pool, "short@example.com", "Shorty", "short1").await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sign_up_rejects_a_duplicate_email(pool: PgPool) {
        sign_up(&pool, "dup@example.com", "First", "correct-horse-battery")
            .await
            .unwrap();
        let result = sign_up(&pool, "Dup@Example.com", "Second", "correct-horse-battery").await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn fifth_new_signup_is_rejected_once_at_the_cap(pool: PgPool) {
        for i in 0..MAX_USERS {
            let email = format!("user{i}@example.com");
            sign_up(&pool, &email, "User", "correct-horse-battery")
                .await
                .unwrap_or_else(|e| panic!("user {i} should be allowed to sign up: {e}"));
        }

        let fifth = sign_up(&pool, "user4@example.com", "User", "correct-horse-battery").await;
        assert!(
            fifth.is_err(),
            "a 5th new signup should be rejected at the cap"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn log_in_succeeds_with_the_right_password(pool: PgPool) {
        sign_up(&pool, "login@example.com", "Loggy", "correct-horse-battery")
            .await
            .unwrap();

        let session_token = log_in(&pool, "Login@Example.com", "correct-horse-battery")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();
        assert_eq!(user.email, "login@example.com");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn log_in_fails_with_the_wrong_password(pool: PgPool) {
        sign_up(
            &pool,
            "wrong@example.com",
            "Wrongy",
            "correct-horse-battery",
        )
        .await
        .unwrap();

        let result = log_in(&pool, "wrong@example.com", "not-the-password").await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn log_in_fails_for_an_unknown_email_with_the_same_generic_error(pool: PgPool) {
        let known_err = {
            sign_up(&pool, "known@example.com", "Known", "correct-horse-battery")
                .await
                .unwrap();
            log_in(&pool, "known@example.com", "wrong-password")
                .await
                .unwrap_err()
        };
        let unknown_err = log_in(&pool, "nobody@example.com", "whatever-password")
            .await
            .unwrap_err();
        assert_eq!(
            known_err, unknown_err,
            "a wrong password and an unknown email must produce the same error"
        );
    }

    /// The migration path for the app's one existing production user:
    /// they exist with `password_hash = NULL` (a magic-link-only account
    /// predating this migration), and logging in against that must fail
    /// gracefully rather than panicking or ever matching.
    #[sqlx::test(migrations = "../../migrations")]
    async fn log_in_against_a_null_password_hash_fails_gracefully(pool: PgPool) {
        sqlx::query("INSERT INTO users (email, name) VALUES ($1, $2)")
            .bind("legacy@example.com")
            .bind("Legacy User")
            .execute(&pool)
            .await
            .unwrap();

        let result = log_in(&pool, "legacy@example.com", "any-password-at-all").await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn requesting_a_reset_for_an_unknown_email_mints_no_token_but_still_succeeds(
        pool: PgPool,
    ) {
        let sender = crate::email::EmailSender::Console;
        let result = request_password_reset(
            &pool,
            &sender,
            "http://localhost:3000",
            "nobody@example.com",
        )
        .await;
        assert!(result.is_ok());

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM password_reset_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn requesting_a_reset_for_a_known_email_mints_a_token(pool: PgPool) {
        sign_up(
            &pool,
            "reset@example.com",
            "Resetter",
            "correct-horse-battery",
        )
        .await
        .unwrap();

        let sender = crate::email::EmailSender::Console;
        request_password_reset(&pool, &sender, "http://localhost:3000", "Reset@Example.com")
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM password_reset_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    /// The scenario with real production stakes: a user who signed up via
    /// magic link and has never had a password (`password_hash = NULL`)
    /// completes a password reset, and can then log in with the new
    /// password they just set.
    #[sqlx::test(migrations = "../../migrations")]
    async fn password_reset_migrates_a_null_password_hash_user_onto_a_real_password(pool: PgPool) {
        sqlx::query("INSERT INTO users (email, name) VALUES ($1, $2)")
            .bind("migrate@example.com")
            .bind("Migrating User")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            password_hash_for(&pool, "migrate@example.com")
                .await
                .is_none()
        );

        let sender = crate::email::EmailSender::Console;
        request_password_reset(
            &pool,
            &sender,
            "http://localhost:3000",
            "migrate@example.com",
        )
        .await
        .unwrap();
        let token = reset_token_for(&pool, "migrate@example.com").await;

        confirm_password_reset(&pool, &token, "a-brand-new-password")
            .await
            .unwrap();
        assert!(
            password_hash_for(&pool, "migrate@example.com")
                .await
                .is_some()
        );

        let session_token = log_in(&pool, "migrate@example.com", "a-brand-new-password")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();
        assert_eq!(user.email, "migrate@example.com");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn password_reset_token_cannot_be_redeemed_twice(pool: PgPool) {
        sign_up(
            &pool,
            "reuse@example.com",
            "Reuser",
            "correct-horse-battery",
        )
        .await
        .unwrap();
        let sender = crate::email::EmailSender::Console;
        request_password_reset(&pool, &sender, "http://localhost:3000", "reuse@example.com")
            .await
            .unwrap();
        let token = reset_token_for(&pool, "reuse@example.com").await;

        confirm_password_reset(&pool, &token, "first-new-password")
            .await
            .unwrap();
        let second_attempt = confirm_password_reset(&pool, &token, "second-new-password").await;
        assert!(second_attempt.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn expired_reset_token_is_rejected(pool: PgPool) {
        sign_up(
            &pool,
            "expired@example.com",
            "Expirer",
            "correct-horse-battery",
        )
        .await
        .unwrap();
        let user_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
            .bind("expired@example.com")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO password_reset_tokens (token, user_id, expires_at) \
             VALUES ($1, $2, now() - interval '1 hour')",
        )
        .bind("already-expired-token")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

        let result =
            confirm_password_reset(&pool, "already-expired-token", "new-password-here").await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn unknown_reset_token_is_rejected(pool: PgPool) {
        let result = confirm_password_reset(&pool, "never-issued", "new-password-here").await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn password_reset_revokes_existing_sessions(pool: PgPool) {
        let session_token = sign_up(
            &pool,
            "revoke@example.com",
            "Revoker",
            "correct-horse-battery",
        )
        .await
        .unwrap();
        assert!(user_for_session(&pool, &session_token).await.is_some());

        let sender = crate::email::EmailSender::Console;
        request_password_reset(
            &pool,
            &sender,
            "http://localhost:3000",
            "revoke@example.com",
        )
        .await
        .unwrap();
        let token = reset_token_for(&pool, "revoke@example.com").await;
        confirm_password_reset(&pool, &token, "a-different-password")
            .await
            .unwrap();

        assert!(
            user_for_session(&pool, &session_token).await.is_none(),
            "the old session should be revoked by the reset"
        );
    }

    async fn invite_token_for(pool: &PgPool, email: &str) -> String {
        sqlx::query_scalar(
            "SELECT token FROM invites WHERE email = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_invite_mints_a_token_for_a_new_email(pool: PgPool) {
        let inviter = sign_up(&pool, "inviter@example.com", "Inviter", "correct-horse")
            .await
            .unwrap();
        let inviter_id = user_for_session(&pool, &inviter).await.unwrap().id;

        create_invite(&pool, inviter_id, "Friend@Example.com")
            .await
            .unwrap();

        let token = invite_token_for(&pool, "friend@example.com").await;
        assert_eq!(token.len(), 64, "generate_token() produces 32 hex bytes");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_invite_rejects_an_email_that_already_has_an_account(pool: PgPool) {
        let inviter = sign_up(&pool, "inviter2@example.com", "Inviter", "correct-horse")
            .await
            .unwrap();
        let inviter_id = user_for_session(&pool, &inviter).await.unwrap().id;
        sign_up(&pool, "existing@example.com", "Existing", "correct-horse")
            .await
            .unwrap();

        let result = create_invite(&pool, inviter_id, "existing@example.com").await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_invite_is_rejected_once_at_the_cap(pool: PgPool) {
        let mut inviter_id = Uuid::nil();
        for i in 0..MAX_USERS {
            let email = format!("capuser{i}@example.com");
            let session = sign_up(&pool, &email, "User", "correct-horse-battery")
                .await
                .unwrap_or_else(|e| panic!("user {i} should be allowed to sign up: {e}"));
            inviter_id = user_for_session(&pool, &session).await.unwrap().id;
        }

        let result = create_invite(&pool, inviter_id, "onemore@example.com").await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sign_up_via_invite_creates_user_bound_to_the_invites_email(pool: PgPool) {
        let inviter = sign_up(&pool, "inviter3@example.com", "Inviter", "correct-horse")
            .await
            .unwrap();
        let inviter_id = user_for_session(&pool, &inviter).await.unwrap().id;
        create_invite(&pool, inviter_id, "newfriend@example.com")
            .await
            .unwrap();
        let token = invite_token_for(&pool, "newfriend@example.com").await;

        let session_token =
            sign_up_via_invite(&pool, &token, "New Friend", "correct-horse-battery")
                .await
                .unwrap();

        let user = user_for_session(&pool, &session_token).await.unwrap();
        assert_eq!(user.email, "newfriend@example.com");
        assert_eq!(user.name, "New Friend");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sign_up_via_invite_rejects_an_unknown_token(pool: PgPool) {
        let result = sign_up_via_invite(&pool, "never-issued", "Someone", "correct-horse").await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sign_up_via_invite_rejects_an_already_used_token(pool: PgPool) {
        let inviter = sign_up(&pool, "inviter4@example.com", "Inviter", "correct-horse")
            .await
            .unwrap();
        let inviter_id = user_for_session(&pool, &inviter).await.unwrap().id;
        create_invite(&pool, inviter_id, "reused@example.com")
            .await
            .unwrap();
        let token = invite_token_for(&pool, "reused@example.com").await;

        sign_up_via_invite(&pool, &token, "First", "correct-horse-battery")
            .await
            .unwrap();
        let second_attempt =
            sign_up_via_invite(&pool, &token, "Second", "correct-horse-battery").await;
        assert!(second_attempt.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn sign_up_via_invite_rejects_an_expired_token(pool: PgPool) {
        let inviter = sign_up(&pool, "inviter5@example.com", "Inviter", "correct-horse")
            .await
            .unwrap();
        let inviter_id = user_for_session(&pool, &inviter).await.unwrap().id;
        sqlx::query(
            "INSERT INTO invites (token, email, invited_by, expires_at) \
             VALUES ($1, $2, $3, now() - interval '1 hour')",
        )
        .bind("already-expired-invite")
        .bind("expired-invitee@example.com")
        .bind(inviter_id)
        .execute(&pool)
        .await
        .unwrap();

        let result = sign_up_via_invite(
            &pool,
            "already-expired-invite",
            "Someone",
            "correct-horse-battery",
        )
        .await;
        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deleted_session_no_longer_resolves_to_a_user(pool: PgPool) {
        let session_token = sign_up(
            &pool,
            "logout@example.com",
            "Loggerouter",
            "correct-horse-battery",
        )
        .await
        .unwrap();

        assert!(user_for_session(&pool, &session_token).await.is_some());
        delete_session(&pool, &session_token).await;
        assert!(user_for_session(&pool, &session_token).await.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_name_changes_the_display_name(pool: PgPool) {
        let session_token = sign_up(&pool, "name@example.com", "Old Name", "correct-horse")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();

        update_name(&pool, user.id, "New Name").await.unwrap();

        let user = user_for_session(&pool, &session_token).await.unwrap();
        assert_eq!(user.name, "New Name");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_name_rejects_an_empty_name(pool: PgPool) {
        let session_token = sign_up(&pool, "empty@example.com", "Someone", "correct-horse")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();

        assert!(update_name(&pool, user.id, "   ").await.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_email_requires_the_correct_current_password(pool: PgPool) {
        let session_token = sign_up(&pool, "old@example.com", "Someone", "correct-horse")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();

        let result = update_email(&pool, user.id, "wrong-password", "new@example.com").await;

        assert!(result.is_err());
        let user = user_for_session(&pool, &session_token).await.unwrap();
        assert_eq!(user.email, "old@example.com", "email should be unchanged");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_email_changes_it_with_the_right_password(pool: PgPool) {
        let session_token = sign_up(&pool, "old2@example.com", "Someone", "correct-horse")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();

        update_email(&pool, user.id, "correct-horse", "New2@Example.com")
            .await
            .unwrap();

        let user = user_for_session(&pool, &session_token).await.unwrap();
        assert_eq!(user.email, "new2@example.com", "email should be lowercased");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_email_rejects_a_duplicate(pool: PgPool) {
        sign_up(&pool, "taken@example.com", "First", "correct-horse")
            .await
            .unwrap();
        let session_token = sign_up(&pool, "second@example.com", "Second", "correct-horse")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();

        let result = update_email(&pool, user.id, "correct-horse", "taken@example.com").await;

        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn update_email_fails_against_a_null_password_hash_user(pool: PgPool) {
        let user_id: Uuid =
            sqlx::query_scalar("INSERT INTO users (email, name) VALUES ($1, $2) RETURNING id")
                .bind("migrated@example.com")
                .bind("Migrating User")
                .fetch_one(&pool)
                .await
                .unwrap();

        let result = update_email(&pool, user_id, "whatever", "new@example.com").await;

        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn change_password_requires_the_correct_current_password(pool: PgPool) {
        let session_token = sign_up(&pool, "cp1@example.com", "Someone", "correct-horse")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();

        let result = change_password(&pool, user.id, "wrong-password", "new-password-123").await;

        assert!(result.is_err());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn change_password_allows_logging_in_with_the_new_password(pool: PgPool) {
        let session_token = sign_up(&pool, "cp2@example.com", "Someone", "correct-horse")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();

        change_password(&pool, user.id, "correct-horse", "new-password-123")
            .await
            .unwrap();

        assert!(
            log_in(&pool, "cp2@example.com", "correct-horse")
                .await
                .is_err()
        );
        assert!(
            log_in(&pool, "cp2@example.com", "new-password-123")
                .await
                .is_ok()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn change_password_revokes_every_existing_session(pool: PgPool) {
        let session_token = sign_up(&pool, "cp3@example.com", "Someone", "correct-horse")
            .await
            .unwrap();
        let user = user_for_session(&pool, &session_token).await.unwrap();

        change_password(&pool, user.id, "correct-horse", "new-password-123")
            .await
            .unwrap();

        assert!(
            user_for_session(&pool, &session_token).await.is_none(),
            "the session active during the change should be revoked too"
        );
    }

    proptest::proptest! {
        // Argon2 is deliberately slow (that's the whole point of using
        // it for passwords) — far fewer cases than proptest's default
        // keeps this from dominating the test suite's runtime, without
        // weakening what the property itself actually checks.
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(10))]

        /// The one thing `hash_password`/`verify_password` exist for:
        /// a password must always verify successfully against a fresh
        /// hash of itself, for any password.
        #[test]
        fn hash_password_then_verify_password_always_succeeds(password in ".{1,100}") {
            let hash = hash_password(&password).unwrap();
            proptest::prop_assert!(verify_password(&password, &hash));
        }
    }

    proptest::proptest! {
        /// Login passes `verify_password` whatever's actually stored in
        /// the `password_hash` column — it must degrade to "doesn't
        /// verify" rather than panic on anything that isn't a
        /// well-formed Argon2 hash. No real Argon2 work happens on the
        /// parse-failure path, so this can run at the default case
        /// count cheaply.
        #[test]
        fn verify_password_never_panics_on_a_malformed_hash(password in ".*", garbage_hash in ".*") {
            let _ = verify_password(&password, &garbage_hash);
        }

        /// `validate_password` counts Unicode *characters*, not bytes —
        /// generating from a multi-byte range (Latin-1 Supplement /
        /// Extended-A, e.g. "é", "ñ") specifically exercises the case
        /// where byte length and char count diverge, which an
        /// ASCII-only input never would.
        #[test]
        fn validate_password_counts_multi_byte_characters_as_one_each(
            password in proptest::strategy::Strategy::prop_map(
                proptest::collection::vec(proptest::char::range('\u{00C0}', '\u{024F}'), 0..20),
                |chars| chars.into_iter().collect::<String>(),
            ),
        ) {
            let char_count = password.chars().count();
            proptest::prop_assert_eq!(validate_password(&password).is_ok(), char_count >= MIN_PASSWORD_LEN);
        }

        /// Anything shaped like `local@domain.tld` must be accepted —
        /// this is a shape check, not a real-deliverability check.
        #[test]
        fn validate_email_accepts_anything_shaped_like_an_email(
            local in "[a-zA-Z0-9]{1,10}",
            domain in "[a-zA-Z0-9]{1,10}",
            tld in "[a-zA-Z]{2,5}",
        ) {
            let email = format!("{local}@{domain}.{tld}");
            proptest::prop_assert!(validate_email(&email).is_ok());
        }

        /// No `@` at all must always be rejected.
        #[test]
        fn validate_email_rejects_anything_without_an_at_sign(s in "[a-zA-Z0-9]{0,20}") {
            proptest::prop_assert!(validate_email(&s).is_err());
        }

        /// An `@` with no `.` anywhere after it must always be
        /// rejected — a domain needs a TLD.
        #[test]
        fn validate_email_rejects_a_domain_with_no_dot(
            local in "[a-zA-Z0-9]{1,10}",
            domain in "[a-zA-Z0-9]{1,10}",
        ) {
            let email = format!("{local}@{domain}");
            proptest::prop_assert!(validate_email(&email).is_err());
        }
    }
}

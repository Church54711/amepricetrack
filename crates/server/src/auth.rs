//! Accounts and sign-in. The website uses a cookie; the phone app sends a
//! token in the Authorization header. Both are random strings whose SHA-256
//! is stored, so a copy of the database cannot be used to sign in.

use crate::db::{self, Db, User};
use crate::App;
use anyhow::{anyhow, Result};
use argon2::password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::{header, request::Parts, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use sha2::{Digest, Sha256};

pub const COOKIE: &str = "apt_session";
/// Website sign-ins last 30 days; app tokens last until revoked.
pub const WEB_SESSION_SECONDS: i64 = 30 * db::DAY;

pub async fn hash_password(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|e| anyhow!("could not hash the password: {e}"))
    })
    .await?
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Creates a sign-in and returns the secret token to hand to the browser or app.
pub async fn create_session(db: &Db, user_id: i64, kind: &str, label: Option<&str>) -> Result<String> {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    let token = hex::encode(bytes);
    let ts = db::now();
    sqlx::query("INSERT INTO sessions (user_id, token_hash, kind, label, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(user_id)
        .bind(hash_token(&token))
        .bind(kind)
        .bind(label)
        .bind(ts)
        .bind((kind == "web").then_some(ts + WEB_SESSION_SECONDS))
        .execute(db)
        .await?;
    Ok(token)
}

pub async fn delete_session(db: &Db, token: &str) -> Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?").bind(hash_token(token)).execute(db).await?;
    Ok(())
}

async fn user_for_token(db: &Db, token: &str) -> Result<Option<User>> {
    let hash = hash_token(token);
    let ts = db::now();
    let user: Option<User> = sqlx::query_as(
        "SELECT u.* FROM users u JOIN sessions s ON s.user_id = u.id
         WHERE s.token_hash = ? AND (s.expires_at IS NULL OR s.expires_at > ?)",
    )
    .bind(&hash)
    .bind(ts)
    .fetch_optional(db)
    .await?;
    if user.is_some() {
        // Only touch the row every few minutes; this runs on every request.
        sqlx::query("UPDATE sessions SET last_used_at = ? WHERE token_hash = ? AND (last_used_at IS NULL OR last_used_at < ?)")
            .bind(ts)
            .bind(&hash)
            .bind(ts - 300)
            .execute(db)
            .await?;
    }
    Ok(user)
}

pub fn cookie_value(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.to_string())
}

pub fn session_cookie(token: &str, secure: bool) -> String {
    format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={WEB_SESSION_SECONDS}{}",
        if secure { "; Secure" } else { "" }
    )
}

pub fn clear_cookie() -> String {
    format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// The signed-in user. Pages redirect to the sign-in form without one;
/// API calls get a 401.
pub struct CurrentUser(pub User);

pub enum AuthRejection {
    ToLogin,
    ToSetup,
    Unauthorized,
    Failed,
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        match self {
            AuthRejection::ToLogin => Redirect::to("/login").into_response(),
            AuthRejection::ToSetup => Redirect::to("/setup").into_response(),
            AuthRejection::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({ "error": "Sign in first: the token is missing, wrong or was revoked." })),
            )
                .into_response(),
            AuthRejection::Failed => (StatusCode::INTERNAL_SERVER_ERROR, "Could not check the sign-in.").into_response(),
        }
    }
}

impl FromRequestParts<App> for CurrentUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let is_api = parts.uri.path().starts_with("/api/");
        let bearer = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|v| v.trim().to_string());
        let token = if is_api { bearer.or_else(|| cookie_value(parts)) } else { cookie_value(parts) };

        if let Some(token) = token {
            match user_for_token(&app.db, &token).await {
                Ok(Some(user)) => return Ok(CurrentUser(user)),
                Ok(None) => {}
                Err(e) => {
                    tracing::error!("sign-in check failed: {e:#}");
                    return Err(AuthRejection::Failed);
                }
            }
        }
        if is_api {
            return Err(AuthRejection::Unauthorized);
        }
        // A brand-new server has no accounts yet: send the first visitor to create one.
        match db::user_count(&app.db).await {
            Ok(0) => Err(AuthRejection::ToSetup),
            _ => Err(AuthRejection::ToLogin),
        }
    }
}

/// Checks a proposed username and password; returns a message to show when unacceptable.
pub fn validate_new_account(username: &str, password: &str) -> Result<(), &'static str> {
    let name = username.trim();
    if name.len() < 2 || name.len() > 40 {
        return Err("The username must be 2 to 40 characters long.");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return Err("The username may only contain letters, digits, dots, dashes and underscores.");
    }
    if password.chars().count() < 8 {
        return Err("The password must be at least 8 characters long.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn password_round_trip() {
        let hash = hash_password("correct horse".into()).await.unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(verify_password("correct horse".into(), hash.clone()).await);
        assert!(!verify_password("wrong".into(), hash).await);
        assert!(!verify_password("x".into(), "not a hash".into()).await);
    }

    #[tokio::test]
    async fn sessions_sign_in_and_out() {
        let db = db::open_in_memory().await;
        let id = db::create_user(&db, "sam", "hash", true).await.unwrap();
        let token = create_session(&db, id, "web", None).await.unwrap();
        assert_eq!(user_for_token(&db, &token).await.unwrap().unwrap().username, "sam");
        assert!(user_for_token(&db, "bogus").await.unwrap().is_none());
        delete_session(&db, &token).await.unwrap();
        assert!(user_for_token(&db, &token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn expired_sessions_are_refused() {
        let db = db::open_in_memory().await;
        let id = db::create_user(&db, "sam", "hash", true).await.unwrap();
        let token = create_session(&db, id, "web", None).await.unwrap();
        sqlx::query("UPDATE sessions SET expires_at = 1").execute(&db).await.unwrap();
        assert!(user_for_token(&db, &token).await.unwrap().is_none());
    }

    #[test]
    fn account_rules() {
        assert!(validate_new_account("sam", "longenough").is_ok());
        assert!(validate_new_account("s", "longenough").is_err());
        assert!(validate_new_account("sam smith", "longenough").is_err());
        assert!(validate_new_account("sam", "short").is_err());
    }
}

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap},
    middleware::Next,
    response::{IntoResponse, Response},
};
use camino::Utf8PathBuf;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Configuration for a single user, loaded from users.toml.
///
/// Example users.toml:
/// ```toml
/// [[users]]
/// username = "alice"
/// password_hash = "5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8"
/// config_dir = "/home/alice/.config/cook"  # optional
///
/// [[users]]
/// username = "bob"
/// password_hash = "6b86b273ff34fce19d6b804eff5a3f5747ada4eaa22f1d49c01e52ddb7875b4b"
/// ```
///
/// The `password_hash` is the hex-encoded SHA-256 digest of the plain-text password.
/// `config_dir` is an optional path to a directory containing user-specific `aisle.conf`
/// and `pantry.conf` files. When set, these override the server-wide configuration.
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)] // config_dir is stored for future per-user config support
pub struct UserConfig {
    pub username: String,
    pub password_hash: String,
    pub config_dir: Option<Utf8PathBuf>,
}

#[derive(Debug, Deserialize, Default)]
struct UsersConfig {
    #[serde(default)]
    users: Vec<UserConfig>,
}

/// Compute the hex-encoded SHA-256 hash of a password.
pub fn hash_password(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Return `true` when `password` matches the stored `hash`.
pub fn verify_password(password: &str, hash: &str) -> bool {
    hash_password(password) == hash
}

/// Generate a 32-byte cryptographically random session token encoded as hex.
pub fn generate_session_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.gen();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Extract the value of the `cook_session` cookie from request headers.
pub fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?;
    let cookie_str = cookie_header.to_str().ok()?;
    for cookie in cookie_str.split(';') {
        let cookie = cookie.trim();
        if let Some(val) = cookie.strip_prefix("cook_session=") {
            return Some(val.to_string());
        }
    }
    None
}

/// Load users from a TOML file.  Returns an empty list when the file does not exist.
pub fn load_users(path: &std::path::Path) -> anyhow::Result<Vec<UserConfig>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path)?;
    let config: UsersConfig = toml::from_str(&content)?;
    Ok(config.users)
}

/// Look up the username currently associated with the session cookie, if any.
pub fn get_logged_in_user(
    headers: &HeaderMap,
    sessions: &Arc<Mutex<HashMap<String, String>>>,
) -> Option<String> {
    let token = extract_session_cookie(headers)?;
    let sessions = sessions.lock().unwrap();
    sessions.get(&token).cloned()
}

/// Axum middleware that enforces authentication for protected routes.
///
/// * When no users are configured the middleware is a no-op.
/// * When users are configured every request must carry a valid `cook_session`
///   cookie; otherwise the browser is redirected to `/login`.
pub async fn auth_middleware(
    State(state): State<Arc<crate::server::AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if state.users.is_empty() {
        return next.run(request).await;
    }

    let session_token = extract_session_cookie(request.headers());
    let is_authenticated = session_token
        .map(|token| state.sessions.lock().unwrap().contains_key(&token))
        .unwrap_or(false);

    if is_authenticated {
        next.run(request).await
    } else {
        axum::response::Redirect::to("/login").into_response()
    }
}

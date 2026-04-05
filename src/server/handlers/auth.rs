use crate::server::{auth, AppState};
use axum::{extract::State, http::HeaderMap, response::IntoResponse, Json};
use std::sync::Arc;

/// `GET /api/auth/me`
///
/// Returns the currently authenticated username and whether authentication is
/// required on this server instance.
///
/// * `200 OK`  — always returned when no auth is configured
///   (`{"username": null, "auth_required": false}`)
/// * `200 OK`  — returned when the request carries a valid session cookie
///   (`{"username": "alice", "auth_required": true}`)
/// * `401 Unauthorized` — returned when auth is required but the session is
///   missing or invalid
pub async fn auth_me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if state.users.is_empty() {
        return Json(serde_json::json!({
            "username": null,
            "auth_required": false
        }))
        .into_response();
    }

    match auth::get_logged_in_user(&headers, &state.sessions) {
        Some(username) => Json(serde_json::json!({
            "username": username,
            "auth_required": true
        }))
        .into_response(),
        None => axum::http::StatusCode::UNAUTHORIZED.into_response(),
    }
}

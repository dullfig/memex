use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;

use crate::state::AppState;

/// Extracts the namespace from the `X-Memex-Namespace` header.
pub struct Namespace(pub String);

impl FromRequestParts<AppState> for Namespace {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &AppState) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("X-Memex-Namespace")
            .and_then(|v| v.to_str().ok())
            .map(|s| Namespace(s.to_owned()))
            .ok_or((StatusCode::BAD_REQUEST, "missing X-Memex-Namespace header"))
    }
}

/// Extracts the actor ID from the `X-Memex-Actor` header.
pub struct ActorId(pub String);

impl FromRequestParts<AppState> for ActorId {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &AppState) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("X-Memex-Actor")
            .and_then(|v| v.to_str().ok())
            .map(|s| ActorId(s.to_owned()))
            .ok_or((StatusCode::BAD_REQUEST, "missing X-Memex-Actor header"))
    }
}

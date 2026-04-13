use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use memex_audit::AuditFilter;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(query_audit))
        .route("/verify", get(verify_chain))
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    namespace: Option<String>,
    actor: Option<String>,
    action: Option<String>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    limit: Option<u64>,
    offset: Option<u64>,
}

#[derive(Debug, Serialize)]
struct AuditEntryDto {
    seq: u64,
    timestamp: String,
    action: serde_json::Value,
    actor: String,
    namespace: String,
    hash: String,
}

#[derive(Debug, Serialize)]
struct AuditQueryResponse {
    entries: Vec<AuditEntryDto>,
    count: usize,
}

#[derive(Debug, Deserialize)]
struct VerifyQuery {
    from: u64,
    to: u64,
}

#[derive(Debug, Serialize)]
struct VerifyResponse {
    valid: bool,
    from: u64,
    to: u64,
}

async fn query_audit(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<AuditQueryResponse>, ApiError> {
    let filter = AuditFilter {
        namespace: q.namespace,
        actor: q.actor,
        action_type: q.action,
        from: q.from,
        to: q.to,
        limit: q.limit,
        offset: q.offset,
    };

    let entries = state
        .audit_log
        .query(&filter)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let dtos: Vec<AuditEntryDto> = entries
        .iter()
        .map(|e| AuditEntryDto {
            seq: e.seq,
            timestamp: e.timestamp.to_rfc3339(),
            action: serde_json::to_value(&e.action).unwrap_or_default(),
            actor: e.actor.clone(),
            namespace: e.namespace.clone(),
            hash: hex::encode(e.hash),
        })
        .collect();

    let count = dtos.len();
    Ok(Json(AuditQueryResponse {
        entries: dtos,
        count,
    }))
}

async fn verify_chain(
    State(state): State<AppState>,
    Query(q): Query<VerifyQuery>,
) -> Result<Json<VerifyResponse>, ApiError> {
    let valid = state
        .audit_log
        .verify_chain(q.from, q.to)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(VerifyResponse {
        valid,
        from: q.from,
        to: q.to,
    }))
}

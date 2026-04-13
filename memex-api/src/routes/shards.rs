use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use memex_audit::entry::AuditAction;
use memex_shards::ShardId;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::extract::Namespace;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_shard))
        .route("/", get(list_shards))
        .route("/{shard_id}", get(get_shard))
        .route("/{shard_id}", delete(evict_shard))
        .route("/{shard_id}/load", post(load_shard))
}

#[derive(Debug, Deserialize)]
struct CreateShardRequest {
    /// Shard key in `namespace.category.entity_id` format.
    shard: String,
    #[serde(default)]
    pinned: bool,
}

#[derive(Debug, Serialize)]
struct ShardDto {
    shard: String,
    state: String,
    created_at: String,
    token_count: u64,
    byte_size: u64,
    pinned: bool,
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    namespace: Option<String>,
}

async fn create_shard(
    State(state): State<AppState>,
    Namespace(ns): Namespace,
    Json(req): Json<CreateShardRequest>,
) -> Result<Json<ShardDto>, ApiError> {
    let shard_id = ShardId::parse(&req.shard)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid shard key: {}", req.shard)))?;

    if shard_id.namespace != ns {
        return Err(ApiError::BadRequest(
            "shard namespace does not match X-Memex-Namespace header".into(),
        ));
    }

    let meta = state
        .shard_manager
        .create(shard_id, req.pinned)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let _ = state
        .audit_log
        .append(
            AuditAction::ShardCreate {
                shard: meta.id.to_string(),
            },
            "system",
            &ns,
            serde_json::json!({}),
        )
        .await;

    Ok(Json(meta_to_dto(&meta)))
}

async fn list_shards(
    State(state): State<AppState>,
    Namespace(ns): Namespace,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<ShardDto>>, ApiError> {
    let namespace = query.namespace.unwrap_or(ns);
    let metas = state
        .shard_manager
        .list(&namespace)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(metas.iter().map(meta_to_dto).collect()))
}

async fn get_shard(
    State(state): State<AppState>,
    Path(shard_key): Path<String>,
) -> Result<Json<ShardDto>, ApiError> {
    let shard_id = ShardId::parse(&shard_key)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid shard key: {shard_key}")))?;

    let meta = state
        .shard_manager
        .get_meta(&shard_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("shard not found: {shard_key}")))?;

    Ok(Json(meta_to_dto(&meta)))
}

async fn evict_shard(
    State(state): State<AppState>,
    Path(shard_key): Path<String>,
) -> Result<Json<ShardDto>, ApiError> {
    let shard_id = ShardId::parse(&shard_key)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid shard key: {shard_key}")))?;

    state
        .shard_manager
        .evict(&shard_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let meta = state
        .shard_manager
        .get_meta(&shard_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("shard not found: {shard_key}")))?;

    let _ = state
        .audit_log
        .append(
            AuditAction::ShardEvict {
                shard: shard_key.clone(),
            },
            "system",
            &shard_id.namespace,
            serde_json::json!({}),
        )
        .await;

    Ok(Json(meta_to_dto(&meta)))
}

async fn load_shard(
    State(state): State<AppState>,
    Path(shard_key): Path<String>,
) -> Result<Json<ShardDto>, ApiError> {
    let shard_id = ShardId::parse(&shard_key)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid shard key: {shard_key}")))?;

    state
        .shard_manager
        .ensure_resident(&shard_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let meta = state
        .shard_manager
        .get_meta(&shard_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("shard not found: {shard_key}")))?;

    Ok(Json(meta_to_dto(&meta)))
}

fn meta_to_dto(meta: &memex_shards::ShardMeta) -> ShardDto {
    ShardDto {
        shard: meta.id.to_string(),
        state: format!("{:?}", meta.state),
        created_at: meta.created_at.to_rfc3339(),
        token_count: meta.token_count,
        byte_size: meta.byte_size,
        pinned: meta.pinned,
    }
}

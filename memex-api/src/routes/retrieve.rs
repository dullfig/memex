use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use memex_retrieval::{RetrievalPurpose, RetrievalRequest};
use memex_shards::ShardId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::extract::ActorId;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(retrieve_general))
        .route("/aggregate", post(retrieve_aggregate))
        .route("/crisis-outreach", post(retrieve_crisis))
        .route("/customer-support", post(retrieve_support))
}

#[derive(Debug, Deserialize)]
struct RetrieveHttpRequest {
    query: String,
    /// Shard keys in `namespace.category.entity_id` format.
    shards: Vec<String>,
    #[serde(default = "default_top_k")]
    top_k: u32,
}

fn default_top_k() -> u32 {
    10
}

#[derive(Debug, Serialize)]
struct RetrieveHttpResponse {
    query_id: Uuid,
    hits: Vec<HitDto>,
    shard_count: u32,
}

#[derive(Debug, Serialize)]
struct HitDto {
    shard: String,
    offset: u64,
    length: u32,
    score: f32,
    source_id: Option<String>,
}

async fn retrieve_general(
    State(state): State<AppState>,
    actor: ActorId,
    Json(req): Json<RetrieveHttpRequest>,
) -> Result<Json<RetrieveHttpResponse>, ApiError> {
    do_retrieve(state, actor, req, RetrievalPurpose::General).await
}

async fn retrieve_aggregate(
    State(state): State<AppState>,
    actor: ActorId,
    Json(req): Json<RetrieveHttpRequest>,
) -> Result<Json<RetrieveHttpResponse>, ApiError> {
    do_retrieve(state, actor, req, RetrievalPurpose::Aggregate).await
}

async fn retrieve_crisis(
    State(state): State<AppState>,
    actor: ActorId,
    Json(req): Json<RetrieveHttpRequest>,
) -> Result<Json<RetrieveHttpResponse>, ApiError> {
    do_retrieve(state, actor, req, RetrievalPurpose::CrisisOutreach).await
}

async fn retrieve_support(
    State(state): State<AppState>,
    actor: ActorId,
    Json(req): Json<RetrieveHttpRequest>,
) -> Result<Json<RetrieveHttpResponse>, ApiError> {
    do_retrieve(state, actor, req, RetrievalPurpose::CustomerSupport).await
}

async fn do_retrieve(
    state: AppState,
    actor: ActorId,
    req: RetrieveHttpRequest,
    purpose: RetrievalPurpose,
) -> Result<Json<RetrieveHttpResponse>, ApiError> {
    let shard_ids: Vec<ShardId> = req
        .shards
        .iter()
        .map(|s| {
            ShardId::parse(s)
                .ok_or_else(|| ApiError::BadRequest(format!("invalid shard key: {s}")))
        })
        .collect::<Result<_, _>>()?;

    let result = state
        .retrieval_pipeline
        .retrieve(RetrievalRequest {
            query: req.query,
            shards: shard_ids,
            top_k: req.top_k,
            purpose,
            actor: actor.0,
        })
        .await?;

    let hits = result
        .hits
        .into_iter()
        .map(|h| HitDto {
            shard: h.shard,
            offset: h.offset,
            length: h.length,
            score: h.score,
            source_id: h.source_id,
        })
        .collect();

    Ok(Json(RetrieveHttpResponse {
        query_id: result.query_id,
        hits,
        shard_count: result.shard_count,
    }))
}

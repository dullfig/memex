use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use memex_consent::ConsentToken;
use memex_ingest::IngestRequest;
use memex_shards::ShardId;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/", post(ingest))
}

#[derive(Debug, Deserialize)]
struct IngestHttpRequest {
    content_id: String,
    content: String,
    /// Pre-tokenized content. If omitted, the pipeline approximates.
    #[serde(default)]
    tokens: Vec<u32>,
    /// Shard key in `namespace.category.entity_id` format.
    shard: String,
    consent_token: ConsentToken,
}

#[derive(Debug, Serialize)]
struct IngestHttpResponse {
    content_id: String,
    shard: String,
    token_count: u64,
    offset: u64,
}

async fn ingest(
    State(state): State<AppState>,
    Json(req): Json<IngestHttpRequest>,
) -> Result<Json<IngestHttpResponse>, ApiError> {
    let shard_id = ShardId::parse(&req.shard)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid shard key: {}", req.shard)))?;

    let result = state
        .ingest_pipeline
        .ingest(IngestRequest {
            content_id: req.content_id,
            content: req.content,
            tokens: req.tokens,
            shard: shard_id,
            consent_token: req.consent_token,
        })
        .await?;

    Ok(Json(IngestHttpResponse {
        content_id: result.content_id,
        shard: result.shard,
        token_count: result.token_count,
        offset: result.offset,
    }))
}

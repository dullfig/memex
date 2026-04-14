use std::sync::Arc;

use memex_audit::AuditLog;
use memex_audit::entry::AuditAction;
use memex_shards::{PositionMap, ShardId, ShardManager};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::position::RetrievalHit;

/// What purpose this retrieval serves (affects audit level and return semantics).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RetrievalPurpose {
    General,
    Aggregate,
    CrisisOutreach,
    CustomerSupport,
}

/// Request to retrieve from the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalRequest {
    pub query: String,
    pub shards: Vec<ShardId>,
    pub top_k: u32,
    pub purpose: RetrievalPurpose,
    pub actor: String,
}

/// Response from a retrieval query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalResponse {
    pub query_id: Uuid,
    pub hits: Vec<RetrievalHit>,
    pub shard_count: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum RetrievalError {
    #[error("shards span multiple namespaces — namespace isolation violated")]
    NamespaceMismatch,
    #[error("shard not found: {0}")]
    ShardNotFound(String),
    #[error("cortex unavailable")]
    CortexUnavailable,
    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

/// Orchestrates retrieval: validate → ensure resident → cortex query → resolve positions → audit.
pub struct RetrievalPipeline {
    shards: Arc<ShardManager>,
    positions: Arc<PositionMap>,
    audit: Arc<AuditLog>,
}

impl RetrievalPipeline {
    pub fn new(
        shards: Arc<ShardManager>,
        positions: Arc<PositionMap>,
        audit: Arc<AuditLog>,
    ) -> Self {
        Self {
            shards,
            positions,
            audit,
        }
    }

    pub async fn retrieve(
        &self,
        req: RetrievalRequest,
    ) -> Result<RetrievalResponse, RetrievalError> {
        // 1. Validate namespace isolation.
        if !req.shards.is_empty() {
            let ns = &req.shards[0].namespace;
            if req.shards.iter().any(|s| s.namespace != *ns) {
                return Err(RetrievalError::NamespaceMismatch);
            }
        }

        // 2. Ensure all shards exist and are resident.
        for shard_id in &req.shards {
            let meta = self
                .shards
                .get_meta(shard_id)
                .await
                .map_err(RetrievalError::Internal)?;
            if meta.is_none() {
                return Err(RetrievalError::ShardNotFound(shard_id.to_string()));
            }
            self.shards
                .ensure_resident(shard_id)
                .await
                .map_err(RetrievalError::Internal)?;
        }

        // 3. Query cortex for raw attention hits via ShardManager.
        let cortex_resp = self
            .shards
            .retrieve(&req.shards, &req.query, req.top_k)
            .await
            .map_err(RetrievalError::Internal)?;

        // 4. Resolve positions to source content IDs.
        let mut hits = Vec::with_capacity(cortex_resp.spans.len());
        for span in &cortex_resp.spans {
            let shard_id = ShardId::parse(&span.shard);
            let source_id = if let Some(sid) = &shard_id {
                self.positions
                    .resolve(sid, span.offset)
                    .ok()
                    .flatten()
                    .map(|r| r.content_id)
            } else {
                None
            };

            hits.push(RetrievalHit {
                shard: span.shard.clone(),
                offset: span.offset,
                length: 0, // cortex returns individual positions, not spans
                score: span.score,
                source_id,
            });
        }

        let query_id = Uuid::new_v4();

        // 5. Audit.
        let query_hash = {
            let mut h = Sha256::new();
            h.update(req.query.as_bytes());
            hex::encode(h.finalize())
        };

        let namespace = req
            .shards
            .first()
            .map(|s| s.namespace.clone())
            .unwrap_or_default();

        let _ = self
            .audit
            .append(
                AuditAction::Retrieve {
                    shards: req.shards.iter().map(|s| s.to_string()).collect(),
                    query_hash,
                    hit_count: hits.len() as u32,
                },
                &req.actor,
                &namespace,
                serde_json::json!({
                    "purpose": req.purpose,
                    "query_id": query_id,
                    "corpus_tokens": cortex_resp.corpus_tokens,
                }),
            )
            .await;

        Ok(RetrievalResponse {
            query_id,
            hits,
            shard_count: req.shards.len() as u32,
        })
    }
}

use std::sync::Arc;

use memex_audit::AuditLog;
use memex_audit::entry::AuditAction;
use memex_consent::{ConsentToken, ConsentVerifier};
use memex_shards::{PositionMap, ShardId, ShardManager};
use serde::{Deserialize, Serialize};

/// Request to ingest content into a shard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRequest {
    /// Opaque content identifier from the caller.
    pub content_id: String,
    /// The text content to ingest.
    pub content: String,
    /// Pre-tokenized content. If empty, the pipeline will use a rough
    /// approximation (whitespace split → placeholder IDs) until a real
    /// tokenizer is wired in.
    #[serde(default)]
    pub tokens: Vec<u32>,
    /// Target shard.
    pub shard: ShardId,
    /// Consent token authorizing this ingestion.
    pub consent_token: ConsentToken,
}

/// Result of a successful ingestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestResult {
    pub content_id: String,
    pub shard: String,
    /// Number of tokens appended.
    pub token_count: u64,
    /// Token offset within the shard where this content starts.
    pub offset: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("consent denied: {0}")]
    ConsentDenied(#[from] memex_consent::ConsentError),
    #[error("shard not found: {0}")]
    ShardNotFound(String),
    #[error("cortex unavailable")]
    CortexUnavailable,
    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

/// Orchestrates the ingestion pipeline: consent → forward pass → shard append → audit.
pub struct IngestPipeline {
    shards: Arc<ShardManager>,
    consent: Arc<dyn ConsentVerifier>,
    positions: Arc<PositionMap>,
    audit: Arc<AuditLog>,
}

impl IngestPipeline {
    pub fn new(
        shards: Arc<ShardManager>,
        consent: Arc<dyn ConsentVerifier>,
        positions: Arc<PositionMap>,
        audit: Arc<AuditLog>,
    ) -> Self {
        Self {
            shards,
            consent,
            positions,
            audit,
        }
    }

    pub async fn ingest(&self, req: IngestRequest) -> Result<IngestResult, IngestError> {
        // 1. Verify consent.
        self.consent.verify(&req.consent_token).await?;

        // 2. Ensure shard exists.
        let meta = self
            .shards
            .get_meta(&req.shard)
            .await
            .map_err(IngestError::Internal)?
            .ok_or_else(|| IngestError::ShardNotFound(req.shard.to_string()))?;

        // 3. Ensure shard is resident on GPU.
        self.shards
            .ensure_resident(&req.shard)
            .await
            .map_err(IngestError::Internal)?;

        // 4. Determine tokens to append.
        //    If caller provided pre-tokenized data, use it.
        //    Otherwise approximate — real tokenization requires cortex
        //    to expose a /v1/tokenize endpoint (TODO).
        let tokens = if !req.tokens.is_empty() {
            req.tokens.clone()
        } else {
            // Rough placeholder: one "token" per ~4 chars.
            // This is wrong but lets the pipeline run end-to-end
            // before cortex exposes tokenization.
            let approx_count = (req.content.len() / 4).max(1);
            vec![0u32; approx_count]
        };

        // Record the offset before appending.
        let offset = meta.token_count;
        let token_count = tokens.len() as u64;

        // 5. Append tokens to shard (sled + cortex).
        self.shards
            .append_tokens(&req.shard, &tokens)
            .await
            .map_err(IngestError::Internal)?;

        // 6. Record position-to-source mapping.
        self.positions
            .record(&req.shard, offset, token_count as u32, &req.content_id)
            .map_err(IngestError::Internal)?;

        // 7. Audit.
        let _ = self
            .audit
            .append(
                AuditAction::Ingest {
                    shard: req.shard.to_string(),
                    content_id: req.content_id.clone(),
                },
                &req.consent_token.source_entity,
                &req.shard.namespace,
                serde_json::json!({ "token_count": token_count }),
            )
            .await;

        Ok(IngestResult {
            content_id: req.content_id,
            shard: req.shard.to_string(),
            token_count,
            offset,
        })
    }
}

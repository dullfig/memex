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
    /// Number of tokens produced by the forward pass.
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

        // 2. Ensure shard exists and is resident.
        self.shards
            .get_meta(&req.shard)
            .await
            .map_err(IngestError::Internal)?
            .ok_or_else(|| IngestError::ShardNotFound(req.shard.to_string()))?;

        self.shards
            .ensure_resident(&req.shard)
            .await
            .map_err(IngestError::Internal)?;

        // 3. Forward-pass through cortex and append KV.
        //    For now, the "data" is the raw text bytes. When cortex is ready,
        //    this will be the KV cache bytes from a forward pass.
        let data = req.content.as_bytes();
        self.shards
            .append_data(&req.shard, data)
            .await
            .map_err(IngestError::Internal)?;

        // Approximate token count (will be accurate once cortex returns real counts).
        let token_count = (req.content.len() / 4) as u64;
        let offset = 0; // Will come from cortex response.

        // 4. Record position-to-source mapping.
        self.positions
            .record(&req.shard, offset, token_count as u32, &req.content_id)
            .map_err(IngestError::Internal)?;

        // 5. Audit.
        let _ = self
            .audit
            .append(
                AuditAction::Ingest {
                    shard: req.shard.to_string(),
                    content_id: req.content_id.clone(),
                },
                &req.consent_token.source_entity,
                &req.shard.namespace,
                serde_json::json!({}),
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

use serde::{Deserialize, Serialize};

/// A single retrieval result: a scored position within a shard,
/// resolved to a source content reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalHit {
    /// Which shard this hit came from.
    pub shard: String,
    /// Token offset within the shard's KV cache.
    pub offset: u64,
    /// Length in tokens of the attended span.
    pub length: u32,
    /// Attention score (0.0–1.0 for sigmoid, unnormalized for softmax).
    pub score: f32,
    /// Resolved source content ID (from the position-to-source sidecar).
    pub source_id: Option<String>,
}

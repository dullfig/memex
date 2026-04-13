use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::shard::{ShardId, ShardState};

/// A raw attention hit from cortex before source resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawHit {
    pub shard: String,
    pub offset: u64,
    pub length: u32,
    pub score: f32,
}

/// Trait abstracting communication with cortex's cache endpoints.
///
/// The stub implementation returns canned results for development.
/// The real implementation will call cortex's HTTP API.
#[async_trait::async_trait]
pub trait CortexClient: Send + Sync {
    /// Load a shard's KV data into GPU memory.
    async fn load_shard(&self, id: &ShardId, data: &[u8]) -> Result<()>;

    /// Check whether a shard is resident on the GPU.
    async fn check_shard(&self, id: &ShardId) -> Result<Option<ShardState>>;

    /// Evict a shard from GPU memory.
    async fn evict_shard(&self, id: &ShardId) -> Result<()>;

    /// Append KV entries to an already-resident shard.
    async fn append_kv(&self, id: &ShardId, kv_data: &[u8]) -> Result<()>;

    /// Run retrieval: compose shards, query, return raw attention hits.
    async fn retrieve(&self, shards: &[ShardId], query: &str, top_k: u32) -> Result<Vec<RawHit>>;
}

/// Stub cortex client that logs calls and returns empty results.
pub struct StubCortexClient;

#[async_trait::async_trait]
impl CortexClient for StubCortexClient {
    async fn load_shard(&self, id: &ShardId, data: &[u8]) -> Result<()> {
        tracing::info!(shard = %id, bytes = data.len(), "stub: load_shard");
        Ok(())
    }

    async fn check_shard(&self, id: &ShardId) -> Result<Option<ShardState>> {
        tracing::info!(shard = %id, "stub: check_shard -> None");
        Ok(None)
    }

    async fn evict_shard(&self, id: &ShardId) -> Result<()> {
        tracing::info!(shard = %id, "stub: evict_shard");
        Ok(())
    }

    async fn append_kv(&self, id: &ShardId, kv_data: &[u8]) -> Result<()> {
        tracing::info!(shard = %id, bytes = kv_data.len(), "stub: append_kv");
        Ok(())
    }

    async fn retrieve(&self, shards: &[ShardId], query: &str, top_k: u32) -> Result<Vec<RawHit>> {
        tracing::info!(
            shards = ?shards.iter().map(|s| s.to_key()).collect::<Vec<_>>(),
            query_len = query.len(),
            top_k,
            "stub: retrieve -> empty"
        );
        Ok(vec![])
    }
}

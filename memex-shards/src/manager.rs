use std::sync::Arc;

use anyhow::{bail, Result};
use chrono::Utc;

use crate::cortex::{CortexClient, CortexRetrievalResponse};
use crate::shard::{ShardId, ShardMeta, ShardState};

/// Manages shard lifecycle: creation, persistence (sled), and GPU residency (cortex).
///
/// Sled stores two things per shard:
/// - `shard_meta` tree: ShardMeta (state, counts, timestamps)
/// - `shard_tokens` tree: the token history (Vec<u32> as JSON) that built the cache
///
/// Cortex is the GPU-side cache. Token history in sled is replayed into cortex
/// to reconstruct the KV cache on cold-start.
pub struct ShardManager {
    db: sled::Db,
    cortex: Arc<dyn CortexClient>,
}

impl ShardManager {
    pub fn new(db: sled::Db, cortex: Arc<dyn CortexClient>) -> Self {
        Self { db, cortex }
    }

    fn meta_tree(&self) -> Result<sled::Tree> {
        Ok(self.db.open_tree("shard_meta")?)
    }

    fn token_tree(&self) -> Result<sled::Tree> {
        Ok(self.db.open_tree("shard_tokens")?)
    }

    /// Create a new shard. Returns error if it already exists.
    pub async fn create(&self, id: ShardId, pinned: bool) -> Result<ShardMeta> {
        let tree = self.meta_tree()?;
        let key = id.to_key();

        if tree.contains_key(key.as_bytes())? {
            bail!("shard already exists: {key}");
        }

        let meta = ShardMeta {
            id,
            state: ShardState::Cold,
            created_at: Utc::now(),
            token_count: 0,
            byte_size: 0,
            pinned,
        };

        let value = serde_json::to_vec(&meta)?;
        tree.insert(key.as_bytes(), value)?;

        // Initialize empty token history.
        let token_tree = self.token_tree()?;
        let empty: Vec<u32> = vec![];
        token_tree.insert(key.as_bytes(), serde_json::to_vec(&empty)?)?;

        Ok(meta)
    }

    /// Get metadata for a shard.
    pub async fn get_meta(&self, id: &ShardId) -> Result<Option<ShardMeta>> {
        let tree = self.meta_tree()?;
        let key = id.to_key();
        match tree.get(key.as_bytes())? {
            Some(v) => Ok(Some(serde_json::from_slice(&v)?)),
            None => Ok(None),
        }
    }

    /// List all shards in a namespace.
    pub async fn list(&self, namespace: &str) -> Result<Vec<ShardMeta>> {
        let tree = self.meta_tree()?;
        let prefix = format!("{namespace}.");
        let mut results = Vec::new();
        for item in tree.scan_prefix(prefix.as_bytes()) {
            let (_, v) = item?;
            let meta: ShardMeta = serde_json::from_slice(&v)?;
            results.push(meta);
        }
        Ok(results)
    }

    /// Ensure a shard is resident on the GPU. Replays token history from sled if cold.
    pub async fn ensure_resident(&self, id: &ShardId) -> Result<()> {
        let meta = self
            .get_meta(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("shard not found: {id}"))?;

        match meta.state {
            ShardState::Pinned | ShardState::Resident => return Ok(()),
            ShardState::Cold => {}
        }

        // Load token history from sled and replay into cortex.
        let tokens = self.load_token_history(id)?;
        let cache_id = id.to_key();
        self.cortex.load_cache(&cache_id, &tokens).await?;

        let new_state = if meta.pinned {
            ShardState::Pinned
        } else {
            ShardState::Resident
        };
        self.update_state(id, new_state).await?;
        Ok(())
    }

    /// Evict a shard from GPU memory. Does NOT delete from sled.
    pub async fn evict(&self, id: &ShardId) -> Result<()> {
        self.cortex.evict_cache(&id.to_key()).await?;
        self.update_state(id, ShardState::Cold).await?;
        Ok(())
    }

    /// Append tokens to a shard (both sled history and cortex if resident).
    /// Returns the new total token count for this shard.
    pub async fn append_tokens(&self, id: &ShardId, tokens: &[u32]) -> Result<u64> {
        let key = id.to_key();

        // Append to sled token history.
        let token_tree = self.token_tree()?;
        let mut history = match token_tree.get(key.as_bytes())? {
            Some(v) => serde_json::from_slice::<Vec<u32>>(&v)?,
            None => bail!("shard not found: {key}"),
        };
        history.extend_from_slice(tokens);
        let new_count = history.len() as u64;
        token_tree.insert(key.as_bytes(), serde_json::to_vec(&history)?)?;

        // Append to cortex if resident.
        let meta = self.get_meta(id).await?;
        if let Some(ref meta) = meta {
            if meta.state != ShardState::Cold {
                self.cortex.append_tokens(&key, tokens).await?;
            }
        }

        // Update metadata.
        self.update_token_count(id, new_count).await?;
        Ok(new_count)
    }

    /// Get the stored token history for a shard.
    pub fn load_token_history(&self, id: &ShardId) -> Result<Vec<u32>> {
        let token_tree = self.token_tree()?;
        let key = id.to_key();
        match token_tree.get(key.as_bytes())? {
            Some(v) => Ok(serde_json::from_slice(&v)?),
            None => Ok(vec![]),
        }
    }

    /// Run retrieval across shards via cortex. Caller (RetrievalPipeline)
    /// is responsible for ensuring shards are resident first.
    pub async fn retrieve(
        &self,
        shard_ids: &[ShardId],
        query: &str,
        top_k: u32,
    ) -> Result<CortexRetrievalResponse> {
        let cache_shards: Vec<String> = shard_ids.iter().map(|s| s.to_key()).collect();
        self.cortex.retrieve(&cache_shards, query, top_k).await
    }

    async fn update_state(&self, id: &ShardId, state: ShardState) -> Result<()> {
        let tree = self.meta_tree()?;
        let key = id.to_key();
        if let Some(v) = tree.get(key.as_bytes())? {
            let mut meta: ShardMeta = serde_json::from_slice(&v)?;
            meta.state = state;
            tree.insert(key.as_bytes(), serde_json::to_vec(&meta)?)?;
        }
        Ok(())
    }

    async fn update_token_count(&self, id: &ShardId, token_count: u64) -> Result<()> {
        let tree = self.meta_tree()?;
        let key = id.to_key();
        if let Some(v) = tree.get(key.as_bytes())? {
            let mut meta: ShardMeta = serde_json::from_slice(&v)?;
            meta.token_count = token_count;
            tree.insert(key.as_bytes(), serde_json::to_vec(&meta)?)?;
        }
        Ok(())
    }
}

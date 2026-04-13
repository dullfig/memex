use std::sync::Arc;

use anyhow::{bail, Result};
use chrono::Utc;

use crate::cortex::CortexClient;
use crate::shard::{ShardId, ShardMeta, ShardState};

/// Manages shard lifecycle: creation, persistence (sled), and GPU residency (cortex).
pub struct ShardManager {
    db: sled::Db,
    cortex: Arc<dyn CortexClient>,
}

impl ShardManager {
    pub fn new(db: sled::Db, cortex: Arc<dyn CortexClient>) -> Self {
        Self { db, cortex }
    }

    fn tree(&self) -> Result<sled::Tree> {
        Ok(self.db.open_tree("shard_meta")?)
    }

    /// Create a new shard. Returns error if it already exists.
    pub async fn create(&self, id: ShardId, pinned: bool) -> Result<ShardMeta> {
        let tree = self.tree()?;
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
        Ok(meta)
    }

    /// Get metadata for a shard.
    pub async fn get_meta(&self, id: &ShardId) -> Result<Option<ShardMeta>> {
        let tree = self.tree()?;
        let key = id.to_key();
        match tree.get(key.as_bytes())? {
            Some(v) => Ok(Some(serde_json::from_slice(&v)?)),
            None => Ok(None),
        }
    }

    /// List all shards in a namespace.
    pub async fn list(&self, namespace: &str) -> Result<Vec<ShardMeta>> {
        let tree = self.tree()?;
        let prefix = format!("{namespace}.");
        let mut results = Vec::new();
        for item in tree.scan_prefix(prefix.as_bytes()) {
            let (_, v) = item?;
            let meta: ShardMeta = serde_json::from_slice(&v)?;
            results.push(meta);
        }
        Ok(results)
    }

    /// Ensure a shard is resident on the GPU. Loads from sled if cold.
    pub async fn ensure_resident(&self, id: &ShardId) -> Result<()> {
        let meta = self
            .get_meta(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("shard not found: {id}"))?;

        match meta.state {
            ShardState::Pinned | ShardState::Resident => return Ok(()),
            ShardState::Cold => {}
        }

        // Load KV data from sled's data tree into cortex.
        let data_tree = self.db.open_tree("shard_data")?;
        let data = data_tree
            .get(id.to_key().as_bytes())?
            .unwrap_or_default();

        self.cortex.load_shard(id, &data).await?;

        // Update state.
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
        self.cortex.evict_shard(id).await?;
        self.update_state(id, ShardState::Cold).await?;
        Ok(())
    }

    /// Append KV data to a shard (both sled and cortex).
    pub async fn append_data(&self, id: &ShardId, data: &[u8]) -> Result<()> {
        // Append to sled's data tree.
        let data_tree = self.db.open_tree("shard_data")?;
        let key = id.to_key();
        let existing = data_tree.get(key.as_bytes())?.unwrap_or_default();
        let mut combined = existing.to_vec();
        combined.extend_from_slice(data);
        data_tree.insert(key.as_bytes(), combined)?;

        // Append to cortex if resident.
        let meta = self.get_meta(id).await?;
        if let Some(meta) = meta {
            if meta.state != ShardState::Cold {
                self.cortex.append_kv(id, data).await?;
            }
        }

        // Update byte_size in metadata.
        self.update_byte_size(id, data.len() as u64).await?;
        Ok(())
    }

    /// Access the cortex client (for retrieval pipeline to call directly).
    pub fn cortex(&self) -> &dyn CortexClient {
        self.cortex.as_ref()
    }

    async fn update_state(&self, id: &ShardId, state: ShardState) -> Result<()> {
        let tree = self.tree()?;
        let key = id.to_key();
        if let Some(v) = tree.get(key.as_bytes())? {
            let mut meta: ShardMeta = serde_json::from_slice(&v)?;
            meta.state = state;
            tree.insert(key.as_bytes(), serde_json::to_vec(&meta)?)?;
        }
        Ok(())
    }

    async fn update_byte_size(&self, id: &ShardId, additional_bytes: u64) -> Result<()> {
        let tree = self.tree()?;
        let key = id.to_key();
        if let Some(v) = tree.get(key.as_bytes())? {
            let mut meta: ShardMeta = serde_json::from_slice(&v)?;
            meta.byte_size += additional_bytes;
            tree.insert(key.as_bytes(), serde_json::to_vec(&meta)?)?;
        }
        Ok(())
    }
}

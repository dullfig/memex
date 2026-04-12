use serde::{Deserialize, Serialize};

/// A parsed shard identifier: `{namespace}.{category}.{entity_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShardId {
    pub namespace: String,
    pub category: String,
    pub entity_id: String,
}

impl ShardId {
    pub fn new(namespace: impl Into<String>, category: impl Into<String>, entity_id: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            category: category.into(),
            entity_id: entity_id.into(),
        }
    }

    pub fn to_key(&self) -> String {
        format!("{}.{}.{}", self.namespace, self.category, self.entity_id)
    }

    pub fn parse(key: &str) -> Option<Self> {
        let mut parts = key.splitn(3, '.');
        Some(Self {
            namespace: parts.next()?.to_owned(),
            category: parts.next()?.to_owned(),
            entity_id: parts.next()?.to_owned(),
        })
    }
}

impl std::fmt::Display for ShardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.namespace, self.category, self.entity_id)
    }
}

/// Whether a shard is resident on the GPU or only in cold storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShardState {
    /// Pinned in GPU memory (shared shards).
    Pinned,
    /// Currently loaded in GPU memory (on-demand).
    Resident,
    /// In sled only, not on GPU.
    Cold,
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single entry in the hash-chained audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Monotonic sequence number.
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub action: AuditAction,
    /// Who performed the action (user ID or service name).
    pub actor: String,
    /// Namespace this action occurred in.
    pub namespace: String,
    /// Additional structured detail.
    pub detail: serde_json::Value,
    /// SHA-256 hash of the previous entry (all zeros for seq 0).
    pub prev_hash: [u8; 32],
    /// SHA-256 hash of this entry (covers prev_hash, seq, timestamp, action, actor, namespace).
    pub hash: [u8; 32],
}

/// What kind of auditable action occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditAction {
    Ingest {
        shard: String,
        content_id: String,
    },
    Retrieve {
        shards: Vec<String>,
        query_hash: String,
        hit_count: u32,
    },
    ShardCreate {
        shard: String,
    },
    ShardEvict {
        shard: String,
    },
    ConsentGrant {
        entity: String,
    },
    ConsentRevoke {
        entity: String,
    },
}

use anyhow::Result;
use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::entry::{AuditAction, AuditEntry};
use crate::filter::AuditFilter;

/// Hash-chained, sled-backed audit log.
pub struct AuditLog {
    tree: sled::Tree,
    /// Protects the append path to guarantee sequential hashing.
    append_lock: Mutex<()>,
}

impl AuditLog {
    pub fn open(db: &sled::Db) -> Result<Self> {
        let tree = db.open_tree("audit_log")?;
        Ok(Self {
            tree,
            append_lock: Mutex::new(()),
        })
    }

    /// Append an entry, computing the hash chain link.
    pub async fn append(
        &self,
        action: AuditAction,
        actor: &str,
        namespace: &str,
        detail: serde_json::Value,
    ) -> Result<AuditEntry> {
        let _guard = self.append_lock.lock().await;

        let (seq, prev_hash) = match self.tree.last()? {
            Some((_, v)) => {
                let prev: AuditEntry = serde_json::from_slice(&v)?;
                (prev.seq + 1, prev.hash)
            }
            None => (0, [0u8; 32]),
        };

        let timestamp = Utc::now();
        let action_bytes = serde_json::to_vec(&action)?;

        let mut hasher = Sha256::new();
        hasher.update(prev_hash);
        hasher.update(seq.to_le_bytes());
        hasher.update(timestamp.timestamp_micros().to_le_bytes());
        hasher.update(&action_bytes);
        hasher.update(actor.as_bytes());
        hasher.update(namespace.as_bytes());
        let hash: [u8; 32] = hasher.finalize().into();

        let entry = AuditEntry {
            seq,
            timestamp,
            action,
            actor: actor.to_owned(),
            namespace: namespace.to_owned(),
            detail,
            prev_hash,
            hash,
        };

        let key = seq.to_be_bytes();
        let value = serde_json::to_vec(&entry)?;
        self.tree.insert(key, value)?;

        Ok(entry)
    }

    /// Query the audit log with optional filters.
    pub async fn query(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>> {
        let mut results = Vec::new();
        let mut skipped = 0u64;
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(100);

        for item in self.tree.iter() {
            let (_, v) = item?;
            let entry: AuditEntry = serde_json::from_slice(&v)?;

            if !Self::matches_filter(&entry, filter) {
                continue;
            }

            if skipped < offset {
                skipped += 1;
                continue;
            }

            results.push(entry);
            if results.len() as u64 >= limit {
                break;
            }
        }

        Ok(results)
    }

    /// Verify that the hash chain is intact over a range.
    pub async fn verify_chain(&self, from_seq: u64, to_seq: u64) -> Result<bool> {
        let mut expected_prev_hash = None;

        for seq in from_seq..=to_seq {
            let key = seq.to_be_bytes();
            let Some(v) = self.tree.get(key)? else {
                return Ok(false);
            };
            let entry: AuditEntry = serde_json::from_slice(&v)?;

            if let Some(expected) = expected_prev_hash {
                if entry.prev_hash != expected {
                    return Ok(false);
                }
            }

            // Recompute hash to verify integrity.
            let action_bytes = serde_json::to_vec(&entry.action)?;
            let mut hasher = Sha256::new();
            hasher.update(entry.prev_hash);
            hasher.update(entry.seq.to_le_bytes());
            hasher.update(entry.timestamp.timestamp_micros().to_le_bytes());
            hasher.update(&action_bytes);
            hasher.update(entry.actor.as_bytes());
            hasher.update(entry.namespace.as_bytes());
            let computed: [u8; 32] = hasher.finalize().into();

            if computed != entry.hash {
                return Ok(false);
            }

            expected_prev_hash = Some(entry.hash);
        }

        Ok(true)
    }

    fn matches_filter(entry: &AuditEntry, filter: &AuditFilter) -> bool {
        if let Some(ns) = &filter.namespace {
            if &entry.namespace != ns {
                return false;
            }
        }
        if let Some(actor) = &filter.actor {
            if &entry.actor != actor {
                return false;
            }
        }
        if let Some(action_type) = &filter.action_type {
            let entry_type = match &entry.action {
                AuditAction::Ingest { .. } => "Ingest",
                AuditAction::Retrieve { .. } => "Retrieve",
                AuditAction::ShardCreate { .. } => "ShardCreate",
                AuditAction::ShardEvict { .. } => "ShardEvict",
                AuditAction::ConsentGrant { .. } => "ConsentGrant",
                AuditAction::ConsentRevoke { .. } => "ConsentRevoke",
            };
            if entry_type != action_type {
                return false;
            }
        }
        if let Some(from) = &filter.from {
            if entry.timestamp < *from {
                return false;
            }
        }
        if let Some(to) = &filter.to {
            if entry.timestamp > *to {
                return false;
            }
        }
        true
    }
}

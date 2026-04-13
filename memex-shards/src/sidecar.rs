use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::shard::ShardId;

/// A resolved reference from a token position back to source content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub content_id: String,
    pub offset_within_source: u64,
}

/// Maps token positions within a shard to source content IDs.
///
/// Each shard has a sidecar stored in sled. When content is ingested,
/// the position mapping is recorded. When retrieval returns a position,
/// this map resolves it to the original content.
pub struct PositionMap {
    tree: sled::Tree,
}

impl PositionMap {
    pub fn open(db: &sled::Db) -> Result<Self> {
        let tree = db.open_tree("position_map")?;
        Ok(Self { tree })
    }

    /// Record that tokens at `offset..offset+length` in `shard` came from `content_id`.
    pub fn record(
        &self,
        shard: &ShardId,
        offset: u64,
        length: u32,
        content_id: &str,
    ) -> Result<()> {
        let key = Self::key(shard, offset);
        let entry = SidecarEntry {
            content_id: content_id.to_owned(),
            offset,
            length,
        };
        let value = serde_json::to_vec(&entry)?;
        self.tree.insert(key, value)?;
        Ok(())
    }

    /// Resolve a token offset within a shard to its source content.
    pub fn resolve(&self, shard: &ShardId, offset: u64) -> Result<Option<SourceRef>> {
        // Scan backwards from offset to find the entry that contains this position.
        let prefix = format!("{}:", shard.to_key());
        for item in self.tree.scan_prefix(prefix.as_bytes()).rev() {
            let (_, v) = item?;
            let entry: SidecarEntry = serde_json::from_slice(&v)?;
            if entry.offset <= offset && offset < entry.offset + entry.length as u64 {
                return Ok(Some(SourceRef {
                    content_id: entry.content_id,
                    offset_within_source: offset - entry.offset,
                }));
            }
            // If we've gone past, stop scanning.
            if entry.offset + entry.length as u64 <= offset {
                break;
            }
        }
        Ok(None)
    }

    fn key(shard: &ShardId, offset: u64) -> Vec<u8> {
        // Key format: "{shard_key}:{offset:020}" — zero-padded for lexicographic ordering.
        format!("{}:{:020}", shard.to_key(), offset).into_bytes()
    }
}

#[derive(Serialize, Deserialize)]
struct SidecarEntry {
    content_id: String,
    offset: u64,
    length: u32,
}

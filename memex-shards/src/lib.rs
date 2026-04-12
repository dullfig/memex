//! KV cache shard management.
//!
//! Handles shard naming (`{namespace}.{category}.{entity_id}`),
//! VMM-style tiering (pinned vs on-demand), sled persistence,
//! and GPU-resident cache coordination via cortex.

pub mod shard;

pub use shard::{ShardId, ShardState};

//! KV cache shard management.
//!
//! Handles shard naming (`{namespace}.{category}.{entity_id}`),
//! VMM-style tiering (pinned vs on-demand), sled persistence,
//! and GPU-resident cache coordination via cortex.

pub mod cortex;
pub mod manager;
pub mod shard;
pub mod sidecar;

pub use cortex::{CortexClient, RawHit, StubCortexClient};
pub use manager::ShardManager;
pub use shard::{ShardId, ShardMeta, ShardState};
pub use sidecar::{PositionMap, SourceRef};

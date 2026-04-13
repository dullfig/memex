//! Content ingestion pipeline.
//!
//! Takes raw text, verifies consent, runs a forward pass through the
//! librarian model (via cortex), and appends the resulting KV entries
//! to the appropriate shard.

pub mod pipeline;

pub use pipeline::{IngestError, IngestPipeline, IngestRequest, IngestResult};

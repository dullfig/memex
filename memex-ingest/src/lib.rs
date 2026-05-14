//! Content ingestion pipeline.
//!
//! Two surfaces, evolving toward one:
//!
//! - **`pipeline`** — the legacy in-process API: caller hands a
//!   `(content_id, content, shard, consent_token)` to
//!   [`IngestPipeline::ingest`]. Used today by `memex-api`'s
//!   `/v1/ingest` route and the (about-to-be-replaced) Rust-side
//!   walker in `memex-cli`.
//!
//! - **`driver` + `runtime`** — the in-progress WASM+WIT host that
//!   loads sandboxed corpus drivers, drives `init → next-chunk →
//!   finish`, and feeds emitted chunks through `IngestPipeline`. Per
//!   `memex-ingest/README.md`. Skeleton only in Phase 1; bindgen and
//!   the agentos-wasm load_component_raw blocker land in Phase 2.

pub mod driver;
pub mod pipeline;
pub mod runtime;

// Note: the WIT-derived `driver::IngestError` is *not* re-exported here
// to avoid colliding with `pipeline::IngestError`. Callers inspect
// driver-reported errors via the structured `DriverError::Driver { kind,
// message, context }` variant; the underlying record stays inside
// `driver::`.
pub use driver::{Chunk, CorpusConfig, DriverError, DriverMetadata, IngestionDriverPeer};
pub use pipeline::{IngestError, IngestPipeline, IngestRequest, IngestResult};
pub use runtime::{ingest_capabilities, GUEST_CORPUS_ROOT};

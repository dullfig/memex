//! Memex HTTP API surface.
//!
//! Exposes endpoints for ingestion, retrieval, shard management,
//! and audit queries. Delegates to the domain crates.

pub mod error;
pub mod extract;
pub mod routes;
pub mod state;

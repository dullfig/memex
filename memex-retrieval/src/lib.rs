//! Attention-based retrieval.
//!
//! Composes relevant shards, runs a query through the librarian model
//! (via cortex `mode: "retrieve"`), extracts top-K attention positions,
//! and resolves them to source text references.

pub mod pipeline;
pub mod position;

pub use pipeline::{
    RetrievalError, RetrievalPipeline, RetrievalPurpose, RetrievalRequest, RetrievalResponse,
};
pub use position::RetrievalHit;

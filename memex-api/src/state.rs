use std::sync::Arc;

use memex_audit::AuditLog;
use memex_consent::ConsentVerifier;
use memex_ingest::IngestPipeline;
use memex_retrieval::RetrievalPipeline;
use memex_shards::ShardManager;

/// Shared application state, passed to all route handlers via axum.
#[derive(Clone)]
pub struct AppState {
    pub shard_manager: Arc<ShardManager>,
    pub ingest_pipeline: Arc<IngestPipeline>,
    pub retrieval_pipeline: Arc<RetrievalPipeline>,
    pub audit_log: Arc<AuditLog>,
    pub consent_verifier: Arc<dyn ConsentVerifier>,
}

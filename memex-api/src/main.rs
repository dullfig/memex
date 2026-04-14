use std::sync::Arc;

use memex_audit::AuditLog;
use memex_consent::StubConsentVerifier;
use memex_ingest::IngestPipeline;
use memex_retrieval::RetrievalPipeline;
use memex_shards::{
    CortexClient, HttpCortexClient, PositionMap, ShardManager, StubCortexClient,
};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use memex_api::routes;
use memex_api::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let bind_addr = std::env::var("MEMEX_BIND").unwrap_or_else(|_| "127.0.0.1:7720".into());
    let sled_path = std::env::var("MEMEX_DB").unwrap_or_else(|_| "memex.db".into());
    let cortex_url = std::env::var("CORTEX_URL").ok();

    let cortex: Arc<dyn CortexClient> = match &cortex_url {
        Some(url) => {
            tracing::info!(url, "connecting to cortex");
            Arc::new(HttpCortexClient::new(url))
        }
        None => {
            tracing::warn!("CORTEX_URL not set — using stub client (no GPU, no real retrieval)");
            Arc::new(StubCortexClient)
        }
    };

    let consent: Arc<dyn memex_consent::ConsentVerifier> = Arc::new(StubConsentVerifier);

    tracing::info!(bind = %bind_addr, db = %sled_path, "starting memex");

    let db = sled::open(&sled_path)?;
    let shard_manager = Arc::new(ShardManager::new(db.clone(), cortex));
    let position_map = Arc::new(PositionMap::open(&db)?);
    let audit_log = Arc::new(AuditLog::open(&db)?);

    let ingest_pipeline = Arc::new(IngestPipeline::new(
        shard_manager.clone(),
        consent.clone(),
        position_map.clone(),
        audit_log.clone(),
    ));

    let retrieval_pipeline = Arc::new(RetrievalPipeline::new(
        shard_manager.clone(),
        position_map,
        audit_log.clone(),
    ));

    let state = AppState {
        shard_manager,
        ingest_pipeline,
        retrieval_pipeline,
        audit_log,
        consent_verifier: consent,
    };

    let app = routes::router()
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("memex listening on {bind_addr}");
    axum::serve(listener, app).await?;

    Ok(())
}

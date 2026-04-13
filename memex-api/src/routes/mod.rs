pub mod audit;
pub mod health;
pub mod ingest;
pub mod retrieve;
pub mod shards;

use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/v1/ingest", ingest::router())
        .nest("/v1/retrieve", retrieve::router())
        .nest("/v1/shards", shards::router())
        .nest("/v1/audit", audit::router())
        .merge(health::router())
}

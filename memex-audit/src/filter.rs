use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Filter criteria for querying the audit log.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AuditFilter {
    pub namespace: Option<String>,
    pub actor: Option<String>,
    /// Filter by action type name (e.g. "Ingest", "Retrieve").
    pub action_type: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

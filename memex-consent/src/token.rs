use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A cryptographically signed consent token authorizing content ingestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentToken {
    pub token_id: Uuid,
    /// The entity granting consent (e.g. a user ID).
    pub source_entity: String,
    /// Which namespace this consent applies to.
    pub namespace: String,
    /// What content this consent covers.
    pub scope: ConsentScope,
    pub issued_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Cryptographic signature over the above fields.
    pub signature: Vec<u8>,
}

/// What content a consent token covers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsentScope {
    /// All content from this entity.
    AllContent,
    /// Content in a specific category.
    Category(String),
    /// Specific content IDs only.
    SpecificIds(Vec<String>),
}

/// Errors from consent verification.
#[derive(Debug, thiserror::Error)]
pub enum ConsentError {
    #[error("consent token has expired")]
    Expired,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("consent has been revoked")]
    Revoked,
    #[error("content not covered by consent scope")]
    ScopeMismatch,
}

/// Trait for verifying consent tokens.
#[async_trait::async_trait]
pub trait ConsentVerifier: Send + Sync {
    async fn verify(&self, token: &ConsentToken) -> Result<(), ConsentError>;
}

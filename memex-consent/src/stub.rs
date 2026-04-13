use crate::token::{ConsentError, ConsentToken, ConsentVerifier};

/// Stub verifier that accepts all tokens. For development only.
pub struct StubConsentVerifier;

#[async_trait::async_trait]
impl ConsentVerifier for StubConsentVerifier {
    async fn verify(&self, _token: &ConsentToken) -> Result<(), ConsentError> {
        tracing::debug!("stub consent verifier: accepting token unconditionally");
        Ok(())
    }
}

//! Consent-gated ingestion.
//!
//! Content enters the cache only if the source opted in via a
//! cryptographically signed consent token. This crate handles
//! token generation, verification, and revocation.

pub mod token;
pub mod stub;

pub use token::{ConsentError, ConsentScope, ConsentToken, ConsentVerifier};
pub use stub::StubConsentVerifier;

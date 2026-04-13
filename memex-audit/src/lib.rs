//! Tamper-evident query audit trail.
//!
//! Every retrieval query is logged with hash chaining so the audit
//! history cannot be silently altered.

pub mod entry;
pub mod filter;
pub mod log;

pub use entry::{AuditAction, AuditEntry};
pub use filter::AuditFilter;
pub use log::AuditLog;

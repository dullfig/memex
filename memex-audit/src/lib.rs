//! Tamper-evident query audit trail.
//!
//! Every retrieval query is logged with hash chaining so the audit
//! history cannot be silently altered. Supports the responsibility
//! constraint: "cannot be silently altered."

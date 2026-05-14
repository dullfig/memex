//! IngestionDriverPeer — host-side wrapper around a memex ingestion
//! driver WASM component.
//!
//! Holds a long-lived `WasmSession` so iterator state on the driver side
//! (file list cursor, open file handles, etc.) persists across
//! `next_chunk` calls. Memex drives the lifecycle:
//!
//! ```text
//!   load(path, corpus) → init(config) → next_chunk* → finish → drop
//! ```
//!
//! Loading goes through `agentos_wasm::WasmRuntime::load_component_raw_from_path`
//! to skip the AgentOS-specific `get-metadata` extraction step (memex's
//! WIT contract has no such export).
//!
//! The WIT exports (`init`, `next-chunk`, `finish`) are reached via
//! `wasmtime::component::bindgen!`-generated typed wrappers; see the
//! [`bindings`] module below.

use std::path::Path;

use agentos_wasm::runtime::WasmRuntime;
use agentos_wasm::WasmSession;

use crate::runtime::ingest_capabilities;

// ---------------------------------------------------------------------------
// Generated bindings for memex's ingestion-driver WIT world.
// ---------------------------------------------------------------------------
//
// `wasmtime::component::bindgen!` expands the WIT into typed Rust at
// compile time. Field names go kebab → snake; records become structs.
// The generated `IngestionDriver` struct caches the exported `Func`
// handles and exposes `call_init` / `call_next_chunk` / `call_finish`.
//
// `with` maps the WIT `result<T, ingest-error>` into our own DriverError
// inner channel where convenient; here we leave it as the generated
// shape and unwrap at the IngestionDriverPeer boundary.
mod bindings {
    wasmtime::component::bindgen!({
        world: "ingestion-driver",
        path: "wit",
    });
}

/// Re-exports of the bindgen-generated record types. Callers of
/// `IngestionDriverPeer` use these to assemble `CorpusConfig` values and
/// to inspect the `Chunk` / `DriverMetadata` / `IngestError` records
/// emitted by drivers.
pub use bindings::{Chunk, CorpusConfig, DriverMetadata, IngestError};

/// Errors from driver loading and invocation.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("agentos-wasm runtime error: {0}")]
    Runtime(String),
    #[error("corpus path does not exist: {0}")]
    CorpusNotFound(String),
    /// wasmtime-level error invoking an export (trap, link error, etc).
    /// Distinct from a WIT-level `ingest-error` returned by the driver.
    #[error("wasm call failed ({export}): {source}")]
    Trap {
        export: &'static str,
        #[source]
        source: wasmtime::Error,
    },
    /// The driver returned a structured `ingest-error` from one of the
    /// WIT exports. Carries the driver's own `kind` / `message` /
    /// `context` so callers can route on `kind`.
    #[error("driver reported {kind}: {message}{context}",
            context = .context.as_deref().map(|c| format!(" (context: {c})")).unwrap_or_default())]
    Driver {
        export: &'static str,
        kind: String,
        message: String,
        context: Option<String>,
    },
}

impl DriverError {
    fn from_ingest_error(export: &'static str, e: IngestError) -> Self {
        DriverError::Driver {
            export,
            kind: e.kind,
            message: e.message,
            context: e.context,
        }
    }
}

/// A loaded, instantiated memex ingestion driver. Owns the long-lived
/// `WasmSession` plus the generated bindings handle.
///
/// Construction goes through [`IngestionDriverPeer::load`], which:
///   1. Loads the `.wasm` via `load_component_raw_from_path`
///      (no AgentOS metadata extraction).
///   2. Builds a read-only FS capability for the corpus root.
///   3. Instantiates a session.
///   4. Wraps the session's instance in bindgen-generated typed bindings.
///
/// Invocation order is `init` → `next_chunk` (repeated) → `finish`.
/// Calling them out of order is allowed by the wasm runtime — drivers
/// decide how to handle it (typically returning an `ingest-error` with
/// `kind = "config"` or similar).
pub struct IngestionDriverPeer {
    session: WasmSession,
    bindings: bindings::IngestionDriver,
}

impl IngestionDriverPeer {
    /// Load a driver `.wasm` from disk and prepare a session over a
    /// read-only mount of `corpus_root`.
    pub fn load(
        runtime: &WasmRuntime,
        driver_wasm: &Path,
        corpus_root: &Path,
    ) -> Result<Self, DriverError> {
        if !corpus_root.exists() {
            return Err(DriverError::CorpusNotFound(
                corpus_root.to_string_lossy().into_owned(),
            ));
        }
        let component = runtime
            .load_component_raw_from_path(driver_wasm)
            .map_err(|e| DriverError::Runtime(e.to_string()))?;
        let caps = ingest_capabilities(corpus_root);
        let mut session = component
            .instantiate_session(runtime, &caps)
            .map_err(|e| DriverError::Runtime(e.to_string()))?;
        let bindings = bindings::IngestionDriver::new(&mut session.store, &session.instance)
            .map_err(|e| DriverError::Runtime(format!("binding init failed: {e}")))?;
        Ok(Self { session, bindings })
    }

    /// Call the driver's `init` export. Returns the driver's
    /// self-description on success; on failure, the driver's
    /// `ingest-error` is mapped to [`DriverError::Driver`].
    pub fn init(&mut self, config: &CorpusConfig) -> Result<DriverMetadata, DriverError> {
        match self.bindings.call_init(&mut self.session.store, config) {
            Ok(Ok(meta)) => Ok(meta),
            Ok(Err(e)) => Err(DriverError::from_ingest_error("init", e)),
            Err(source) => Err(DriverError::Trap { export: "init", source }),
        }
    }

    /// Pull the next chunk. `Ok(None)` means the driver is exhausted —
    /// stop iterating and call [`Self::finish`].
    pub fn next_chunk(&mut self) -> Result<Option<Chunk>, DriverError> {
        match self.bindings.call_next_chunk(&mut self.session.store) {
            Ok(Ok(opt)) => Ok(opt),
            Ok(Err(e)) => Err(DriverError::from_ingest_error("next-chunk", e)),
            Err(source) => Err(DriverError::Trap { export: "next-chunk", source }),
        }
    }

    /// Tell the driver to flush state and shut down. Idempotent on the
    /// driver side; the host typically calls this once after iteration
    /// stops (whether on exhaustion or early termination).
    pub fn finish(&mut self) -> Result<(), DriverError> {
        match self.bindings.call_finish(&mut self.session.store) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(DriverError::from_ingest_error("finish", e)),
            Err(source) => Err(DriverError::Trap { export: "finish", source }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_rejects_missing_corpus() {
        let runtime = WasmRuntime::new().expect("runtime");
        let result = IngestionDriverPeer::load(
            &runtime,
            Path::new("any.wasm"),
            Path::new("definitely-not-a-real-path-zzqq"),
        );
        assert!(matches!(result, Err(DriverError::CorpusNotFound(_))));
    }

    #[test]
    fn load_rejects_missing_wasm() {
        // Corpus path exists (use the workspace root), but driver doesn't.
        let runtime = WasmRuntime::new().expect("runtime");
        let workspace = std::env::current_dir().unwrap();
        let result = IngestionDriverPeer::load(
            &runtime,
            Path::new("nonexistent-driver.wasm"),
            &workspace,
        );
        assert!(matches!(result, Err(DriverError::Runtime(_))));
    }

    /// Verify the generated record types have the WIT shape we expect.
    /// This is a compile-time check disguised as a unit test — if a
    /// future WIT edit accidentally renames a field, the build breaks
    /// here rather than producing mystery errors in callers.
    #[test]
    fn generated_types_have_expected_shape() {
        let config = CorpusConfig {
            root: "/corpus".to_owned(),
            options: vec![("strip-frontmatter".to_owned(), "true".to_owned())],
        };
        assert_eq!(config.root, "/corpus");
        assert_eq!(config.options.len(), 1);

        let chunk = Chunk {
            id: "history/cash.md".to_owned(),
            text: "founding story...".to_owned(),
            source_ref: "history/cash.md".to_owned(),
            metadata: vec![("author".to_owned(), "wiki".to_owned())],
        };
        assert_eq!(chunk.id, "history/cash.md");

        let err = IngestError {
            kind: "parse".to_owned(),
            message: "yaml fail".to_owned(),
            context: Some("history/cash.md".to_owned()),
        };
        let wrapped = DriverError::from_ingest_error("init", err);
        match wrapped {
            DriverError::Driver { export, kind, .. } => {
                assert_eq!(export, "init");
                assert_eq!(kind, "parse");
            }
            other => panic!("expected DriverError::Driver, got {other:?}"),
        }
    }
}

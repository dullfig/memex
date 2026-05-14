//! Capability building for memex ingestion drivers.
//!
//! Drivers are WASM components that read corpus files. The only WASI
//! grant they need is a read-only filesystem mount of the corpus root.
//! No env vars, no stdio, no network — by construction.

use std::path::Path;

use agentos_wasm::capabilities::{FsGrant, WasmCapabilities};

/// Guest-side mount point under which the corpus root appears inside
/// the WASM sandbox. Drivers receive this as `corpus_config.root` and
/// open files relative to it via WASI.
pub const GUEST_CORPUS_ROOT: &str = "/corpus";

/// Build the default ingestion capability set: one read-only filesystem
/// grant for the corpus path, mounted at [`GUEST_CORPUS_ROOT`]. Nothing
/// else is granted.
///
/// The host caller is responsible for ensuring `corpus_root` exists —
/// `agentos_wasm::capabilities::build_wasi_ctx` will reject a missing
/// path at session instantiation time.
pub fn ingest_capabilities(corpus_root: &Path) -> WasmCapabilities {
    WasmCapabilities {
        filesystem: vec![FsGrant {
            host_path: corpus_root.to_string_lossy().into_owned(),
            guest_path: GUEST_CORPUS_ROOT.to_owned(),
            read_only: true,
        }],
        env_vars: vec![],
        stdio: false,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_capabilities_grants_one_read_only_mount() {
        let caps = ingest_capabilities(Path::new("C:/some/corpus"));
        assert_eq!(caps.filesystem.len(), 1);
        let fs = &caps.filesystem[0];
        assert!(fs.read_only);
        assert_eq!(fs.guest_path, GUEST_CORPUS_ROOT);
        assert!(caps.env_vars.is_empty());
        assert!(!caps.stdio);
    }
}

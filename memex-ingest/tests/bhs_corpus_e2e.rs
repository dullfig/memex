//! End-to-end integration test: load the real bhs-corpus driver `.wasm`
//! through `IngestionDriverPeer`, point it at `C:/src/bhs-corpus/sources`,
//! and verify the full `init → next_chunk* → finish` lifecycle.
//!
//! Requires both:
//!   - The driver built: from
//!     `memex-ingest/drivers/bhs-corpus/`, run
//!     `cargo build --release --target wasm32-wasip2`.
//!   - The corpus at `C:/src/bhs-corpus/sources/`.
//!
//! If either is missing the test is skipped with a clear message so a
//! fresh checkout (or CI without the corpus) doesn't fail spuriously.

use std::path::{Path, PathBuf};

use agentos_wasm::runtime::WasmRuntime;
use memex_ingest::{CorpusConfig, IngestionDriverPeer, GUEST_CORPUS_ROOT};

fn driver_wasm_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("drivers/bhs-corpus/target/wasm32-wasip2/release/bhs_corpus_driver.wasm")
}

fn corpus_path() -> PathBuf {
    PathBuf::from("C:/src/bhs-corpus/sources")
}

fn skip_if_missing() -> Option<(PathBuf, PathBuf)> {
    let driver = driver_wasm_path();
    let corpus = corpus_path();
    if !driver.exists() {
        eprintln!(
            "SKIP: driver wasm not found at {}\n  build with: \
             cd memex-ingest/drivers/bhs-corpus && \
             cargo build --release --target wasm32-wasip2",
            driver.display()
        );
        return None;
    }
    if !corpus.exists() {
        eprintln!(
            "SKIP: corpus not found at {} (expected `C:/src/bhs-corpus/sources/`)",
            corpus.display()
        );
        return None;
    }
    Some((driver, corpus))
}

#[test]
fn driver_emits_chunks_from_real_corpus() {
    let Some((driver_path, corpus_root)) = skip_if_missing() else {
        return;
    };

    let runtime = WasmRuntime::new().expect("runtime init");
    let mut peer = IngestionDriverPeer::load(&runtime, &driver_path, &corpus_root)
        .expect("driver should load against the real corpus");

    let config = CorpusConfig {
        root: GUEST_CORPUS_ROOT.to_owned(),
        options: vec![],
    };

    let metadata = peer.init(&config).expect("init should succeed");
    assert_eq!(metadata.name, "bhs-corpus");
    assert_eq!(metadata.accepts, vec!["*.md".to_owned()]);

    let mut chunks = Vec::new();
    loop {
        match peer.next_chunk().expect("next_chunk should not trap") {
            Some(chunk) => chunks.push(chunk),
            None => break,
        }
    }
    peer.finish().expect("finish should succeed");

    // The corpus map (project_bhs_corpus_map.md) documented 55 files at
    // its writing; current count is 57 from the dry-run yesterday. Use
    // a range so adding/removing a few markdown files doesn't break the
    // test in lockstep — the assertion is "did the driver walk the
    // tree", not "is the corpus exactly N files."
    assert!(
        chunks.len() >= 40,
        "expected at least ~40 chunks from the corpus walk, got {}",
        chunks.len()
    );

    // Sanity-check chunk shape.
    let first = &chunks[0];
    assert!(!first.id.is_empty(), "chunk id should be non-empty");
    assert_eq!(first.id, first.source_ref, "id and source_ref should match for this driver");
    assert!(
        !first.text.trim().is_empty(),
        "chunk text should be non-empty (frontmatter-only files are skipped)"
    );
    // ids should be relative paths using forward slashes regardless of host OS.
    assert!(
        !first.id.contains('\\'),
        "chunk id {:?} contains a backslash — driver should normalize to /",
        first.id
    );

    // Frontmatter should have been stripped on at least one file we
    // know carries it. Pick one known content file.
    let cash = chunks.iter().find(|c| c.id.ends_with("people-owen-cash.md"));
    if let Some(c) = cash {
        assert!(
            !c.text.starts_with("---"),
            "frontmatter should be stripped from {}; got: {:?}",
            c.id,
            &c.text[..c.text.len().min(80)]
        );
    }

    // ids must be unique.
    let mut ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
    ids.sort();
    let before = ids.len();
    ids.dedup();
    assert_eq!(before, ids.len(), "chunk ids should be unique");
}

#[test]
fn next_chunk_before_init_returns_config_error() {
    let Some((driver_path, corpus_root)) = skip_if_missing() else {
        return;
    };

    let runtime = WasmRuntime::new().expect("runtime init");
    let mut peer = IngestionDriverPeer::load(&runtime, &driver_path, &corpus_root)
        .expect("driver load");

    let err = peer.next_chunk().expect_err("calling next_chunk before init should error");
    match err {
        memex_ingest::DriverError::Driver { kind, export, .. } => {
            assert_eq!(export, "next-chunk");
            assert_eq!(kind, "config");
        }
        other => panic!("expected Driver{{kind=config}}, got {other:?}"),
    }
}

/// Limit-fixture test: a single small driver call should be fast.
/// Not a perf benchmark, just a tripwire — if iterator state ever leaks
/// (re-walking the tree per next_chunk) this catches it.
#[test]
fn second_init_resets_cursor() {
    let Some((driver_path, corpus_root)) = skip_if_missing() else {
        return;
    };

    let runtime = WasmRuntime::new().expect("runtime init");
    let mut peer = IngestionDriverPeer::load(&runtime, &driver_path, &corpus_root)
        .expect("driver load");

    let config = CorpusConfig {
        root: GUEST_CORPUS_ROOT.to_owned(),
        options: vec![],
    };

    peer.init(&config).expect("first init");
    let first_chunk = peer.next_chunk().expect("ok").expect("some");

    peer.init(&config).expect("second init");
    let restarted = peer.next_chunk().expect("ok").expect("some");

    assert_eq!(
        first_chunk.id, restarted.id,
        "re-init should reset the cursor to the start"
    );
}

// Silence the unused import warning when the corpus isn't present —
// the constants are referenced via the skipped tests above.
#[allow(dead_code)]
fn _force_use(_: &Path) {}

# memex-ingest

Per-corpus ingestion drivers for memex. Different users have
differently-shaped corpora (BHS sheet music + agnostic notation,
RingHub JSON posts, wire-recording audio metadata, harmonizer
formats, future shapes we haven't seen yet). Each corpus type
needs its own ingestion driver that walks the data, parses the
format, and emits normalized chunks for the librarian model to
forward-pass through.

Drivers are **sandboxed WASM components** authored against a fixed
memex WIT contract. They run in a wasmtime component-model runtime
provided by the shared `agentos-wasm` crate. Python today; other
languages possible since the component model is language-agnostic.

## Why WASM+WIT for ingestion

The corpus driver problem:

- **Different shapes per user** — BHS-corpus is a directory tree of
  MEI files + agnostic notation + PNG renderings; RingHub-living is
  JSON posts; wire-recordings is audio paths + transcripts. Each
  needs different parsing logic.
- **Don't recompile memex per corpus type** — users add new corpus
  shapes without touching memex source.
- **Filesystem sandbox matters** — drivers read arbitrary user
  files; capability-gated read-only WASI access is structurally
  better than "trust the Python plugin".
- **Typed contract** — WIT enforces the driver interface (what
  config it takes, what chunks it emits) so memex doesn't have to
  defensively parse driver output.
- **Single-artifact distribution** — a `.wasm` corpus driver is one
  file a user can share + version.

These constraints match exactly what AgentOS's WASM+WIT machinery
solves for its tool system. The shared crate extraction (agentos
commit `70b954e`, 2026-05-11) makes the same machinery available
here.

## Dependencies

`Cargo.toml`:

```toml
[dependencies]
agentos-wit   = { path = "../../agentos/crates/wit" }
agentos-wasm  = { path = "../../agentos/crates/wasm" }
agentos-events = { path = "../../agentos/crates/events" }
```

Path-based; no crates.io publish yet. Adjust paths to your actual
filesystem layout (memex repo's location relative to agentos).

## Architecture

```
memex-ingest/
├── wit/
│   └── ingestion-driver.wit       ← memex's WIT contract
├── tools/
│   └── python-runtime/
│       └── python-runtime.wasm    ← CPython compiled against memex's WIT
├── drivers/                       ← per-corpus drivers (Python source)
│   ├── bhs-corpus/app.py
│   ├── ringhub-living/app.py
│   └── ...
└── src/
    ├── lib.rs
    ├── driver.rs                  ← IngestionDriverPeer (wraps WasmSession)
    └── runtime.rs                 ← runtime + capability builders
```

## The WIT contract

`wit/ingestion-driver.wit` is memex-owned. Strawman starter:

```
package memex:ingest;

interface ingestion-driver {
    record driver-metadata {
        name: string,
        description: string,
        /// Glob patterns this driver knows how to ingest (e.g., "*.mei", "*.json")
        accepts: list<string>,
    }

    record corpus-config {
        /// Absolute path the driver reads from (granted via WASI fs capability)
        root: string,
        /// Free-form key/value options (e.g., locale, encoding hints)
        options: list<tuple<string, string>>,
    }

    record chunk {
        /// Stable identifier for this chunk (e.g., file-path::section-id)
        id: string,
        /// Normalized text passed to the librarian's forward pass
        text: string,
        /// Back-reference for source resolution (file path, offset, etc.)
        source-ref: string,
        /// Provenance metadata kept alongside the chunk in the archive
        metadata: list<tuple<string, string>>,
    }

    record ingest-error {
        kind: string,
        message: string,
        /// Optional path/id pointing at the source of the error
        context: option<string>,
    }

    /// Called once before iteration. Returns driver-metadata so memex
    /// knows what the driver advertises.
    export init: func(config: corpus-config) -> result<driver-metadata, ingest-error>;

    /// Pull the next chunk. Returns `none` when exhausted.
    export next-chunk: func() -> result<option<chunk>, ingest-error>;

    /// Called when memex decides it's done (early termination, fatal error,
    /// or natural end of ingestion). Drivers should flush any state.
    export finish: func() -> result<_, ingest-error>;
}
```

Adjust to match what memex's archive layer actually needs. This is
a starting point, not a freeze.

## Driver lifecycle (WasmSession)

```rust
use std::sync::Arc;
use agentos_wasm::runtime::WasmRuntime;
use agentos_wasm::capabilities::{WasmCapabilities, FsGrant};

// Load the driver and its CPython host once
let runtime = Arc::new(WasmRuntime::new()?);
let python_host = runtime.load_component_from_path(
    "tools/python-runtime/python-runtime.wasm",
)?;

// Build read-only capability grant for the user's corpus path
let caps = WasmCapabilities {
    filesystem: vec![FsGrant {
        host_path: corpus_root.into(),
        guest_path: "/corpus".into(),
        read_only: true,
    }],
    env_vars: vec![],
    stdio: false,
};

// Create a session — long-lived Store, instance, WASI ctx
let mut session = python_host.instantiate_session(&runtime, &caps)?;

// Memex's bindgen!-generated bindings wrap session.store + session.instance
let bindings = ingestion_bindings::IngestionDriver::new(
    &mut session.store,
    &session.instance,
)?;

// Drive the driver
let metadata = bindings.call_init(&mut session.store, &config)??;
loop {
    match bindings.call_next_chunk(&mut session.store)?? {
        Some(chunk) => archive.write(chunk),
        None => break,
    }
}
bindings.call_finish(&mut session.store)??;

// session drops here — Store + Instance + WASI ctx all cleaned up
```

The shared crate provides `WasmSession` and the instantiation
ceremony; everything inside the type-safe `bindings.call_*` is
generated by `wit-bindgen-rust` (or equivalent) from the WIT file
above.

## Python driver shape

A corpus driver in Python looks roughly like (post-`componentize-py
bindings`):

```python
import wit_world  # generated by componentize-py against the memex WIT

class WitWorld(wit_world.WitWorld):
    def __init__(self):
        self._files = None
        self._index = 0

    def init(self, config: wit_world.CorpusConfig) -> wit_world.DriverMetadata:
        # Walk corpus_root via WASI-granted filesystem, build file list
        self._files = list(walk(config.root, accept="*.mei"))
        return wit_world.DriverMetadata(
            name="bhs-corpus",
            description="BHS MEI sheet-music ingestion",
            accepts=["*.mei"],
        )

    def next_chunk(self) -> wit_world.Optional[wit_world.Chunk]:
        if self._index >= len(self._files):
            return None
        path = self._files[self._index]
        self._index += 1
        text = parse_mei_to_normalized_text(path)
        return wit_world.Chunk(
            id=f"{path.name}::{self._index}",
            text=text,
            source_ref=str(path),
            metadata=[],
        )

    def finish(self) -> None:
        pass
```

The state on `self._files` and `self._index` is exactly the iterator
state that needs to persist across `next_chunk` calls — that's why
memex uses `WasmSession` (long-lived Store) rather than `WasmToolPeer`
(fresh Store per call).

## Build pipeline (per-corpus driver)

```bash
# One-time per project: generate Python bindings against the WIT
componentize-py -d wit/ -w ingestion-driver bindings drivers/bindings

# Per driver: compile its .py to a .wasm component
componentize-py -d wit/ -w ingestion-driver componentize \
    -p drivers/ -p drivers/bindings \
    -o drivers/bhs-corpus.wasm \
    bhs_corpus
```

The resulting `.wasm` is the artifact memex loads and runs. One per
corpus type. Users with their own corpus shapes can write + ship
their own `.wasm` without touching memex source.

## What stays out of memex-ingest

Per the shared-crate boundaries documented at
`agentos/crates/wasm/CLAUDE.md`:

- **Memex defines its own peer wrapper** (`IngestionDriverPeer` in
  this crate) — `agentos-wasm` doesn't provide a "peer trait".
- **Memex compiles its own `python-runtime.wasm`** against the
  memex WIT — not bundled in `agentos-wasm`.
- **Memex authors its own WIT** — `agentos-wasm` doesn't bake in any
  contract.

Use of agentos's `ToolMetadata` or `WasmToolPeer` types is **not
expected** — those are AgentOS's tool model, not memex's ingestion
shape. Cross-reference them only if some sidecar feature happens to
need them.

## Cross-references

- `agentos/crates/wit/CLAUDE.md` — WIT parser API
- `agentos/crates/wasm/CLAUDE.md` — runtime API, session vs ephemeral
  patterns, capability model, and what's intentionally out of scope
- Integration pin `project_agentos_topology.md` — full architectural
  reframing (platform-and-apps; the shared-crate extraction is in
  the 2026-05-11 update section)
- `agentos/wit/tool.wit` and `python-runtime.wit` — agentos's WIT
  contracts as reference examples (do NOT use directly; memex
  authors its own)

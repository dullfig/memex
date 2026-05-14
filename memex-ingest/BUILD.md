# Building memex ingestion drivers

Drivers are WASM components compiled against `wit/ingestion-driver.wit`.
They live under `drivers/<name>/` as standalone cargo crates (not
workspace members — they target `wasm32-wasip2`, not the host).

## Prerequisites

```
rustup target add wasm32-wasip2
```

No `cargo-component`, `wit-bindgen-cli`, or `wasm-tools` needed —
`wasm32-wasip2` is the component-targeting WASI build target and
emits a ready-to-load component directly.

## Building a driver

From the driver's directory (e.g. `drivers/bhs-corpus/`):

```
cargo build --release --target wasm32-wasip2
```

Artifact lands at:

```
drivers/<name>/target/wasm32-wasip2/release/<crate_name>.wasm
```

For `bhs-corpus`:
```
drivers/bhs-corpus/target/wasm32-wasip2/release/bhs_corpus_driver.wasm   (~131 KB)
```

## Writing a new driver

1. Create `drivers/<name>/Cargo.toml` modeled on `bhs-corpus/Cargo.toml`:
   - `crate-type = ["cdylib"]`
   - `wit-bindgen = "0.41"` dep
   - Empty `[workspace]` table at the bottom to stand outside the memex
     workspace.
2. In `src/lib.rs`, expand the bindings:
   ```rust
   wit_bindgen::generate!({
       path: "../../wit",          // memex-ingest/wit
       world: "ingestion-driver",
   });
   ```
3. Implement `Guest` (the trait the macro generates):
   - `fn init(config: CorpusConfig) -> Result<DriverMetadata, IngestError>`
   - `fn next_chunk() -> Result<Option<Chunk>, IngestError>`
   - `fn finish() -> Result<(), IngestError>`
4. Persist iterator state in a `thread_local! RefCell<Option<State>>` —
   the WIT exports are static-method-shaped, so any per-instance state
   has to live there.
5. `export!(YourDriverType);` at the bottom.

## Testing

Host-side integration test pattern in `memex-ingest/tests/`:
load the built `.wasm` via `IngestionDriverPeer::load`, point it at a
real corpus path, drive the lifecycle, assert on emitted `Chunk`s.

See `tests/bhs_corpus_e2e.rs` for the working reference.

## End-to-end smoke (driver → memex → cortex)

Once the driver builds, you can dry-run it without any services up:

```
cargo run -p memex-cli -- ingest \
    --driver memex-ingest/drivers/bhs-corpus/target/wasm32-wasip2/release/bhs_corpus_driver.wasm \
    --corpus C:/src/bhs-corpus/sources \
    --dry-run --limit 5
```

Prints one line per emitted chunk (id, byte count, source_ref,
metadata count). No HTTP traffic, no shard creation, no GPU.

To actually ingest into a running stack:

1. Start cortex-server with `--enable-retrieve` pointing at the base
   Qwen2.5-3B GGUF (`C:/src/cortex/models/Qwen2.5-3B-Q4_K_M.gguf`).
2. Start memex-api with `CORTEX_URL` set to the cortex address.
3. Drop `--dry-run`:

```
cargo run -p memex-cli -- ingest \
    --driver memex-ingest/drivers/bhs-corpus/target/wasm32-wasip2/release/bhs_corpus_driver.wasm \
    --corpus C:/src/bhs-corpus/sources \
    --shard bhs.corpus.all
```

The CLI POSTs each chunk to `/v1/ingest`, which tokenizes and
appends to the shard's KV cache via cortex. After completion, the
canonical demo query goes through `/v1/retrieve`.

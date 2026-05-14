//! Operator CLI for memex.
//!
//! Current subcommands:
//!   ingest    — load a WASM corpus driver, walk its emitted chunks,
//!               POST each to memex's /v1/ingest. The driver
//!               (`bhs-corpus.wasm`, `harmonizer.wasm`, etc.) owns the
//!               corpus-specific format and shape; the CLI is the thin
//!               host that drives `init → next_chunk* → finish` and
//!               forwards chunks at the HTTP boundary.
//!
//! Future: wipe (drop a shard + sled state), smoke (run a query and
//! pretty-print hits with resolved source ids).

use std::path::PathBuf;

use agentos_wasm::runtime::WasmRuntime;
use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use memex_consent::{ConsentScope, ConsentToken};
use memex_ingest::{Chunk, CorpusConfig, DriverError, IngestionDriverPeer, GUEST_CORPUS_ROOT};
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "memex-cli", about = "Operator CLI for memex")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Load a WASM corpus driver and POST every chunk it emits to /v1/ingest.
    Ingest(IngestArgs),
}

#[derive(clap::Args)]
struct IngestArgs {
    /// Path to the compiled driver `.wasm` component (see
    /// memex-ingest/BUILD.md for the build invocation).
    #[arg(long)]
    driver: PathBuf,

    /// Corpus root on the host filesystem. Mounted read-only inside the
    /// driver's WASM sandbox at the host-default guest path.
    #[arg(long)]
    corpus: PathBuf,

    /// Memex server URL.
    #[arg(long, default_value = "http://localhost:7720")]
    memex: String,

    /// Target shard in `namespace.category.entity_id` format. Every
    /// chunk emitted by the driver lands in this single shard.
    #[arg(long, default_value = "bhs.corpus.all")]
    shard: String,

    /// Actor identity used for the consent token's `source_entity` and
    /// audit-log `actor` field.
    #[arg(long, default_value = "bhs-ingest")]
    actor: String,

    /// Mark the shard as pinned when first created (always resident on GPU).
    #[arg(long)]
    pinned: bool,

    /// Print emitted chunks without POSTing them. Useful for verifying a
    /// new driver's output before pointing it at a real memex.
    #[arg(long)]
    dry_run: bool,

    /// Stop after this many chunks. Smoke-test convenience.
    #[arg(long)]
    limit: Option<usize>,

    /// Free-form driver options passed through `init`. Repeatable.
    /// Example: --driver-option locale=en-US --driver-option strict=true
    #[arg(long = "driver-option", value_parser = parse_kv)]
    driver_options: Vec<(String, String)>,
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .ok_or_else(|| format!("expected key=value, got {s:?}"))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Ingest(args) => run_ingest(args),
    }
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

fn run_ingest(args: IngestArgs) -> Result<()> {
    let (namespace, _, _) = parse_shard(&args.shard)?;

    if !args.driver.exists() {
        bail!("driver wasm not found: {}", args.driver.display());
    }
    if !args.corpus.exists() {
        bail!("corpus root not found: {}", args.corpus.display());
    }

    // Bring up the wasm runtime and load the driver. This is the same
    // path the host integration tests exercise. wasmtime's sync API is
    // used here; running this CLI under a tokio runtime would conflict
    // with wasmtime-wasi's internal block_on.
    let runtime = WasmRuntime::new().context("creating wasm runtime")?;
    let mut peer = IngestionDriverPeer::load(&runtime, &args.driver, &args.corpus)
        .map_err(driver_to_anyhow)?;

    let config = CorpusConfig {
        root: GUEST_CORPUS_ROOT.to_owned(),
        options: args.driver_options.clone(),
    };

    let metadata = peer.init(&config).map_err(driver_to_anyhow)?;
    tracing::info!(
        driver = %args.driver.display(),
        corpus = %args.corpus.display(),
        name = %metadata.name,
        accepts = ?metadata.accepts,
        memex = %args.memex,
        shard = %args.shard,
        dry_run = args.dry_run,
        "driver initialized"
    );

    let http = if args.dry_run {
        None
    } else {
        Some(
            reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(1800))
                .build()?,
        )
    };

    if let Some(http) = &http {
        ensure_shard(http, &args.memex, &args.shard, &namespace, args.pinned)?;
    }

    let mut emitted = 0usize;
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut total_tokens: u64 = 0;

    loop {
        // Honor --limit by short-circuiting before pulling the next chunk
        // (avoids one wasted driver call past the limit).
        if let Some(limit) = args.limit {
            if emitted >= limit {
                break;
            }
        }

        let chunk = match peer.next_chunk().map_err(driver_to_anyhow)? {
            Some(c) => c,
            None => break,
        };
        emitted += 1;

        if args.dry_run {
            println!(
                "{} bytes={} source_ref={} metadata={}",
                chunk.id,
                chunk.text.len(),
                chunk.source_ref,
                chunk.metadata.len()
            );
            continue;
        }

        let http = http.as_ref().expect("http client present when not dry-run");
        match send_ingest(http, &args, &namespace, &chunk) {
            Ok(info) => {
                total_tokens += info.token_count;
                ok += 1;
                tracing::info!(
                    idx = emitted,
                    content_id = %chunk.id,
                    tokens = info.token_count,
                    offset = info.offset,
                    "ingested"
                );
            }
            Err(e) => {
                failed += 1;
                tracing::error!(
                    idx = emitted,
                    content_id = %chunk.id,
                    error = %e,
                    "ingest failed"
                );
            }
        }
    }

    // Drain any remaining driver state regardless of how we exited.
    if let Err(e) = peer.finish() {
        tracing::warn!(error = %e, "driver finish reported an error");
    }

    tracing::info!(emitted, ok, failed, total_tokens, "ingest complete");
    if failed > 0 {
        bail!("{failed} of {emitted} ingest requests failed");
    }
    Ok(())
}

fn send_ingest(
    http: &reqwest::blocking::Client,
    args: &IngestArgs,
    namespace: &str,
    chunk: &Chunk,
) -> Result<IngestHttpResponse> {
    let url = format!("{}/v1/ingest", args.memex.trim_end_matches('/'));
    let body = IngestHttpRequest {
        content_id: chunk.id.clone(),
        content: chunk.text.clone(),
        shard: args.shard.clone(),
        consent_token: consent_token(&args.actor, namespace),
    };
    let resp = http.post(&url).json(&body).send().context("POST /v1/ingest")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        bail!("/v1/ingest returned {status}: {body}");
    }
    resp.json::<IngestHttpResponse>()
        .context("decoding ingest response")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_shard(s: &str) -> Result<(String, String, String)> {
    let mut parts = s.splitn(3, '.');
    let ns = parts.next().filter(|p| !p.is_empty());
    let cat = parts.next().filter(|p| !p.is_empty());
    let id = parts.next().filter(|p| !p.is_empty());
    match (ns, cat, id) {
        (Some(a), Some(b), Some(c)) => Ok((a.to_owned(), b.to_owned(), c.to_owned())),
        _ => bail!("shard must be in `namespace.category.entity_id` form, got {s:?}"),
    }
}

fn consent_token(source_entity: &str, namespace: &str) -> ConsentToken {
    ConsentToken {
        token_id: Uuid::new_v4(),
        source_entity: source_entity.to_owned(),
        namespace: namespace.to_owned(),
        scope: ConsentScope::AllContent,
        issued_at: Utc::now(),
        expires_at: None,
        signature: vec![],
    }
}

fn ensure_shard(
    http: &reqwest::blocking::Client,
    memex: &str,
    shard: &str,
    namespace: &str,
    pinned: bool,
) -> Result<()> {
    let base = memex.trim_end_matches('/');
    let get_url = format!("{base}/v1/shards/{shard}");
    let get = http.get(&get_url).send().context("GET /v1/shards")?;
    if get.status().is_success() {
        tracing::info!(shard, "shard already exists — reusing");
        return Ok(());
    }
    if get.status() != reqwest::StatusCode::NOT_FOUND {
        let s = get.status();
        let b = get.text().unwrap_or_default();
        bail!("unexpected status from GET /v1/shards/{shard}: {s} {b}");
    }

    let create_url = format!("{base}/v1/shards");
    let body = serde_json::json!({ "shard": shard, "pinned": pinned });
    let resp = http
        .post(&create_url)
        .header("X-Memex-Namespace", namespace)
        .json(&body)
        .send()
        .context("POST /v1/shards")?;
    if !resp.status().is_success() {
        let s = resp.status();
        let b = resp.text().unwrap_or_default();
        bail!("create shard {shard} failed: {s} {b}");
    }
    tracing::info!(shard, pinned, "shard created");
    Ok(())
}

/// Map a DriverError to anyhow with the export name and structured fields
/// preserved in the message so failures are debuggable from a log line.
fn driver_to_anyhow(e: DriverError) -> anyhow::Error {
    anyhow::anyhow!(e)
}

// ---------------------------------------------------------------------------
// Wire types — local copies (don't drag in memex-api as a dep just for these)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct IngestHttpRequest {
    content_id: String,
    content: String,
    shard: String,
    consent_token: ConsentToken,
}

#[derive(Deserialize)]
struct IngestHttpResponse {
    #[allow(dead_code)]
    content_id: String,
    #[allow(dead_code)]
    shard: String,
    token_count: u64,
    offset: u64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_shard() {
        let (a, b, c) = parse_shard("bhs.corpus.all").unwrap();
        assert_eq!((a.as_str(), b.as_str(), c.as_str()), ("bhs", "corpus", "all"));
    }

    #[test]
    fn parse_invalid_shard() {
        assert!(parse_shard("bhs.corpus").is_err());
        assert!(parse_shard("bhs").is_err());
        assert!(parse_shard("").is_err());
    }

    #[test]
    fn driver_options_kv_parses() {
        let kv = parse_kv("locale=en-US").unwrap();
        assert_eq!(kv, ("locale".to_owned(), "en-US".to_owned()));
        // Values containing '=' keep the rest intact.
        let kv = parse_kv("token=abc=def").unwrap();
        assert_eq!(kv, ("token".to_owned(), "abc=def".to_owned()));
    }

    #[test]
    fn driver_options_kv_rejects_bare() {
        assert!(parse_kv("no-equals-sign").is_err());
    }
}

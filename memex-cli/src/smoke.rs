//! Retrieval smoke battery — runs a YAML-configured set of queries
//! against memex's /v1/retrieve, evaluates expectations, prints
//! pass/fail per query plus a summary.
//!
//! Two expectation kinds:
//!
//! - `positive` — at least one hit's `source_id` must match one of the
//!   given glob patterns within an optional `max_rank` window.
//!   Disambiguates "is the retrieval pipeline working at all" from
//!   "corpus thinness." Failure = bug in pipeline.
//!
//! - `negative` — the top-1 hit's score must be below `max_top_score`.
//!   Catches the case where retrieval shape works but scores are
//!   uncalibrated — every query lights up the cache.
//!
//! See `memex-cli/smoke/bhs-corpus.yaml` for the default fixture.

use std::path::Path;

use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// YAML config
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SmokeConfig {
    /// Shard list to compose for every query. Must share a namespace
    /// (memex's retrieval pipeline enforces this server-side).
    pub shards: Vec<String>,
    /// Top-K depth requested from /v1/retrieve. Default 10.
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    /// Default actor identity for the X-Memex-Actor header. CLI
    /// `--actor` overrides this.
    #[serde(default = "default_actor")]
    pub actor: String,
    pub queries: Vec<SmokeQuery>,
}

fn default_top_k() -> u32 {
    10
}
fn default_actor() -> String {
    "smoke-test".to_owned()
}

#[derive(Debug, Deserialize)]
pub struct SmokeQuery {
    pub name: String,
    pub query: String,
    pub expect: Expectation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Expectation {
    /// At least one hit's source_id must match one of `match_globs`,
    /// at rank ≤ `max_rank` (default = config's top_k).
    Positive {
        match_globs: Vec<String>,
        #[serde(default)]
        max_rank: Option<u32>,
    },
    /// The top-1 hit's score must be below `max_top_score`.
    Negative { max_top_score: f32 },
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct RetrieveHttpRequest<'a> {
    query: &'a str,
    shards: &'a [String],
    top_k: u32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RetrieveHttpResponse {
    #[allow(dead_code)]
    pub query_id: String,
    pub hits: Vec<HitDto>,
    #[allow(dead_code)]
    pub shard_count: u32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct HitDto {
    #[allow(dead_code)]
    pub shard: String,
    #[allow(dead_code)]
    pub offset: u64,
    #[allow(dead_code)]
    pub length: u32,
    pub score: f32,
    pub source_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// Per-query outcome of running the battery.
#[derive(Debug, Clone)]
pub struct QueryOutcome {
    pub name: String,
    pub pass: bool,
    /// Human-readable single-line explanation rendered after PASS/FAIL.
    pub detail: String,
}

/// Aggregate result of running the full battery.
#[derive(Debug, Clone)]
pub struct SmokeReport {
    pub outcomes: Vec<QueryOutcome>,
}

impl SmokeReport {
    pub fn pass_count(&self) -> usize {
        self.outcomes.iter().filter(|o| o.pass).count()
    }
    pub fn fail_count(&self) -> usize {
        self.outcomes.iter().filter(|o| !o.pass).count()
    }
    pub fn all_passed(&self) -> bool {
        self.fail_count() == 0
    }
}

/// Evaluate a `positive` expectation against a retrieval response.
/// Returns (pass, detail).
pub fn eval_positive(
    response: &RetrieveHttpResponse,
    match_globs: &[String],
    max_rank: Option<u32>,
    config_top_k: u32,
) -> (bool, String) {
    let limit = max_rank.unwrap_or(config_top_k) as usize;
    let set = match build_globset(match_globs) {
        Ok(s) => s,
        Err(e) => return (false, format!("invalid glob pattern: {e}")),
    };

    for (idx, hit) in response.hits.iter().take(limit).enumerate() {
        let Some(sid) = &hit.source_id else {
            continue;
        };
        if set.is_match(sid) {
            return (
                true,
                format!("rank={} source={} score={:.3}", idx + 1, sid, hit.score),
            );
        }
    }

    // No match. Surface what *did* come back so a failed positive is
    // diagnostic, not just "nope."
    let top_summary = response
        .hits
        .iter()
        .take(3)
        .map(|h| {
            let s = h.source_id.as_deref().unwrap_or("<unresolved>");
            format!("{s}@{:.3}", h.score)
        })
        .collect::<Vec<_>>()
        .join(", ");
    (
        false,
        format!(
            "no glob match within top-{limit}; top-3: [{}]",
            if top_summary.is_empty() {
                "no hits".to_owned()
            } else {
                top_summary
            }
        ),
    )
}

/// Evaluate a `negative` expectation against a retrieval response.
/// Pass if top-1 score < threshold OR there are no hits at all.
pub fn eval_negative(response: &RetrieveHttpResponse, max_top_score: f32) -> (bool, String) {
    match response.hits.first() {
        None => (true, "no hits returned".to_owned()),
        Some(h) => {
            let pass = h.score < max_top_score;
            let src = h.source_id.as_deref().unwrap_or("<unresolved>");
            if pass {
                (
                    true,
                    format!("top1 score={:.3} < threshold {:.3} (source={src})", h.score, max_top_score),
                )
            } else {
                (
                    false,
                    format!("top1 score={:.3} >= threshold {:.3} (source={src})", h.score, max_top_score),
                )
            }
        }
    }
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).with_context(|| format!("compiling glob {p:?}"))?;
        b.add(g);
    }
    b.build().context("building globset")
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Load a YAML smoke config from disk.
pub fn load_config(path: &Path) -> Result<SmokeConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading smoke config: {}", path.display()))?;
    let cfg: SmokeConfig =
        serde_yaml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    if cfg.shards.is_empty() {
        bail!("smoke config has no shards");
    }
    if cfg.queries.is_empty() {
        bail!("smoke config has no queries");
    }
    Ok(cfg)
}

/// Run the battery against a live memex. Returns the full report;
/// caller decides how to render and what exit code to use.
pub fn run(
    client: &reqwest::blocking::Client,
    memex_url: &str,
    actor: &str,
    config: &SmokeConfig,
) -> Result<SmokeReport> {
    let url = format!("{}/v1/retrieve", memex_url.trim_end_matches('/'));
    let mut outcomes = Vec::with_capacity(config.queries.len());

    for q in &config.queries {
        let body = RetrieveHttpRequest {
            query: &q.query,
            shards: &config.shards,
            top_k: config.top_k,
        };

        let resp = client
            .post(&url)
            .header("X-Memex-Actor", actor)
            .json(&body)
            .send()
            .with_context(|| format!("POST /v1/retrieve for {:?}", q.name))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            outcomes.push(QueryOutcome {
                name: q.name.clone(),
                pass: false,
                detail: format!("HTTP {status}: {body}"),
            });
            continue;
        }

        let parsed: RetrieveHttpResponse = resp
            .json()
            .with_context(|| format!("decoding /v1/retrieve response for {:?}", q.name))?;

        let (pass, detail) = match &q.expect {
            Expectation::Positive {
                match_globs,
                max_rank,
            } => eval_positive(&parsed, match_globs, *max_rank, config.top_k),
            Expectation::Negative { max_top_score } => eval_negative(&parsed, *max_top_score),
        };

        outcomes.push(QueryOutcome {
            name: q.name.clone(),
            pass,
            detail,
        });
    }

    Ok(SmokeReport { outcomes })
}

/// Render a report to stdout in the standard memex-cli smoke format.
pub fn print_report(report: &SmokeReport, config: &SmokeConfig) {
    println!("=== memex retrieval smoke ===");
    println!(
        "shards: {}    top_k: {}",
        config.shards.join(","),
        config.top_k
    );
    // Pad name column for readable columns.
    let name_w = report
        .outcomes
        .iter()
        .map(|o| o.name.len())
        .max()
        .unwrap_or(0)
        .max(8);
    for o in &report.outcomes {
        let tag = if o.pass { "PASS" } else { "FAIL" };
        println!("{tag}  {:<width$}  {}", o.name, o.detail, width = name_w);
    }
    println!("---");
    println!(
        "{} pass, {} fail",
        report.pass_count(),
        report.fail_count()
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_hit(source: Option<&str>, score: f32) -> HitDto {
        HitDto {
            shard: "bhs.corpus.all".into(),
            offset: 0,
            length: 1,
            score,
            source_id: source.map(str::to_owned),
        }
    }

    fn mk_response(hits: Vec<HitDto>) -> RetrieveHttpResponse {
        RetrieveHttpResponse {
            query_id: "q".into(),
            shard_count: 1,
            hits,
        }
    }

    #[test]
    fn positive_matches_within_max_rank() {
        let resp = mk_response(vec![
            mk_hit(Some("bhs-org/about.md"), 0.9),
            mk_hit(Some("history/people-owen-cash.md"), 0.7),
        ]);
        let (pass, detail) = eval_positive(
            &resp,
            &["history/people-owen-cash.md".into()],
            Some(3),
            10,
        );
        assert!(pass, "should match at rank 2: {detail}");
        assert!(detail.contains("rank=2"));
    }

    #[test]
    fn positive_misses_when_glob_absent_from_top_k() {
        let resp = mk_response(vec![
            mk_hit(Some("bhs-org/about.md"), 0.9),
            mk_hit(Some("forums/community-directory.md"), 0.7),
        ]);
        let (pass, detail) =
            eval_positive(&resp, &["history/*.md".into()], Some(10), 10);
        assert!(!pass);
        assert!(detail.contains("no glob match"));
        assert!(detail.contains("bhs-org/about.md@0.900"));
    }

    #[test]
    fn positive_honors_max_rank_cutoff() {
        let resp = mk_response(vec![
            mk_hit(Some("forums/community-directory.md"), 0.9),
            mk_hit(Some("history/people-owen-cash.md"), 0.7),
        ]);
        // Look only at rank 1 — should not find the rank-2 match.
        let (pass, _) =
            eval_positive(&resp, &["history/*.md".into()], Some(1), 10);
        assert!(!pass, "max_rank=1 must not see rank-2");
    }

    #[test]
    fn positive_glob_pattern_matches_subtree() {
        let resp =
            mk_response(vec![mk_hit(Some("history/people-cash.md"), 0.9)]);
        let (pass, _) =
            eval_positive(&resp, &["history/people-*.md".into()], None, 10);
        assert!(pass);
    }

    #[test]
    fn positive_skips_hits_without_source_id() {
        let resp = mk_response(vec![
            mk_hit(None, 0.9), // unresolved — should not match anything
            mk_hit(Some("history/cash.md"), 0.7),
        ]);
        let (pass, detail) =
            eval_positive(&resp, &["history/*.md".into()], None, 10);
        assert!(pass, "should skip the unresolved hit and match rank 2");
        assert!(detail.contains("rank=2"));
    }

    #[test]
    fn negative_passes_when_below_threshold() {
        let resp = mk_response(vec![mk_hit(Some("any.md"), 0.2)]);
        let (pass, _) = eval_negative(&resp, 0.5);
        assert!(pass);
    }

    #[test]
    fn negative_fails_when_at_or_above_threshold() {
        let resp = mk_response(vec![mk_hit(Some("any.md"), 0.6)]);
        let (pass, detail) = eval_negative(&resp, 0.5);
        assert!(!pass);
        assert!(detail.contains(">="));
    }

    #[test]
    fn negative_passes_with_no_hits() {
        // No retrievals at all is the cleanest negative-control pass:
        // memex correctly reported nothing.
        let resp = mk_response(vec![]);
        let (pass, detail) = eval_negative(&resp, 0.5);
        assert!(pass);
        assert!(detail.contains("no hits"));
    }

    #[test]
    fn yaml_round_trips() {
        let raw = r#"
shards: [bhs.corpus.all]
top_k: 5
queries:
  - name: founder
    query: "Who founded SPEBSQSA?"
    expect:
      kind: positive
      match_globs:
        - history/people-owen-cash.md
      max_rank: 3
  - name: off-topic
    query: "Price of tea"
    expect:
      kind: negative
      max_top_score: 0.30
"#;
        let cfg: SmokeConfig = serde_yaml::from_str(raw).unwrap();
        assert_eq!(cfg.shards, vec!["bhs.corpus.all"]);
        assert_eq!(cfg.top_k, 5);
        assert_eq!(cfg.queries.len(), 2);
        match &cfg.queries[0].expect {
            Expectation::Positive { match_globs, max_rank } => {
                assert_eq!(match_globs.len(), 1);
                assert_eq!(*max_rank, Some(3));
            }
            other => panic!("expected positive, got {other:?}"),
        }
        match &cfg.queries[1].expect {
            Expectation::Negative { max_top_score } => {
                assert!((max_top_score - 0.30).abs() < 1e-6);
            }
            other => panic!("expected negative, got {other:?}"),
        }
    }

    #[test]
    fn yaml_defaults_top_k_and_actor() {
        let raw = r#"
shards: [bhs.corpus.all]
queries:
  - name: q
    query: hi
    expect:
      kind: negative
      max_top_score: 1.0
"#;
        let cfg: SmokeConfig = serde_yaml::from_str(raw).unwrap();
        assert_eq!(cfg.top_k, 10);
        assert_eq!(cfg.actor, "smoke-test");
    }
}

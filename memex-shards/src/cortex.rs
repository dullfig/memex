use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types matching cortex's wire format
// ---------------------------------------------------------------------------

/// Info about a cache slot returned by cortex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheInfo {
    pub cache_id: String,
    pub seq_len: u64,
    #[serde(default)]
    pub max_seq_len: u64,
}

/// A single retrieval span returned by cortex's `mode: "retrieve"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawHit {
    pub shard: String,
    pub offset: u64,
    pub score: f32,
    #[serde(default)]
    pub token_text: String,
}

/// Full retrieval response from cortex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexRetrievalResponse {
    pub spans: Vec<RawHit>,
    pub query_tokens: u64,
    pub corpus_tokens: u64,
}

/// Response from cortex's tokenize endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenizeResponse {
    pub tokens: Vec<u32>,
    pub count: usize,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over cortex's cache and retrieval endpoints.
///
/// Matches cortex's actual HTTP API:
///   POST /v1/tokenize          → tokenize
///   POST /v1/cache/load        → load_cache
///   POST /v1/cache/append      → append_tokens
///   GET  /v1/cache/{id}        → check_cache
///   DELETE /v1/cache/{id}      → evict_cache
///   POST /v1/chat/completions  → retrieve (with mode: "retrieve")
#[async_trait::async_trait]
pub trait CortexClient: Send + Sync {
    /// Tokenize text using cortex's model tokenizer.
    async fn tokenize(&self, text: &str, add_bos: bool) -> Result<TokenizeResponse>;

    /// Create a cache slot and optionally replay tokens into it.
    /// Cortex auto-prepends sink tokens.
    async fn load_cache(&self, cache_id: &str, tokens: &[u32]) -> Result<CacheInfo>;

    /// Append tokens to an existing cache slot.
    async fn append_tokens(&self, cache_id: &str, tokens: &[u32]) -> Result<CacheInfo>;

    /// Check whether a cache slot is resident. Returns None if not found (404).
    async fn check_cache(&self, cache_id: &str) -> Result<Option<CacheInfo>>;

    /// Evict a cache slot from GPU memory.
    async fn evict_cache(&self, cache_id: &str) -> Result<()>;

    /// Run retrieval over composed cache shards.
    /// `query` is natural language text — cortex tokenizes it internally.
    async fn retrieve(
        &self,
        cache_shards: &[String],
        query: &str,
        top_k: u32,
    ) -> Result<CortexRetrievalResponse>;
}

// ---------------------------------------------------------------------------
// Stub (development)
// ---------------------------------------------------------------------------

/// Stub client that logs calls and returns empty results.
pub struct StubCortexClient;

#[async_trait::async_trait]
impl CortexClient for StubCortexClient {
    async fn tokenize(&self, text: &str, _add_bos: bool) -> Result<TokenizeResponse> {
        // Approximate: ~1 token per 4 chars.
        let count = (text.len() / 4).max(1);
        let tokens = vec![0u32; count];
        tracing::info!(text_len = text.len(), count, "stub: tokenize");
        Ok(TokenizeResponse { tokens, count })
    }

    async fn load_cache(&self, cache_id: &str, tokens: &[u32]) -> Result<CacheInfo> {
        tracing::info!(cache_id, token_count = tokens.len(), "stub: load_cache");
        Ok(CacheInfo {
            cache_id: cache_id.to_owned(),
            seq_len: tokens.len() as u64,
            max_seq_len: 4096,
        })
    }

    async fn append_tokens(&self, cache_id: &str, tokens: &[u32]) -> Result<CacheInfo> {
        tracing::info!(cache_id, token_count = tokens.len(), "stub: append_tokens");
        Ok(CacheInfo {
            cache_id: cache_id.to_owned(),
            seq_len: tokens.len() as u64,
            max_seq_len: 4096,
        })
    }

    async fn check_cache(&self, cache_id: &str) -> Result<Option<CacheInfo>> {
        tracing::info!(cache_id, "stub: check_cache -> None");
        Ok(None)
    }

    async fn evict_cache(&self, cache_id: &str) -> Result<()> {
        tracing::info!(cache_id, "stub: evict_cache");
        Ok(())
    }

    async fn retrieve(
        &self,
        cache_shards: &[String],
        query: &str,
        top_k: u32,
    ) -> Result<CortexRetrievalResponse> {
        tracing::info!(
            ?cache_shards,
            query_len = query.len(),
            top_k,
            "stub: retrieve -> empty"
        );
        Ok(CortexRetrievalResponse {
            spans: vec![],
            query_tokens: 0,
            corpus_tokens: 0,
        })
    }
}

// ---------------------------------------------------------------------------
// Real HTTP client
// ---------------------------------------------------------------------------

/// HTTP client that talks to a running cortex-cloud instance.
pub struct HttpCortexClient {
    base_url: String,
    http: reqwest::Client,
}

impl HttpCortexClient {
    /// Create a new client pointing at a cortex server.
    /// `base_url` should be e.g. `http://localhost:8080`.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http: reqwest::Client::new(),
        }
    }
}

// -- request/response types matching cortex's wire format --

#[derive(Serialize)]
struct CacheLoadReq {
    cache_id: String,
    tokens: Vec<u32>,
}

#[derive(Serialize)]
struct CacheAppendReq {
    cache_id: String,
    tokens: Vec<u32>,
}

#[derive(Serialize)]
struct ChatRequest {
    messages: Vec<ChatMessage>,
    cache_shards: Vec<String>,
    mode: String,
    top_k: u32,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct TokenizeReq {
    text: String,
    add_bos: bool,
}

#[async_trait::async_trait]
impl CortexClient for HttpCortexClient {
    async fn tokenize(&self, text: &str, add_bos: bool) -> Result<TokenizeResponse> {
        let url = format!("{}/v1/tokenize", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&TokenizeReq {
                text: text.to_owned(),
                add_bos,
            })
            .send()
            .await
            .context("cortex tokenize request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("cortex tokenize returned {status}: {body}");
        }

        resp.json::<TokenizeResponse>()
            .await
            .context("cortex tokenize: invalid response")
    }

    async fn load_cache(&self, cache_id: &str, tokens: &[u32]) -> Result<CacheInfo> {
        let url = format!("{}/v1/cache/load", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&CacheLoadReq {
                cache_id: cache_id.to_owned(),
                tokens: tokens.to_vec(),
            })
            .send()
            .await
            .context("cortex load_cache request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("cortex load_cache returned {status}: {body}");
        }

        resp.json::<CacheInfo>()
            .await
            .context("cortex load_cache: invalid response")
    }

    async fn append_tokens(&self, cache_id: &str, tokens: &[u32]) -> Result<CacheInfo> {
        let url = format!("{}/v1/cache/append", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&CacheAppendReq {
                cache_id: cache_id.to_owned(),
                tokens: tokens.to_vec(),
            })
            .send()
            .await
            .context("cortex append_tokens request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("cortex append_tokens returned {status}: {body}");
        }

        resp.json::<CacheInfo>()
            .await
            .context("cortex append_tokens: invalid response")
    }

    async fn check_cache(&self, cache_id: &str) -> Result<Option<CacheInfo>> {
        let url = format!("{}/v1/cache/{}", self.base_url, cache_id);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("cortex check_cache request failed")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("cortex check_cache returned {status}: {body}");
        }

        let info = resp
            .json::<CacheInfo>()
            .await
            .context("cortex check_cache: invalid response")?;
        Ok(Some(info))
    }

    async fn evict_cache(&self, cache_id: &str) -> Result<()> {
        let url = format!("{}/v1/cache/{}", self.base_url, cache_id);
        let resp = self
            .http
            .delete(&url)
            .send()
            .await
            .context("cortex evict_cache request failed")?;

        // 204 No Content = success, 404 = already gone (treat as success).
        if resp.status() == reqwest::StatusCode::NOT_FOUND
            || resp.status() == reqwest::StatusCode::NO_CONTENT
        {
            return Ok(());
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("cortex evict_cache returned {status}: {body}");
        }

        Ok(())
    }

    async fn retrieve(
        &self,
        cache_shards: &[String],
        query: &str,
        top_k: u32,
    ) -> Result<CortexRetrievalResponse> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&ChatRequest {
                messages: vec![ChatMessage {
                    role: "user".to_owned(),
                    content: query.to_owned(),
                }],
                cache_shards: cache_shards.to_vec(),
                mode: "retrieve".to_owned(),
                top_k,
            })
            .send()
            .await
            .context("cortex retrieve request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("cortex retrieve returned {status}: {body}");
        }

        resp.json::<CortexRetrievalResponse>()
            .await
            .context("cortex retrieve: invalid response")
    }
}

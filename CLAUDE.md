> **Cross-session coordination:** Before making any design/scope decision, read `C:\Users\Daniel\.claude\projects\C--src-ringhub-integration\memory\MEMORY.md` first — that folder is the shared brain across all Claude sessions on this project. Decisions pinned there supersede anything in this repo's older docs.
>
> **If something has happened to Daniel:** read `C:\src\CARETAKER.md` — the project's caretaker-handoff document.

# Memex — Semantic Indexing via Attention-Based Retrieval

**Named after:** Vannevar Bush, *As We May Think* (Atlantic Monthly, July 1945) — the original associative memory machine vision.

## What Memex Is

Memex is a semantic indexing service that replaces the traditional vector-embedding + vector-DB pipeline with a single language model whose attention over a compressed KV cache IS the index. Content is ingested via forward pass; retrieval is attention over the accumulated cache; results are position references resolved to source text.

No separate embedding model. No vector database. No ANN index. No retraining when new content arrives. One model, one cache, one retrieval call.

## Architecture — Three-Layer Stack

```
┌────────────────────────────────────────────┐
│ Raw text archive (sled)                    │
│   Source of truth for all content.         │
│   Text + metadata, indexed by opaque IDs.  │
│   Stored by the consuming platform         │
│   (e.g., AgentOS / ringhub).              │
└────────────────────────────────────────────┘
           │ ingest: forward-pass through librarian model
           ▼
┌────────────────────────────────────────────┐
│ Memex Librarian (dedicated GPU, 4090)      │
│                                             │
│   Model: ~1-7B, sigmoid attention, BASE    │
│   (not instruct — base models have better  │
│   content-aware attention patterns for     │
│   retrieval than instruct-tuned variants)  │
│                                             │
│   Cache: sharded KV pool with VMM tiering  │
│     Shared shards (pinned, always resident)│
│     Per-entity shards (loaded on demand)   │
│                                             │
│   Retrieval: attention over composed       │
│   shards → top-K positions → source refs   │
│                                             │
│   KV compression: TurboQuant (~12x)        │
└────────────────────────────────────────────┘
           │ retrieved text spans
           ▼
┌────────────────────────────────────────────┐
│ Generative model (A100, 32B Qwen Instruct) │
│   Completely stateless per request.        │
│   Input: persona + retrieved text + query  │
│   Output: response                          │
│   No cache state, no memory management.    │
└────────────────────────────────────────────┘
```

The librarian and the generative model are both served by **cortex** (`C:\src\cortex`) — same codebase, two deployments with different startup flags and different model weights.

## The Librarian Model — Requirements

### Why sigmoid attention (not softmax)

Softmax attention forces weights to sum to 1.0 across all attended positions. When nothing is strongly relevant, the model must "dump" probability mass somewhere — this creates **attention sinks** (position 0 absorbs garbage attention regardless of content). Attention sinks corrupt retrieval results by always ranking position 0 highest.

Sigmoid attention (`σ(QK^T/√d)`) gives each position an **independent** 0-1 relevance score. The model can express "nothing here is relevant" by scoring everything low. No forced normalization → no sink → clean retrieval scores.

### Why base model (not instruct)

Instruct fine-tuning biases attention toward instruction tokens, dialogue patterns, and RLHF-driven helpfulness signals. These are useful for generation but **harmful for retrieval**, where attention should reflect content relevance, not instruction-following.

Base models develop more uniform, content-aware attention patterns because they're trained purely on "understand the text." Their hidden representations are richer and less distorted. For a librarian that never generates text — just computes attention and reports high-scoring positions — a base model is the correct choice.

### Model selection criteria

Search HuggingFace and the open-source community for models matching:

1. **Sigmoid attention** (not softmax) — this is the primary filter. Look for models described as using "sigmoid attention," "gated attention," "SigLIP-style attention," or referencing the "Differential Transformer" paper (Microsoft Research). The specific Microsoft 7B sigmoid base model is the primary target.

2. **Size: 1B-7B parameters.** Must fit on a consumer 4090 (24GB) alongside a large TurboQuant-compressed KV cache pool. At int4 quantization:
   - 1B = ~500MB weights → ~23GB for cache
   - 3B = ~1.5GB weights → ~22GB for cache
   - 7B = ~4GB weights → ~19GB for cache

3. **Base (not instruct/chat).** No RLHF, no instruction tuning, no chat templates. Pure pretrained base model.

4. **Decoder-only transformer** (standard autoregressive architecture). Needed for KV cache compatibility and RoPE positional encoding.

5. **Open weights** with a license permitting commercial use (Apache 2.0, MIT, or similar).

6. **GQA (Grouped Query Attention)** preferred for KV cache efficiency — fewer KV heads means less storage per token.

### If no sigmoid model is available

Fall back to a standard softmax base model (Qwen 1.5B base, Llama 3.2 1B base) and use 4 padding sink tokens at the start of each shard as a workaround. This works but is inelegant — sigmoid is strongly preferred.

## The Shard Model — Multi-Tenant Memory

The librarian's KV cache is a **pool of named shards**, each separately addressable, loadable, evictable, and persistable.

### Shard naming convention

```
{namespace}.{category}.{entity_id}
```

Examples:
- `ringhub.shared.public` — all public platform activity
- `ringhub.shared.wiki` — wiki/doc content
- `ringhub.users.alice` — Alice's personal history
- `ringhub.tasks.winter-coordination-2027` — per-task context

### Lifecycle (VMM-style tiering)

- **Shared shards: pinned** in GPU memory (always resident, big, stable)
- **Per-entity shards: loaded on demand** when that entity queries, evicted on idle
- **Sled is source of truth** — GPU memory is a hot cache, sled is durable
- Cold-start: pod restart → shared shards reload immediately, per-entity shards reload on first query
- 404-on-missing: librarian says "not resident" → caller loads from sled → retries

### Privacy

User shards are only ever loaded when that user is the querying entity. Cross-user shards never co-exist in the same retrieval computation. Privacy is structural (enforced at shard load), not policy.

## Retrieval Mode — How It Works

The librarian runs `forward_traced` (cortex feature that captures attention scores at every layer) over the composed shard cache + query. It does NOT generate tokens.

1. Load relevant shards: `["shared.public", "shared.wiki", "user.alice"]`
2. Compose by sequential forward pass (respects RoPE positions, shared prefix optimization)
3. Run query through composed cache
4. Extract pre-softmax (or sigmoid) attention scores from last N layers
5. Rank all cache positions by attention weight
6. Return top-K as `(shard_name, offset, length, score)` tuples
7. Caller resolves positions to source text via the raw archive

The retrieval path is **deterministic** — no sampling, no generation, no temperature. Same query over same cache produces same results every time. The response JSON is constructed by Rust code, not by the model.

### Position-to-source mapping

Each shard has a sidecar that maps token positions to source content IDs. Managed by memex (out-of-band from cortex). When cortex returns `(shard: "user.alice", offset: 4521, length: 166)`, memex resolves offset 4521 via the sidecar to find `source-id: alice-dm-2026-04-03-msg-042`, then pulls the text from the archive.

## Cortex Integration

Cortex (`C:\src\cortex`) serves both the librarian and the generative model. Key endpoints:

### Librarian deployment (4090, 1-7B sigmoid base model)

Started with: `--enable-cache --enable-retrieve`

- `POST /v1/chat/completions` with `cache_shards: [...]` and `mode: "retrieve"` → returns top-K attention positions
- `POST /v1/cache/load` — load a shard from sled bytes into GPU pool
- `POST /v1/cache/append` — append new KV entries to a resident shard
- `GET /v1/cache/{id}` — check if shard is resident
- `DELETE /v1/cache/{id}` — evict shard from GPU

### Generative deployment (A100, 32B Qwen Instruct)

Started with: no cache flags (pure stateless)

- `POST /v1/chat/completions` — standard OpenAI wire format, no cache state

### Cache protocol semantics (both deployments share the codebase)

- 404 on missing shard = the protocol (never implicit creation)
- Per-user mutex on caller side serializes concurrent queries per shard
- Sled is truth, GPU is cache (asymmetric durability)
- Cortex stores opaque bytes — memex/caller owns all format decisions

## Responsibility Constraints — Structural, Not Policy

These are architectural properties of memex v1, not post-hoc policies. They exist because the retrieval capability is powerful and dual-use. See `memex-values.md` in the AgentOS memory folder for the full ethics document.

1. **Consent-gated ingestion.** Content enters the cache only if the source opted in via cryptographically signed consent token.
2. **Tamper-evident query audit trail.** Every query logged with hash chaining. Cannot be silently altered.
3. **Differential privacy for aggregate queries.** Noisy at individual level, accurate in aggregate.
4. **Access control by declared purpose.** Separate API endpoints per use case (`/retrieve/aggregate`, `/retrieve/crisis-outreach`, `/retrieve/customer-support`), each with different return semantics and audit levels.
5. **Right to erasure at the ingest layer.** Delete from archive → recompute affected shard region → done.
6. **Per-tenant namespace isolation.** Enforced at the shard loader. Cross-tenant shards never co-exist in retrieval.

## Customer Zero — RingHub Concierge

The ringhub barbershop community (14K members) is the first deployment. The concierge ("Bob") uses memex for:
- Per-user memory (each member has a personal shard)
- Community knowledge (shared shards for events, wiki, arrangements)
- Proactive behavior (cron-triggered "on this day" posts using memex retrieval)
- Multi-channel support (DM, public thread, help bubble — each a buffer)

The concierge organism lives at `C:\src\concierge` (separate repo). The ringhub web platform lives at `C:\src\RingHub`. RingHub talks to AgentOS directly over HTTP/WebSocket — the kernel exposes the listener; no intermediate bridge service.

## Related Projects

- `C:\src\cortex` — The inference engine (serves both librarian and 32B models)
- `C:\src\AgentOS` — The agent runtime platform (orchestration, triggers, addressing)
- `C:\src\engram` — Hierarchical memory engine (substrate for memex's cache management)
- `C:\src\concierge` — The ringhub concierge application (Bob)
- `C:\src\RingHub` — The barbershop social platform (Django)
- `C:\src\bhs-corpus` — Barbershop content corpus for training/testing

## Immediate Priorities

1. **Find the right sigmoid attention base model on HuggingFace.** Search criteria above. The Microsoft 7B sigmoid base model is the primary target. If not found, catalog all sigmoid-attention models and evaluate candidates by size, license, and architecture.

2. **Validate retrieval quality.** Run the $200 weekend experiment: ingest ~10K barbershop-adjacent documents, construct ~1000 synthetic query-document pairs, test retrieval quality (Recall@5, Recall@10) against vector-embedding baselines. Go/no-go decision.

3. **Bootstrap the repo structure.** Cargo workspace with crate stubs for: ingest, retrieval, shards, api, audit, consent.

## Key Design Principles

- **The cache IS the index.** No separate data structure for retrieval.
- **Memex stores opaque bytes.** Cortex/librarian owns all KV format concerns (compression, layout, versioning). Memex just stores and retrieves byte blobs.
- **Attention is the retrieval primitive.** No vector similarity, no ANN search, no separate embedding model.
- **The model is along for the ride.** The 1-7B librarian doesn't "understand" the corpus — it just computes attention patterns. The patterns ARE the retrieval.
- **The ethical version must be the best version.** Responsibility constraints make the product better (consent = higher-quality data, audit = debugging, privacy = trust), not worse.
- **Ringhub proves the architecture.** Don't build abstractions ahead of the first customer. Build for ringhub, generalize when the second customer arrives.

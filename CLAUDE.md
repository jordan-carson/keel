# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What keel is

`keel` is a Rust gRPC daemon designed to run as a Kubernetes DaemonSet sidecar on LLM inference nodes. It extends the effective context window of LLMs by providing fast, hierarchically-addressable memory at the infrastructure layer. A secondary purpose is generating training signal (retrieval patterns, eviction outcomes) for long-context curriculum training pipelines.

The caller always provides embeddings — keel is model-agnostic and does not generate them.

## Commands

```bash
# Build
cargo build

# Run all tests
cargo test

# Run a single test by name
cargo test test_write_and_read
cargo test test_add_and_search

# Run only integration tests
cargo test --test integration_write_read
cargo test --test integration_search

# Run only lib unit tests
cargo test --lib

# Build release binary
cargo build --release
```

The proto file (`proto/keel.proto`) is compiled at build time via `build.rs` using `tonic-build`. Generated code is written to `src/pb/keel.rs` and is committed — do not delete it. If you modify `keel.proto`, run `cargo build` to regenerate `src/pb/keel.rs`.

## Architecture

### Request path (Phase 1.5 + Phase 2 in progress)

```
gRPC client → KeelServer (server.rs)
                  ↓
             KeelService (implements KeelMemory tonic trait)
                  ↓ Phase 2: check session routing
             ClusterManager (cluster.rs)   ← optional; nil in single-node mode
                  ↓ if local session (or single-node)
             MemoryRegistry (registry.rs)
             ├── FlatIndex hot tier (index.rs)     ← in-memory cosine scan, all embeddings
             └── TantivyStore cold tier (store.rs) ← BM25 + embedding term search + persistence
                  ↓ emits to
             SignalExporter (signal.rs)   ← async mpsc → JSONL file
```

### Key data type

`MemoryChunk` (defined in `proto/keel.proto`, generated into `src/pb/keel.rs`) is the canonical unit of storage. It carries: `id`, `embedding` (raw f32 LE bytes), `payload` (arbitrary bytes), `session_id`, `created_at_ms`, `ttl_ms`, and `meta` (string map). The registry stores and retrieves `MemoryChunk` directly — no separate internal struct.

### Embedding encoding

Embeddings are passed as `bytes` on the wire. The convention throughout the codebase is **little-endian f32**. Use `registry::f32_to_bytes` and `registry::bytes_to_f32` for conversion. Both the server (decoding `SearchRequest.query_embedding`) and registry (storing/rebuilding the FlatIndex) must use this same convention.

### TantivyStore (store.rs) — Phase 1.5

The persistent cold tier. Schema:
- `id` (STRING|STORED) — primary key, exact-match
- `session_id` (STRING|STORED) — for session eviction
- `payload_text` (TEXT|STORED) — BM25 full-text search over payload bytes
- `embedding_terms` (TEXT) — quantized embedding tokens: top-20 dims by magnitude → "ep{i}" (positive) or "en{i}" (negative)
- `created_at_ms`, `ttl_ms` (U64|STORED)
- `raw_bytes` (BYTES|STORED) — prost-encoded MemoryChunk for lossless read

**Tantivy 0.22 API notes:**
- `TantivyDocument` is the CONCRETE struct (the old `Document`). `Document` is the TRAIT.
- `OwnedValue` carries field values; use `match v { OwnedValue::Str(s) => ..., OwnedValue::Bytes(b) => ... }` — no `as_str()`/`as_bytes()` helpers exist.
- `searcher.doc::<TantivyDocument>(addr)?` — explicit type param required.
- `IndexWriter` is behind `Mutex<>`; `IndexReader` with `ReloadPolicy::Manual` + explicit `reader.reload()` after each commit.

### MemoryRegistry internals (registry.rs)

- `FlatIndex` (hot tier) is wrapped in `Arc<tokio::sync::Mutex<>>`. Release the lock before any `.await` calls.
- `TantivyStore` is wrapped in `Arc<>`. Its methods are sync (blocking); safe to call from async context for Phase 1.5 (wrap in `spawn_blocking` in a later phase for production).
- On startup, hot tier starts empty (TODO: rebuild from Tantivy on open).
- TTL is checked lazily: on read via `store.read()`, and during search result loading.
- `evict_session` uses `store.ids_for_session()` then deletes — O(1) Tantivy index lookup.
- `enforce_capacity` calls `eviction.select_evictions(current_count, max_chunks)` — no full scan needed; LRU deque is the source of truth.

### Score semantics

`FlatIndex::search` returns similarity in `[−1, 1]` (cosine, 1.0 = identical). The registry filters `similarity < min_score`. For callers, `min_score = 0.0` returns all positive-similarity results.

### Phase 2 — Cluster modules

- `router.rs` / `SessionRouter`: consistent-hash (DefaultHasher) routing. All nodes must see the same sorted peer list to agree on ownership. `is_local(session_id)` returns true if this node owns the session.
- `cluster.rs` / `ClusterManager`: lazy tonic client pool. `get_client(addr)` creates a `connect_lazy()` channel on first use and caches it. Returns `Option<KeelMemoryClient<Channel>>`.
- `server.rs` Write handler: checks `cluster.is_local_session(session_id)` and forwards to peer if not local; falls back to local if peer is unreachable.
- Cluster is optional: `None` = single-node mode; `Some(...)` = cluster mode activated when `--peers` / `KEEL_PEERS` is non-empty.

### What is not yet wired

- Hot tier startup rebuild — FlatIndex starts empty; vectors are added on new writes only (not reloaded from Tantivy on start).
- Hot tier LRU bounding — `hot_tier_max` config field exists but not enforced; FlatIndex grows unbounded until `max_chunks` eviction.
- `SemanticSearch` fan-out — currently searches local node only; cross-node fan-out needed for Phase 2 completion.
- Gossip / peer health — no peer failure detection; peers assumed healthy.
- Unix socket transport — config currently binds TCP.

## Phase plan

- **Phase 1** ✅ complete: single-node gRPC daemon
- **Phase 1.5** ✅ complete: Tantivy storage (FlatIndex hot tier + TantivyStore)
- **Phase 2** 🔜 in progress: session-affinity routing (router + cluster modules done; fan-out search + gossip TODO)
- **Phase 3**: KV cache prefix sharing backed by `memmap2`
- **Phase 4**: S3 training signal export as Parquet via `arrow`/`parquet` crates
- **Phase 5**: Observer UI — lightweight HTTP dashboard on port 9091 for `kubectl port-forward` monitoring

## Proto / codegen

- Edit `proto/keel.proto` to change the service API.
- `build.rs` calls `tonic_build` once with both `.build_server(true).build_client(true)` — do not split into two calls.
- Generated file `src/pb/keel.rs` is checked in. `src/pb/mod.rs` contains only `include!("keel.rs");`.

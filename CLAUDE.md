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

### Request path

```
gRPC client → KeelServer (server.rs)
                  ↓
             KeelService (implements KeelMemory tonic trait)
                  ↓ delegates to
             MemoryRegistry (registry.rs)
             ├── FlatIndex (index.rs)     ← pure-Rust in-memory flat vector index
             └── sled::Db                 ← persistent KV store
                  ↓ emits to
             SignalExporter (signal.rs)   ← async mpsc → JSONL file
```

### Key data type

`MemoryChunk` (defined in `proto/keel.proto`, generated into `src/pb/keel.rs`) is the canonical unit of storage. It carries: `id`, `embedding` (raw f32 LE bytes), `payload` (arbitrary bytes), `session_id`, `created_at_ms`, `ttl_ms`, and `meta` (string map). The registry stores and retrieves `MemoryChunk` directly — no separate internal struct.

### Embedding encoding

Embeddings are passed as `bytes` on the wire. The convention throughout the codebase is **little-endian f32**. Use `registry::f32_to_bytes` and `registry::bytes_to_f32` for conversion. Both the server (decoding `SearchRequest.query_embedding`) and registry (storing/rebuilding the FlatIndex) must use this same convention.

### MemoryRegistry internals

- `FlatIndex` is wrapped in `Arc<tokio::sync::Mutex<>>`. Never hold the lock across an `.await` — acquire, do the work, drop.
- `sled::Db` is `Clone + Send + Sync`. Sled operations are synchronous.
- On startup, `MemoryRegistry::new` rebuilds the FlatIndex by scanning sled (best-effort; does not re-check TTL).
- TTL is checked lazily on read: if `created_at_ms + ttl_ms < now_ms`, the chunk is deleted and `None` is returned.
- `evict_session` does a full sled scan — O(n). Acceptable for Phase 1.

### What is not yet wired

- `EvictionPolicy` / `LruEvictionPolicy` (`eviction.rs`) — the trait exists and is correct but is not called from the registry. Capacity enforcement is a TODO.
- `VectorIndex` (`index.rs`) — the async wrapper used by integration tests; not used in the production path (the registry uses `FlatIndex` directly).
- Unix socket transport — config currently binds TCP. Plan calls for Unix socket for K8s DaemonSet use (Phase 2+).

## Phase plan

The build plan lives in `plans/keel_build_plan.md`. Implementation phases:

- **Phase 1** (complete): single-node gRPC daemon — write, read, semantic search, TTL eviction, session eviction, health, signal export
- **Phase 2**: gossip replication via `chitchat` or `memberlist-rs`; session affinity routing
- **Phase 3**: KV cache prefix sharing backed by `memmap2`
- **Phase 4**: S3 training signal export as Parquet via `arrow`/`parquet` crates

Do not begin Phase 2 until Phase 1 integration tests are green.

## Proto / codegen

- Edit `proto/keel.proto` to change the service API.
- `build.rs` calls `tonic_build` once with both `.build_server(true).build_client(true)` — do not split into two calls.
- Generated file `src/pb/keel.rs` is checked in. `src/pb/mod.rs` contains only `include!("keel.rs");`.

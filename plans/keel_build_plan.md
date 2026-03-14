# keel — Rust Distributed Memory Daemon for LLM Infrastructure

## Project Summary

`keel` is a Rust-native distributed memory database designed to be deployed as a Kubernetes DaemonSet, running as a sidecar on every inference node. Its primary purpose is to extend the effective context window of large language models far beyond their native limits by providing fast, hierarchical, semantically-addressable memory at the infrastructure layer — without requiring application-layer changes.

Secondary purpose: generate training signal (retrieval patterns, attention proxies, eviction outcomes) that can feed back into long-context curriculum training pipelines.

---

## Design Philosophy

- **Infrastructure layer, not application layer.** `keel` makes things fast and efficient. It does not make decisions about what is semantically important — that is left to the application layer or the model itself.
- **Local-first, gossip-second.** Every node owns its local memory. Cross-node replication is eventual and gossip-based. Raft consensus is reserved only for high-value persistent memories.
- **Zero-copy where possible.** Use `flatbuffers` or `capnp` for serialization. Minimize allocations on the hot path.
- **Policy-configurable.** Eviction strategy, TTL, replication factor, and retrieval thresholds are all runtime-configurable — `keel` does not hardcode memory policy.

---

## Architecture

```
[Inference Pod]
      |
      | (Unix socket / loopback gRPC)
      v
[keel sidecar daemon]
      |
      |-- [In-memory HNSW vector index]     <- L1: hot retrieval (<1ms)
      |-- [Local sled/redb store]            <- L2: warm persistent store
      |
      | (gossip protocol / tokio async)
      v
[keel cluster — other nodes]               <- L3: cross-node cache misses
      |
      v
[Training signal exporter]                 <- async, non-blocking
```

### The Five Core Subsystems

| Subsystem | Responsibility |
|---|---|
| **Registry** | Stores memory chunks with metadata (TTL, source, embedding vector, session ID) |
| **Index** | HNSW vector index for semantic retrieval; flat index for exact key lookup |
| **Replication** | Gossip-based eventual consistency across nodes; Raft for pinned memories |
| **Eviction Engine** | LRU baseline; pluggable policy interface for relevance-weighted eviction |
| **Signal Exporter** | Async log of retrieval hits/misses, evictions, and session graphs for training pipeline |

---

## Phase 1 — Local Daemon (Single Node)

**Goal:** A working `keel` daemon that accepts writes and reads over a local gRPC socket, stores vectors in an in-memory HNSW index, persists to a local store, and exposes basic metrics.

### Crates

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
tonic = "0.11"               # gRPC server
prost = "0.12"               # protobuf codegen
sled = "0.34"                # local persistent KV store (or redb)
usearch = "2"                # HNSW vector index (Rust bindings)
flatbuffers = "23"           # zero-copy serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = { version = "1", features = ["v4"] }
clap = { version = "4", features = ["derive"] }   # CLI config
```

### Proto Definition (`proto/keel.proto`)

```protobuf
syntax = "proto3";
package keel;

service KeelMemory {
  rpc Write(WriteRequest) returns (WriteResponse);
  rpc Read(ReadRequest) returns (ReadResponse);
  rpc SemanticSearch(SearchRequest) returns (SearchResponse);
  rpc Evict(EvictRequest) returns (EvictResponse);
  rpc Health(HealthRequest) returns (HealthResponse);
}

message MemoryChunk {
  string id = 1;
  bytes embedding = 2;          // f32 array, serialized
  bytes payload = 3;            // raw content bytes
  string session_id = 4;
  uint64 created_at_ms = 5;
  uint64 ttl_ms = 6;            // 0 = no expiry
  map<string, string> meta = 7;
}

message WriteRequest { MemoryChunk chunk = 1; }
message WriteResponse { string id = 1; bool ok = 2; }

message ReadRequest { string id = 1; }
message ReadResponse { MemoryChunk chunk = 1; bool found = 2; }

message SearchRequest {
  bytes query_embedding = 1;
  uint32 top_k = 2;
  float min_score = 3;
}
message SearchResponse { repeated ScoredChunk results = 1; }
message ScoredChunk { MemoryChunk chunk = 1; float score = 2; }

message EvictRequest { string session_id = 1; }
message EvictResponse { uint32 evicted_count = 1; }

message HealthRequest {}
message HealthResponse { string status = 1; uint64 chunks_stored = 2; }
```

### Directory Structure

```
keel/
├── Cargo.toml
├── build.rs                  # tonic proto codegen
├── proto/
│   └── keel.proto
├── src/
│   ├── main.rs               # daemon entrypoint, CLI, config
│   ├── config.rs             # Config struct (clap + env)
│   ├── server.rs             # tonic gRPC service impl
│   ├── registry.rs           # MemoryRegistry: in-memory + sled
│   ├── index.rs              # HNSW wrapper around usearch
│   ├── eviction.rs           # EvictionPolicy trait + LRU impl
│   ├── signal.rs             # SignalExporter: async training log
│   └── error.rs              # unified Error type
└── tests/
    ├── integration_write_read.rs
    └── integration_search.rs
```

### Key Interfaces to Implement First

**`registry.rs` — MemoryRegistry**
```rust
pub struct MemoryRegistry {
    index: Arc<Mutex<HnswIndex>>,
    store: sled::Db,
    eviction: Box<dyn EvictionPolicy>,
}

impl MemoryRegistry {
    pub async fn write(&self, chunk: MemoryChunk) -> Result<String>;
    pub async fn read(&self, id: &str) -> Result<Option<MemoryChunk>>;
    pub async fn search(&self, query: &[f32], top_k: usize, min_score: f32) -> Result<Vec<ScoredChunk>>;
    pub async fn evict_session(&self, session_id: &str) -> Result<u32>;
}
```

**`eviction.rs` — EvictionPolicy trait**
```rust
pub trait EvictionPolicy: Send + Sync {
    fn should_evict(&self, chunk: &MemoryChunk, now_ms: u64) -> bool;
    fn rank_for_eviction(&self, chunks: &[MemoryChunk]) -> Vec<String>; // returns IDs in eviction order
}

pub struct LruEviction { max_chunks: usize }
impl EvictionPolicy for LruEviction { ... }
```

**`signal.rs` — SignalExporter**
```rust
pub struct SignalEvent {
    pub event_type: SignalEventType,   // Write | Hit | Miss | Eviction
    pub session_id: String,
    pub chunk_id: Option<String>,
    pub query_embedding: Option<Vec<f32>>,
    pub timestamp_ms: u64,
}

pub struct SignalExporter {
    tx: mpsc::Sender<SignalEvent>,
}

impl SignalExporter {
    pub fn emit(&self, event: SignalEvent);  // non-blocking, fire-and-forget
}
```

### Phase 1 Acceptance Criteria

- [ ] `keel` starts as a background process, binds to a configurable Unix socket
- [ ] `WriteRequest` stores a chunk in both HNSW index and sled
- [ ] `SemanticSearch` returns correct top-k results under 5ms for indices up to 100k chunks
- [ ] TTL-expired chunks are evicted on next access or background sweep
- [ ] `Health` endpoint returns chunk count and status
- [ ] Signal events are written to a local JSONL file asynchronously
- [ ] Integration tests pass for write → read and write → search round trips

---

## Phase 2 — Multi-Node Gossip Replication

**Goal:** Multiple `keel` instances on different nodes discover each other and replicate high-priority memories via gossip. Session affinity routing is implemented.

### Additional Crates

```toml
chitchat = "0.7"             # gossip protocol (Quickwit's crate)
# OR
memberlist-rs = "0.1"        # alternative gossip
```

### New Subsystems

**Node Registry** — each `keel` instance announces itself via gossip with:
- Node ID (UUID)
- Listen address
- Current chunk count
- Session affinity table (session_id → node_id)

**Replication Policy**
- Default: replicate chunks with `ttl_ms == 0` (pinned memories) to 2 other nodes
- Gossip carries chunk IDs + embeddings only; full payload fetched on cache miss
- Session affinity: when a session is first written, the node claims it and gossips the mapping

**Cross-node Read Path**
```
Local HNSW miss
  → check gossip table for session affinity
  → gRPC fetch from owning node
  → cache locally with short TTL
```

### Phase 2 Acceptance Criteria

- [ ] Two `keel` instances discover each other on startup
- [ ] Pinned memories replicate to peer nodes within 500ms
- [ ] Session affinity routing sends reads to the correct node
- [ ] Cross-node fetch adds <10ms overhead on LAN

---

## Phase 3 — KV Cache Prefix Sharing

**Goal:** Detect shared prompt prefixes across sessions, store their KV cache representation, and serve cache hits to the inference layer.

### Design Notes

- This requires the inference framework (vLLM, TensorRT-LLM, etc.) to expose a KV cache write/read API — `keel` becomes its storage backend
- Store prefix hashes as keys; KV cache tensors as values (can be large — use memory-mapped files via `memmap2`)
- LRU eviction on KV cache store is separate from the memory chunk store

### Additional Crates

```toml
memmap2 = "0.9"              # memory-mapped file I/O for large tensors
blake3 = "1"                 # fast prefix hashing
```

### Phase 3 Acceptance Criteria

- [ ] `keel` accepts a KV cache write with a prefix hash key
- [ ] Cache hit rate is measurable via metrics endpoint
- [ ] Memory-mapped storage does not OOM on large tensor payloads

---

## Phase 4 — Training Signal Pipeline

**Goal:** Export structured training signals from the signal log to an S3-compatible store for use in long-context curriculum training.

### Signal Types to Export

| Signal | Training Use |
|---|---|
| Retrieval hit (chunk_id, query_embedding, session position) | Retrieval distillation targets |
| Retrieval miss followed by poor output | Hard negatives for compression training |
| Eviction outcome (evicted → later needed) | Memory importance supervision |
| Session graph (stitched multi-session) | Synthetic long-context training data |

### Additional Crates

```toml
aws-sdk-s3 = "1"             # S3 export
parquet = "50"               # columnar format for training pipelines
arrow = "50"                 # Arrow arrays for parquet
```

### Phase 4 Acceptance Criteria

- [ ] Signal JSONL is periodically batched and written to S3 as Parquet
- [ ] Session graph stitcher produces valid ultra-long synthetic documents
- [ ] Export is fully async and does not block the hot retrieval path

---

## Kubernetes DaemonSet Deployment

```yaml
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: keel
spec:
  selector:
    matchLabels:
      app: keel
  template:
    metadata:
      labels:
        app: keel
    spec:
      containers:
      - name: keel
        image: your-registry/keel:latest
        ports:
        - containerPort: 9090   # gRPC
        - containerPort: 9091   # metrics (Prometheus)
        volumeMounts:
        - name: keel-storage
          mountPath: /var/keel/data
        - name: keel-socket
          mountPath: /var/keel/socket
        env:
        - name: KEEL_NODE_ID
          valueFrom:
            fieldRef:
              fieldPath: spec.nodeName
        - name: KEEL_MAX_CHUNKS
          value: "500000"
        - name: KEEL_GOSSIP_PORT
          value: "9092"
      volumes:
      - name: keel-storage
        hostPath:
          path: /var/keel/data
      - name: keel-socket
        hostPath:
          path: /var/keel/socket
```

Inference pods connect to `keel` via the shared `hostPath` Unix socket — no service discovery needed within a node.

---

## Configuration Reference (`keel.toml`)

```toml
[daemon]
node_id = ""                  # auto-generated if empty
socket_path = "/var/keel/socket/keel.sock"
grpc_port = 9090
metrics_port = 9091

[storage]
data_dir = "/var/keel/data"
max_chunks = 500_000
sled_cache_mb = 512

[index]
dimensions = 1536             # match your embedding model
ef_construction = 200         # HNSW build quality
ef_search = 100               # HNSW search quality
max_connections = 16

[eviction]
policy = "lru"                # lru | ttl | relevance
lru_max_chunks = 500_000
ttl_sweep_interval_secs = 60

[replication]
enabled = false               # set true in Phase 2
gossip_port = 9092
replication_factor = 2
pin_ttl_threshold_ms = 0      # chunks with ttl=0 are replicated

[signal]
enabled = true
output_path = "/var/keel/data/signals.jsonl"
export_to_s3 = false
s3_bucket = ""
s3_prefix = "keel/signals/"
flush_interval_secs = 30
```

---

## What to Build First (Instructions for Claude Code)

1. **Scaffold the Cargo workspace** with the directory structure above
2. **Write `proto/keel.proto`** and configure `build.rs` for tonic codegen
3. **Implement `index.rs`** — wrap `usearch` with a clean `HnswIndex` struct supporting `insert`, `search`, and `remove`
4. **Implement `registry.rs`** — `MemoryRegistry` backed by the HNSW index + sled; no replication yet
5. **Implement `eviction.rs`** — `EvictionPolicy` trait and `LruEviction`
6. **Implement `server.rs`** — tonic service impl that delegates to `MemoryRegistry`
7. **Implement `signal.rs`** — `SignalExporter` with a tokio mpsc channel and async JSONL writer
8. **Wire up `main.rs`** — parse config, start daemon, bind socket
9. **Write integration tests** for write → read and write → search
10. **Add a minimal Dockerfile** for building the release binary

Do not implement gossip replication (Phase 2) until Phase 1 integration tests are green.

---

## Open Design Questions (Decide Before Phase 2)

- **Embedding ownership:** Does `keel` generate embeddings (requires a local model), or does the caller always provide them? Recommendation: caller provides — keeps `keel` model-agnostic.
- **Payload size limits:** What is the max size of a memory chunk payload? Suggest 64KB default, configurable.
- **Consistency model for pinned memories:** Is Raft worth the complexity, or is 2-of-N gossip replication sufficient for the MVP?
- **KV cache storage:** Should Phase 3 live in the same binary or a separate `keel-kvcache` sidecar?

// Configuration management with clap + env variables

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "keel")]
#[command(about = "Distributed Memory Daemon for LLM Infrastructure")]
pub struct Config {
    /// Bind address for the gRPC server
    #[clap(long, default_value = "127.0.0.1:50051")]
    pub bind_address: String,

    /// Data directory for persistent Tantivy storage
    #[clap(long, default_value = "./data")]
    pub data_dir: String,

    /// Maximum memory chunks to store before LRU eviction triggers
    #[clap(long, default_value = "100000")]
    pub max_entries: usize,

    /// Vector dimension size
    #[clap(long, default_value = "1536")]
    pub vector_dim: usize,

    /// Maximum vectors to hold in the in-memory hot tier
    #[clap(long, default_value = "10000")]
    pub hot_tier_max: usize,

    /// Path for the signal JSONL output file
    #[clap(long, default_value = "./signals.jsonl")]
    pub signal_output_path: String,

    /// Directory for KV cache prefix-sharing files (Phase 3)
    #[clap(long, default_value = "./kvcache", env = "KEEL_KV_CACHE_DIR")]
    pub kv_cache_dir: String,

    /// Maximum number of KV cache entries before LRU eviction
    #[clap(long, default_value = "1000", env = "KEEL_KV_CACHE_MAX")]
    pub kv_cache_max_entries: usize,

    /// Stable node ID for cluster routing (defaults to $HOSTNAME)
    #[clap(long, env = "KEEL_NODE_ID")]
    pub node_id: Option<String>,

    /// Comma-separated list of peer keel gRPC addresses for Phase 2 multi-node routing.
    /// Example: keel-node-1:50051,keel-node-2:50051
    #[clap(long, env = "KEEL_PEERS", default_value = "")]
    pub peers: String,

    /// Enable verbose logging
    #[clap(long, short, default_value = "false")]
    pub verbose: bool,
}

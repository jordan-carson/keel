// Configuration management with clap + env variables

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "keel")]
#[command(about = "Distributed Memory Daemon for LLM Infrastructure")]
pub struct Config {
    /// Bind address for the gRPC server
    #[clap(long, default_value = "127.0.0.1:50051")]
    pub bind_address: String,

    /// Data directory for persistent storage
    #[clap(long, default_value = "./data")]
    pub data_dir: String,

    /// Maximum memory entries to store
    #[clap(long, default_value = "100000")]
    pub max_entries: usize,

    /// Vector dimension size
    #[clap(long, default_value = "1536")]
    pub vector_dim: usize,

    /// HNSW m parameter
    #[clap(long, default_value = "16")]
    pub hnsw_m: usize,

    /// HNSW ef_construction parameter
    #[clap(long, default_value = "200")]
    pub hnsw_ef_construction: usize,

    /// Path for the signal JSONL output file
    #[clap(long, default_value = "./signals.jsonl")]
    pub signal_output_path: String,

    /// Enable verbose logging
    #[clap(long, short, default_value = "false")]
    pub verbose: bool,
}

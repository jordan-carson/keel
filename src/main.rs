// Keel - Distributed Memory Daemon for LLM Infrastructure
// Main entrypoint

mod config;
mod error;
mod pb;
mod server;
mod registry;
mod index;
mod eviction;
mod signal;

use config::Config;
use registry::MemoryRegistry;
use server::KeelServer;
use signal::SignalExporter;
use clap::Parser;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_target(false)
        .init();

    let config = Config::parse();

    tracing::info!("Starting keel daemon on {}", config.bind_address);

    let registry = Arc::new(
        MemoryRegistry::new(
            &config.data_dir,
            config.vector_dim,
            config.hnsw_ef_construction,
            config.hnsw_m,
        )?
    );

    let signal = Arc::new(SignalExporter::new(&config.signal_output_path));

    let server = KeelServer::new(config, registry, signal);
    server.run().await?;

    Ok(())
}

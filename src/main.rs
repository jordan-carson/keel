// Keel - Distributed Memory Daemon for LLM Infrastructure
// Main entrypoint

mod config;
mod error;
mod server;
mod registry;
mod index;
mod eviction;
mod signal;

use config::Config;
use server::KeelServer;
use clap::Parser;
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_target(false)
        .init();

    // Parse CLI arguments and config
    let config = Config::parse();

    tracing::info!("Starting keel daemon on {}", config.bind_address);

    // Create and start the gRPC server
    let server = KeelServer::new(config).await?;
    server.run().await?;

    Ok(())
}

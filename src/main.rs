// Keel - Distributed Memory Daemon for LLM Infrastructure
// Main entrypoint

mod config;
mod error;
mod server;

use clap::Parser;
use config::Config;
use keel_cluster::cluster::ClusterManager;
use keel_signal::SignalExporter;
use keel_store::registry::MemoryRegistry;
use server::KeelServer;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let config = Config::parse();

    tracing::info!("Starting keel daemon on {}", config.bind_address);

    let registry = Arc::new(MemoryRegistry::new(
        &config.data_dir,
        config.vector_dim,
        config.hot_tier_max,
        config.max_entries,
    )?);

    let signal = Arc::new(SignalExporter::new(&config.signal_output_path));

    let cluster = {
        let peers: Vec<String> = config
            .peers
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if !peers.is_empty() {
            let node_id = config
                .node_id
                .clone()
                .or_else(|| std::env::var("HOSTNAME").ok())
                .unwrap_or_else(|| "node-0".to_string());

            tracing::info!(
                "Cluster mode: node_id={}, peers={:?}",
                node_id,
                peers
            );

            Some(Arc::new(ClusterManager::new(
                node_id,
                config.bind_address.clone(),
                peers,
            )))
        } else {
            tracing::info!("Single-node mode (set KEEL_PEERS to enable clustering)");
            None
        }
    };

    let server = KeelServer::new(config, registry, signal, cluster);
    server.run().await?;

    Ok(())
}

// gRPC Server implementation using tonic

use crate::config::Config;
use crate::error::Result;
use tracing;

pub struct KeelServer {
    config: Config,
}

impl KeelServer {
    pub async fn new(config: Config) -> Result<Self> {
        Ok(Self { config })
    }

    pub async fn run(self) -> Result<()> {
        let addr: std::net::SocketAddr = self.config.bind_address.parse()
            .map_err(|e| crate::error::KeelError::Config(format!("Invalid address: {}", e)))?;

        tracing::info!("Keel server would listen on {}", addr);
        
        // TODO: Add gRPC service implementation
        // For now, just keep the server alive
        tokio::signal::ctrl_c().await
            .map_err(|e| crate::error::KeelError::Config(format!("Failed to listen for ctrl-c: {}", e)))?;

        Ok(())
    }
}

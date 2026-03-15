// ClusterManager: Phase 2 multi-node session routing and peer forwarding.

use crate::router::SessionRouter;
use keel_proto::pb::keel_memory_client::KeelMemoryClient;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::transport::{Channel, Endpoint};

pub struct ClusterManager {
    pub local_id: String,
    router: SessionRouter,
    clients: Arc<RwLock<HashMap<String, KeelMemoryClient<Channel>>>>,
}

impl ClusterManager {
    pub fn new(local_id: String, local_addr: String, peer_addrs: Vec<String>) -> Self {
        let mut all_nodes = peer_addrs.clone();
        if !all_nodes.contains(&local_addr) {
            all_nodes.push(local_addr.clone());
        }
        let router = SessionRouter::new(all_nodes, local_addr);
        Self {
            local_id,
            router,
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn is_local_session(&self, session_id: &str) -> bool {
        self.router.is_local(session_id)
    }

    pub fn node_for_session(&self, session_id: &str) -> String {
        self.router.node_for_session(session_id).to_string()
    }

    pub fn peer_count(&self) -> usize {
        self.router.peer_count()
    }

    pub async fn get_client(&self, addr: &str) -> Option<KeelMemoryClient<Channel>> {
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(addr) {
                return Some(client.clone());
            }
        }

        let uri = format!("http://{}", addr);
        let endpoint = match Endpoint::from_shared(uri) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("ClusterManager: invalid peer address {}: {}", addr, e);
                return None;
            }
        };
        let channel = endpoint.connect_lazy();
        let client = KeelMemoryClient::new(channel);

        let mut clients = self.clients.write().await;
        clients.insert(addr.to_string(), client.clone());
        Some(client)
    }
}

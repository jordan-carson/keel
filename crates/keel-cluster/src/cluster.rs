// ClusterManager: Phase 2 multi-node session routing and peer forwarding.
//
// Dead-peer circuit breaker
// -------------------------
// When a peer RPC fails, that peer is marked dead for DEAD_PEER_TIMEOUT_SECS.
// Fan-out searches skip dead peers to avoid blocking on unavailable nodes.
// The dead mark expires automatically — no background task required.
//
// Fan-out search
// --------------
// `fan_out_search` fires concurrent SearchRPCs to all live peers, merges
// the scored results, and returns them. The caller (server.rs) is responsible
// for deduplication and final top-k truncation after merging with local results.

use crate::router::SessionRouter;
use keel_proto::pb::keel_memory_client::KeelMemoryClient;
use keel_proto::pb::{ScoredChunk, SearchRequest};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::RwLock as AsyncRwLock;
use tonic::transport::{Channel, Endpoint};
use tonic::Request;

const DEAD_PEER_TIMEOUT_SECS: u64 = 30;

pub struct ClusterManager {
    pub local_id: String,
    router: SessionRouter,
    /// Cached lazy-connect clients keyed by peer gRPC address.
    clients: Arc<AsyncRwLock<HashMap<String, KeelMemoryClient<Channel>>>>,
    /// Dead-peer expiry map. Entry present and in the future ⟹ peer is dead.
    dead_until: Arc<RwLock<HashMap<String, Instant>>>,
}

impl ClusterManager {
    /// Build the cluster manager.
    ///
    /// `peer_addrs` should NOT include the local address; it is added automatically.
    pub fn new(local_id: String, local_addr: String, peer_addrs: Vec<String>) -> Self {
        let mut all_nodes = peer_addrs.clone();
        if !all_nodes.contains(&local_addr) {
            all_nodes.push(local_addr.clone());
        }
        let router = SessionRouter::new(all_nodes, local_addr);
        Self {
            local_id,
            router,
            clients: Arc::new(AsyncRwLock::new(HashMap::new())),
            dead_until: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Returns `true` if this node is the authoritative owner of `session_id`.
    pub fn is_local_session(&self, session_id: &str) -> bool {
        self.router.is_local(session_id)
    }

    /// Returns the peer gRPC address that owns `session_id`.
    pub fn node_for_session(&self, session_id: &str) -> String {
        self.router.node_for_session(session_id).to_string()
    }

    /// Number of configured peer nodes (total nodes minus self).
    pub fn peer_count(&self) -> usize {
        self.router.peer_count()
    }

    /// All peer addresses (excludes self).
    pub fn peer_addrs(&self) -> Vec<String> {
        self.router.peer_addrs()
    }

    /// Return a lazily-connected gRPC client for `addr`, creating one if needed.
    /// Returns `None` if the address is invalid.
    pub async fn get_client(&self, addr: &str) -> Option<KeelMemoryClient<Channel>> {
        // Fast path: return cached client
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

    /// Fan-out `req` to all live peers concurrently.
    ///
    /// Results from each peer are collected and returned flat; the caller merges
    /// them with local results and performs deduplication + top-k truncation.
    /// Failed peers are automatically marked dead for `DEAD_PEER_TIMEOUT_SECS`.
    pub async fn fan_out_search(&self, req: SearchRequest) -> Vec<ScoredChunk> {
        let peers = self.peer_addrs();
        if peers.is_empty() {
            return vec![];
        }

        let mut handles = Vec::new();
        for addr in peers {
            if self.is_dead(&addr) {
                tracing::debug!("Skipping dead peer {} for fan-out search", addr);
                continue;
            }
            let Some(mut client) = self.get_client(&addr).await else {
                continue;
            };
            let req_clone = req.clone();
            let dead_map = Arc::clone(&self.dead_until);
            let addr_owned = addr.clone();
            handles.push(tokio::spawn(async move {
                match client.semantic_search(Request::new(req_clone)).await {
                    Ok(resp) => resp.into_inner().results,
                    Err(e) => {
                        tracing::warn!(
                            "Peer {} fan-out search failed: {}; marking dead for {}s",
                            addr_owned,
                            e,
                            DEAD_PEER_TIMEOUT_SECS
                        );
                        let mut dead = dead_map.write().unwrap();
                        dead.insert(
                            addr_owned,
                            Instant::now() + Duration::from_secs(DEAD_PEER_TIMEOUT_SECS),
                        );
                        vec![]
                    }
                }
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            if let Ok(r) = handle.await {
                results.extend(r);
            }
        }
        results
    }

    // ---------- dead-peer helpers ----------

    /// Returns `true` if `addr` is currently in the dead-peer window.
    pub fn is_dead(&self, addr: &str) -> bool {
        let dead = self.dead_until.read().unwrap();
        dead.get(addr).map(|&until| Instant::now() < until).unwrap_or(false)
    }

    /// Manually mark a peer dead (used by the Write forwarding path in server.rs).
    pub fn mark_dead(&self, addr: &str) {
        let mut dead = self.dead_until.write().unwrap();
        dead.insert(
            addr.to_string(),
            Instant::now() + Duration::from_secs(DEAD_PEER_TIMEOUT_SECS),
        );
    }

    /// Clear the dead mark for a peer (useful in tests or manual recovery).
    pub fn clear_dead(&self, addr: &str) {
        let mut dead = self.dead_until.write().unwrap();
        dead.remove(addr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(local: &str, peers: &[&str]) -> ClusterManager {
        ClusterManager::new(
            local.to_string(),
            local.to_string(),
            peers.iter().map(|s| s.to_string()).collect(),
        )
    }

    #[test]
    fn test_dead_peer_tracking() {
        let cm = manager("node-a:50051", &["node-b:50051"]);
        assert!(!cm.is_dead("node-b:50051"));
        cm.mark_dead("node-b:50051");
        assert!(cm.is_dead("node-b:50051"));
        cm.clear_dead("node-b:50051");
        assert!(!cm.is_dead("node-b:50051"));
    }

    #[test]
    fn test_peer_addrs_excludes_self() {
        let cm = manager("node-a:50051", &["node-b:50051", "node-c:50051"]);
        let peers = cm.peer_addrs();
        assert_eq!(peers.len(), 2);
        assert!(!peers.contains(&"node-a:50051".to_string()));
    }

    #[test]
    fn test_single_node_no_peers() {
        let cm = manager("node-a:50051", &[]);
        assert_eq!(cm.peer_count(), 0);
        assert!(cm.peer_addrs().is_empty());
    }
}

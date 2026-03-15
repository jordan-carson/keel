// SessionRouter: consistent-hash session affinity for Phase 2 multi-node routing.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub struct SessionRouter {
    nodes: Vec<String>,
    local_addr: String,
}

impl SessionRouter {
    pub fn new(mut nodes: Vec<String>, local_addr: String) -> Self {
        nodes.sort();
        nodes.dedup();
        Self { nodes, local_addr }
    }

    pub fn node_for_session(&self, session_id: &str) -> &str {
        if self.nodes.is_empty() {
            return &self.local_addr;
        }
        let mut hasher = DefaultHasher::new();
        session_id.hash(&mut hasher);
        let idx = (hasher.finish() as usize) % self.nodes.len();
        &self.nodes[idx]
    }

    pub fn is_local(&self, session_id: &str) -> bool {
        self.node_for_session(session_id) == self.local_addr.as_str()
    }

    pub fn peer_count(&self) -> usize {
        self.nodes.len().saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_node_is_always_local() {
        let router = SessionRouter::new(vec!["127.0.0.1:50051".into()], "127.0.0.1:50051".into());
        assert!(router.is_local("any-session"));
    }

    #[test]
    fn test_deterministic_routing() {
        let nodes = vec![
            "node-a:50051".to_string(),
            "node-b:50051".to_string(),
            "node-c:50051".to_string(),
        ];
        let router = SessionRouter::new(nodes, "node-a:50051".into());
        let first = router.node_for_session("session-xyz").to_string();
        for _ in 0..10 {
            assert_eq!(router.node_for_session("session-xyz"), first);
        }
    }

    #[test]
    fn test_distribution_across_nodes() {
        let nodes: Vec<String> = (0..3).map(|i| format!("node-{}:50051", i)).collect();
        let router = SessionRouter::new(nodes, "node-0:50051".into());
        let mut owners: std::collections::HashSet<String> = std::collections::HashSet::new();
        for i in 0..100 {
            let session = format!("session-{}", i);
            owners.insert(router.node_for_session(&session).to_string());
        }
        assert!(owners.len() >= 2, "Expected distribution across nodes, got {:?}", owners);
    }
}

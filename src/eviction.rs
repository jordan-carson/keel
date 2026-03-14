// EvictionPolicy trait and LRU implementation

use crate::pb::MemoryChunk;
use std::collections::VecDeque;

/// Trait for memory eviction policies
pub trait EvictionPolicy: Send + Sync {
    /// Returns IDs to evict to bring the store back to `capacity`.
    fn select_evictions(&self, entries: &[MemoryChunk], capacity: usize) -> Vec<String>;

    fn on_access(&mut self, id: &str);
    fn on_write(&mut self, id: &str);
    fn on_delete(&mut self, id: &str);
}

/// Least Recently Used eviction policy.
/// Tracks access order via a VecDeque (front = most recent).
pub struct LruEvictionPolicy {
    /// Front = most recently used, back = least recently used.
    access_order: VecDeque<String>,
}

impl LruEvictionPolicy {
    pub fn new() -> Self {
        Self {
            access_order: VecDeque::new(),
        }
    }
}

impl Default for LruEvictionPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl EvictionPolicy for LruEvictionPolicy {
    fn select_evictions(&self, entries: &[MemoryChunk], capacity: usize) -> Vec<String> {
        if entries.len() <= capacity {
            return vec![];
        }
        let to_evict = entries.len() - capacity;
        // Build a set of IDs that actually exist in the store right now
        let existing: std::collections::HashSet<&str> =
            entries.iter().map(|e| e.id.as_str()).collect();
        // Evict from the back of the deque (least recently used)
        self.access_order
            .iter()
            .rev()
            .filter(|id| existing.contains(id.as_str()))
            .take(to_evict)
            .cloned()
            .collect()
    }

    fn on_access(&mut self, id: &str) {
        if let Some(pos) = self.access_order.iter().position(|x| x == id) {
            self.access_order.remove(pos);
        }
        self.access_order.push_front(id.to_string());
    }

    fn on_write(&mut self, id: &str) {
        // Treat a write as the most recent access
        if let Some(pos) = self.access_order.iter().position(|x| x == id) {
            self.access_order.remove(pos);
        }
        self.access_order.push_front(id.to_string());
    }

    fn on_delete(&mut self, id: &str) {
        if let Some(pos) = self.access_order.iter().position(|x| x == id) {
            self.access_order.remove(pos);
        }
    }
}

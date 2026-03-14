// EvictionPolicy trait and LRU implementation
#![allow(dead_code)]

use crate::pb::MemoryChunk;
use std::collections::VecDeque;

/// Trait for memory eviction policies
pub trait EvictionPolicy: Send + Sync {
    /// Returns IDs of entries to evict when over capacity
    fn select_evictions(&self, entries: &[MemoryChunk], capacity: usize) -> Vec<String>;

    fn on_access(&mut self, id: &str);
    fn on_write(&mut self, id: &str);
    fn on_delete(&mut self, id: &str);
}

/// Least Recently Used eviction policy
pub struct LruEvictionPolicy {
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
        // Evict oldest by created_at_ms as LRU proxy
        let mut sorted: Vec<_> = entries
            .iter()
            .map(|e| (e.id.clone(), e.created_at_ms))
            .collect();
        sorted.sort_by_key(|(_, t)| *t);
        let to_evict = entries.len() - capacity;
        sorted.into_iter().take(to_evict).map(|(id, _)| id).collect()
    }

    fn on_access(&mut self, id: &str) {
        if let Some(pos) = self.access_order.iter().position(|x| x == id) {
            self.access_order.remove(pos);
        }
        self.access_order.push_front(id.to_string());
    }

    fn on_write(&mut self, id: &str) {
        self.access_order.push_front(id.to_string());
    }

    fn on_delete(&mut self, id: &str) {
        if let Some(pos) = self.access_order.iter().position(|x| x == id) {
            self.access_order.remove(pos);
        }
    }
}

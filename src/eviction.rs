// EvictionPolicy trait and LRU implementation

use crate::registry::MemoryEntry;
use std::collections::{HashMap, VecDeque};

/// Trait for memory eviction policies
pub trait EvictionPolicy: Send + Sync {
    /// Returns entries to evict when capacity is exceeded
    fn select_evictions(&self, entries: &[MemoryEntry], capacity: usize) -> Vec<String>;

    /// Notify the policy of an access
    fn on_access(&mut self, id: &str);

    /// Notify the policy of a write
    fn on_write(&mut self, id: &str);

    /// Notify the policy of a deletion
    fn on_delete(&mut self, id: &str);
}

/// Least Recently Used eviction policy
pub struct LruEvictionPolicy {
    access_order: VecDeque<String>,
    access_count: HashMap<String, usize>,
}

impl LruEvictionPolicy {
    pub fn new() -> Self {
        Self {
            access_order: VecDeque::new(),
            access_count: HashMap::new(),
        }
    }
}

impl Default for LruEvictionPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl EvictionPolicy for LruEvictionPolicy {
    fn select_evictions(&self, entries: &[MemoryEntry], capacity: usize) -> Vec<String> {
        if entries.len() <= capacity {
            return vec![];
        }

        let mut sorted: Vec<_> = entries
            .iter()
            .map(|e| (e.id.clone(), e.accessed_at))
            .collect();

        sorted.sort_by_key(|(_, accessed)| *accessed);

        let to_evict = entries.len() - capacity;
        sorted
            .into_iter()
            .take(to_evict)
            .map(|(id, _)| id)
            .collect()
    }

    fn on_access(&mut self, id: &str) {
        // Move to front if exists
        if let Some(pos) = self.access_order.iter().position(|x| x == id) {
            self.access_order.remove(pos);
        }
        self.access_order.push_front(id.to_string());
    }

    fn on_write(&mut self, id: &str) {
        self.access_order.push_front(id.to_string());
        *self.access_count.entry(id.to_string()).or_insert(0) += 1;
    }

    fn on_delete(&mut self, id: &str) {
        if let Some(pos) = self.access_order.iter().position(|x| x == id) {
            self.access_order.remove(pos);
        }
        self.access_count.remove(id);
    }
}

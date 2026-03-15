// EvictionPolicy trait and LRU implementation

use std::collections::VecDeque;

pub trait EvictionPolicy: Send + Sync {
    fn select_evictions(&self, current_count: usize, capacity: usize) -> Vec<String>;
    fn on_access(&mut self, id: &str);
    fn on_write(&mut self, id: &str);
    fn on_delete(&mut self, id: &str);
}

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
    fn select_evictions(&self, current_count: usize, capacity: usize) -> Vec<String> {
        if current_count <= capacity {
            return vec![];
        }
        let to_evict = current_count - capacity;
        self.access_order
            .iter()
            .rev()
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

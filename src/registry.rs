// MemoryRegistry: in-memory cache + sled persistent storage

use crate::error::{KeelError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub namespace: String,
    pub key: String,
    pub value: Vec<u8>,
    pub vector: Option<Vec<f32>>,
    pub created_at: u64,
    pub accessed_at: u64,
    pub access_count: u64,
}

pub struct MemoryRegistry {
    entries: Arc<RwLock<HashMap<String, MemoryEntry>>>,
    // TODO: Add sled DB handle
}

impl MemoryRegistry {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn write(&self, namespace: String, key: String, value: Vec<u8>, vector: Option<Vec<f32>>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let entry = MemoryEntry {
            id: id.clone(),
            namespace,
            key,
            value,
            vector,
            created_at: now,
            accessed_at: now,
            access_count: 0,
        };

        let mut entries = self.entries.write().await;
        entries.insert(id.clone(), entry);

        Ok(id)
    }

    pub async fn read(&self, id: &str) -> Result<Option<MemoryEntry>> {
        let entries = self.entries.read().await;
        let mut entry = entries.get(id).cloned();

        if let Some(ref mut e) = entry {
            e.accessed_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            e.access_count += 1;
        }

        Ok(entry)
    }

    pub async fn list(&self, namespace: &str) -> Result<Vec<MemoryEntry>> {
        let entries = self.entries.read().await;
        let result: Vec<MemoryEntry> = entries
            .values()
            .filter(|e| e.namespace == namespace)
            .cloned()
            .collect();
        Ok(result)
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        let mut entries = self.entries.write().await;
        Ok(entries.remove(id).is_some())
    }

    pub async fn count(&self) -> Result<usize> {
        let entries = self.entries.read().await;
        Ok(entries.len())
    }
}

impl Default for MemoryRegistry {
    fn default() -> Self {
        Self::new()
    }
}

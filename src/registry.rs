// MemoryRegistry: HNSW in-memory index + sled persistent store

use crate::error::{KeelError, Result};
use crate::index::HnswIndex;
use crate::pb::{MemoryChunk, ScoredChunk};
use prost::Message;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

pub struct MemoryRegistry {
    index: Arc<Mutex<HnswIndex>>,
    store: sled::Db,
}

impl MemoryRegistry {
    pub fn new(data_dir: &str, dim: usize, ef_construction: usize, max_connections: usize) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| KeelError::Registry(e.to_string()))?;

        let store = sled::open(data_dir)?;
        let mut index = HnswIndex::new(dim, ef_construction, max_connections);

        // Rebuild HNSW index from persisted data on startup
        for item in store.iter() {
            let (key, val) = item?;
            if let Ok(chunk) = MemoryChunk::decode(val.as_ref()) {
                if !chunk.embedding.is_empty() {
                    if let Ok(vector) = bytes_to_f32(&chunk.embedding) {
                        if vector.len() == dim {
                            let id = String::from_utf8_lossy(&key).to_string();
                            let _ = index.insert(&id, &vector);
                        }
                    }
                }
            }
        }

        Ok(Self {
            index: Arc::new(Mutex::new(index)),
            store,
        })
    }

    pub async fn write(&self, mut chunk: MemoryChunk) -> Result<String> {
        if chunk.id.is_empty() {
            chunk.id = Uuid::new_v4().to_string();
        }
        if chunk.created_at_ms == 0 {
            chunk.created_at_ms = now_ms();
        }

        let id = chunk.id.clone();

        // Insert into HNSW if embedding is present
        if !chunk.embedding.is_empty() {
            let vector = bytes_to_f32(&chunk.embedding)?;
            let mut index = self.index.lock().await;
            index.insert(&id, &vector)?;
        }

        // Persist to sled
        self.store.insert(id.as_bytes(), chunk.encode_to_vec())?;

        Ok(id)
    }

    pub async fn read(&self, id: &str) -> Result<Option<MemoryChunk>> {
        match self.store.get(id.as_bytes())? {
            None => Ok(None),
            Some(val) => {
                let chunk = MemoryChunk::decode(val.as_ref())?;
                // TTL check: evict on access if expired
                if chunk.ttl_ms > 0 && chunk.created_at_ms + chunk.ttl_ms < now_ms() {
                    self.delete(id).await?;
                    return Ok(None);
                }
                Ok(Some(chunk))
            }
        }
    }

    pub async fn search(&self, query: &[f32], top_k: usize, min_score: f32) -> Result<Vec<ScoredChunk>> {
        let raw_results = {
            let index = self.index.lock().await;
            index.search(query, top_k)?
        };

        let mut scored = Vec::new();
        for (id, score) in raw_results {
            if score > min_score {
                continue;
            }
            if let Some(chunk) = self.read(&id).await? {
                scored.push(ScoredChunk { chunk: Some(chunk), score });
            }
        }
        Ok(scored)
    }

    pub async fn evict_session(&self, session_id: &str) -> Result<u32> {
        // Collect matching IDs first (no lock held during iteration)
        let mut to_delete: Vec<String> = Vec::new();
        for item in self.store.iter() {
            let (key, val) = item?;
            if let Ok(chunk) = MemoryChunk::decode(val.as_ref()) {
                if chunk.session_id == session_id {
                    to_delete.push(String::from_utf8_lossy(&key).to_string());
                }
            }
        }

        let count = to_delete.len() as u32;
        for id in &to_delete {
            self.delete(id).await?;
        }
        Ok(count)
    }

    pub async fn count(&self) -> Result<u64> {
        Ok(self.store.len() as u64)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        self.store.remove(id.as_bytes())?;
        let mut index = self.index.lock().await;
        index.remove(id)?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub fn bytes_to_f32(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return Err(KeelError::Serialization(
            "Embedding byte length not aligned to f32 (must be multiple of 4)".into(),
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

#[allow(dead_code)]
pub fn f32_to_bytes(floats: &[f32]) -> Vec<u8> {
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

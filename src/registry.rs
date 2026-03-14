// MemoryRegistry: FlatIndex in-memory index + sled persistent store

use crate::error::{KeelError, Result};
use crate::eviction::{EvictionPolicy, LruEvictionPolicy};
use crate::index::FlatIndex;
use crate::pb::{MemoryChunk, ScoredChunk};
use crate::signal::now_ms;
use prost::Message;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

pub struct MemoryRegistry {
    index: Arc<AsyncMutex<FlatIndex>>,
    store: sled::Db,
    eviction: Mutex<Box<dyn EvictionPolicy>>,
    max_chunks: usize,
}

impl MemoryRegistry {
    pub fn new(data_dir: &str, dim: usize, max_chunks: usize) -> Result<Self> {
        std::fs::create_dir_all(data_dir).map_err(|e| KeelError::Registry(e.to_string()))?;

        let store = sled::open(data_dir)?;
        let mut index = FlatIndex::new(dim);

        // Rebuild HNSW index and eviction state from persisted data on startup
        let eviction_policy: Box<dyn EvictionPolicy> = Box::new(LruEvictionPolicy::new());
        let eviction = Mutex::new(eviction_policy);

        for item in store.iter() {
            let (key, val) = item?;
            if let Ok(chunk) = MemoryChunk::decode(val.as_ref()) {
                let id = String::from_utf8_lossy(&key).to_string();
                if !chunk.embedding.is_empty() {
                    if let Ok(vector) = bytes_to_f32(&chunk.embedding) {
                        if vector.len() == dim {
                            let _ = index.insert(&id, &vector);
                        }
                    }
                }
                eviction.lock().unwrap().on_write(&id);
            }
        }

        Ok(Self {
            index: Arc::new(AsyncMutex::new(index)),
            store,
            eviction,
            max_chunks,
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

        // Notify eviction policy
        self.eviction.lock().unwrap().on_write(&id);

        // Enforce capacity if over limit
        if self.store.len() > self.max_chunks {
            self.enforce_capacity().await?;
        }

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
                self.eviction.lock().unwrap().on_access(id);
                Ok(Some(chunk))
            }
        }
    }

    /// Search returns results ordered by cosine similarity (1.0 = identical).
    /// `min_score` is a minimum similarity threshold in [0.0, 1.0].
    pub async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        min_score: f32,
    ) -> Result<Vec<ScoredChunk>> {
        let raw_results = {
            let index = self.index.lock().await;
            index.search(query, top_k)?
        };

        let mut scored = Vec::new();
        for (id, distance) in raw_results {
            // usearch Cosine metric returns distance in [0, 2]; convert to similarity [−1, 1]
            // clamped to [0, 1] for practical use with unit-normalized embeddings
            let similarity = (1.0 - distance).max(0.0);
            if similarity < min_score {
                continue;
            }
            if let Some(chunk) = self.read(&id).await? {
                scored.push(ScoredChunk {
                    chunk: Some(chunk),
                    score: similarity,
                });
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
        {
            let mut index = self.index.lock().await;
            index.remove(id)?;
        }
        self.eviction.lock().unwrap().on_delete(id);
        Ok(())
    }

    async fn enforce_capacity(&self) -> Result<()> {
        let chunks: Vec<MemoryChunk> = self
            .store
            .iter()
            .filter_map(|item| {
                let (_, val) = item.ok()?;
                MemoryChunk::decode(val.as_ref()).ok()
            })
            .collect();

        let ids_to_evict = {
            let eviction = self.eviction.lock().unwrap();
            eviction.select_evictions(&chunks, self.max_chunks)
        }; // lock dropped before any await

        for id in ids_to_evict {
            self.delete(&id).await?;
        }
        Ok(())
    }
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

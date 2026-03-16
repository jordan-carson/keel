// MemoryRegistry: two-tier storage
//   Hot tier  — FlatIndex (in-memory, exact cosine, bounded by hot_tier_max)
//   Cold tier — TantivyStore (persistent, BM25 + embedding term search)
//
// Startup: hot tier is rebuilt from TantivyStore by scanning up to hot_tier_max
//          chunks with embeddings, so searches are warm immediately after restart.
//
// Hot-tier bounding: insertions evict the LRU entry from FlatIndex when
//                    hot.len() == hot_tier_max, keeping memory bounded.
//                    Evicted entries remain in TantivyStore and are still found
//                    via cold-tier embedding-term search.

use crate::error::{Result, StoreError};
use crate::eviction::{EvictionPolicy, LruEvictionPolicy};
use crate::index::{cosine_similarity, FlatIndex};
use crate::store::{quantize_embedding, TantivyStore};
use keel_proto::pb::{MemoryChunk, ScoredChunk};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub struct MemoryRegistry {
    hot: Arc<AsyncMutex<FlatIndex>>,
    /// Insertion-order LRU for hot tier (front = most recent). Sync mutex — never held
    /// across an .await point.
    hot_lru: Mutex<VecDeque<String>>,
    store: Arc<TantivyStore>,
    eviction: Mutex<Box<dyn EvictionPolicy>>,
    max_chunks: usize,
    hot_tier_max: usize,
}

impl MemoryRegistry {
    pub fn new(data_dir: &str, dim: usize, hot_tier_max: usize, max_chunks: usize) -> Result<Self> {
        let store = TantivyStore::new(data_dir)?;

        // Rebuild hot tier from persisted data on startup.
        let mut hot_index = FlatIndex::new(dim);
        let mut hot_lru_deque: VecDeque<String> = VecDeque::new();
        for (id, emb_bytes) in store.scan_embeddings(hot_tier_max)? {
            if let Ok(vector) = bytes_to_f32(&emb_bytes) {
                let _ = hot_index.insert(&id, &vector);
                hot_lru_deque.push_back(id); // oldest at back; new writes push_front
            }
        }

        let eviction_policy: Box<dyn EvictionPolicy> = Box::new(LruEvictionPolicy::new());

        Ok(Self {
            hot: Arc::new(AsyncMutex::new(hot_index)),
            hot_lru: Mutex::new(hot_lru_deque),
            store: Arc::new(store),
            eviction: Mutex::new(eviction_policy),
            max_chunks,
            hot_tier_max,
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

        let embedding_terms = if !chunk.embedding.is_empty() {
            match bytes_to_f32(&chunk.embedding) {
                Ok(emb) => quantize_embedding(&emb),
                Err(_) => String::new(),
            }
        } else {
            String::new()
        };

        self.store.write(&chunk, &embedding_terms)?;

        // Insert into hot tier with LRU bounding.
        if !chunk.embedding.is_empty() {
            if let Ok(vector) = bytes_to_f32(&chunk.embedding) {
                let mut hot = self.hot.lock().await;

                // Evict LRU entry from FlatIndex if at capacity.
                let maybe_evict = {
                    let mut lru = self.hot_lru.lock().unwrap();
                    let evict = if hot.len() >= self.hot_tier_max {
                        lru.pop_back()
                    } else {
                        None
                    };
                    // Move id to front (most recent).
                    if let Some(pos) = lru.iter().position(|x| x == &id) {
                        lru.remove(pos);
                    }
                    lru.push_front(id.clone());
                    evict
                };
                if let Some(ref eid) = maybe_evict {
                    let _ = hot.remove(eid);
                }
                let _ = hot.insert(&id, &vector);
            }
        }

        self.eviction.lock().unwrap().on_write(&id);

        let total = self.store.count() as usize;
        if total > self.max_chunks {
            self.enforce_capacity(total).await?;
        }

        Ok(id)
    }

    pub async fn read(&self, id: &str) -> Result<Option<MemoryChunk>> {
        match self.store.read(id)? {
            None => Ok(None),
            Some(chunk) => {
                if chunk.ttl_ms > 0 && chunk.created_at_ms + chunk.ttl_ms < now_ms() {
                    self.delete(id).await?;
                    return Ok(None);
                }
                self.eviction.lock().unwrap().on_access(id);
                Ok(Some(chunk))
            }
        }
    }

    pub async fn search(
        &self,
        query: &[f32],
        top_k: usize,
        min_score: f32,
    ) -> Result<Vec<ScoredChunk>> {
        let mut candidates: Vec<(String, f32)> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        {
            let hot = self.hot.lock().await;
            for (id, sim) in hot.search(query, top_k)? {
                if sim >= min_score {
                    seen.insert(id.clone());
                    candidates.push((id, sim));
                }
            }
        }

        if candidates.len() < top_k {
            let terms = quantize_embedding(query);
            let cold_limit = (top_k - candidates.len()) * 3;
            for id in self.store.search_by_embedding_terms(&terms, cold_limit)? {
                if !seen.contains(&id) {
                    seen.insert(id.clone());
                    candidates.push((id, -1.0));
                }
            }
        }

        let mut scored: Vec<ScoredChunk> = Vec::new();
        for (id, pre_sim) in candidates {
            let chunk = match self.store.read(&id)? {
                Some(c) => c,
                None => continue,
            };
            if chunk.ttl_ms > 0 && chunk.created_at_ms + chunk.ttl_ms < now_ms() {
                let _ = self.store.delete(&id);
                continue;
            }
            let sim = if pre_sim >= 0.0 {
                pre_sim
            } else {
                if chunk.embedding.is_empty() {
                    continue;
                }
                match bytes_to_f32(&chunk.embedding) {
                    Ok(emb) => cosine_similarity(query, &emb),
                    Err(_) => continue,
                }
            };
            if sim >= min_score {
                scored.push(ScoredChunk {
                    chunk: Some(chunk),
                    score: sim,
                });
            }
        }

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    pub async fn evict_session(&self, session_id: &str) -> Result<u32> {
        let ids = self.store.ids_for_session(session_id)?;
        let count = ids.len() as u32;
        for id in &ids {
            self.delete(id).await?;
        }
        Ok(count)
    }

    pub async fn count(&self) -> Result<u64> {
        Ok(self.store.count())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        self.store.delete(id)?;
        {
            let mut hot = self.hot.lock().await;
            let _ = hot.remove(id);
            let mut lru = self.hot_lru.lock().unwrap();
            if let Some(pos) = lru.iter().position(|x| x == id) {
                lru.remove(pos);
            }
        }
        self.eviction.lock().unwrap().on_delete(id);
        Ok(())
    }

    async fn enforce_capacity(&self, current_count: usize) -> Result<()> {
        let ids_to_evict = {
            let eviction = self.eviction.lock().unwrap();
            eviction.select_evictions(current_count, self.max_chunks)
        };
        for id in ids_to_evict {
            self.delete(&id).await?;
        }
        Ok(())
    }
}

/// Decode raw little-endian f32 bytes into a Vec<f32>.
pub fn bytes_to_f32(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return Err(StoreError::Serialization(
            "Embedding byte length not aligned to f32 (must be multiple of 4)".into(),
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

/// Encode f32 slice to little-endian bytes.
pub fn f32_to_bytes(floats: &[f32]) -> Vec<u8> {
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

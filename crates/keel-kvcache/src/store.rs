// KvCacheStore: KV cache prefix sharing backed by memory-mapped files (Phase 3).
//
// Design
// ------
// Each (model_id, prefix_hash) pair maps to one file on disk.
// `memmap2` is used for both writing and reading:
//   - Write: the OS maps the file into virtual address space; we copy bytes into the map
//     and flush. This avoids userspace buffering for large tensors.
//   - Read:  the OS page cache serves repeated reads from the same slab without disk I/O
//     once the pages are resident — critical for shared prefixes.
//
// Index
// -----
// An in-memory `HashMap<String, KvMeta>` provides O(1) lookup. The string key is
// `"{model_id}\x00{prefix_hash}"` (null-byte separator avoids conflicts). The index
// is serialised as JSON after every write/evict so hot entries survive restarts.
//
// LRU eviction
// ------------
// A `VecDeque<String>` mirrors the HashMap keys in access order (front = most recent).
// When `max_entries` is exceeded the tail entry is evicted from both data structures
// and its file is deleted.
//
// File layout
// -----------
//   {data_dir}/
//   ├── {model_id_safe}/
//   │   └── {prefix_hash_safe}.bin   ← raw KV bytes (mmap)
//   └── kv_index.json                ← persisted metadata

use crate::error::{KvError, Result};
use keel_proto::pb::KvEntry;
use memmap2::{Mmap, MmapMut};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Composite key: `"{model_id}\x00{prefix_hash}"`.
/// The null byte makes the separator unambiguous.
fn make_key(model_id: &str, prefix_hash: &str) -> String {
    format!("{}\x00{}", model_id, prefix_hash)
}

const INDEX_FILE: &str = "kv_index.json";

/// Persisted metadata for a single cache entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KvMeta {
    prefix_hash: String,
    model_id: String,
    seq_len: i32,
    file_path: PathBuf,
    file_size: u64,
    created_at_ms: u64,
    ttl_ms: u64,
}

pub struct KvCacheStore {
    data_dir: PathBuf,
    /// Key: `make_key(model_id, prefix_hash)`.
    index: Mutex<HashMap<String, KvMeta>>,
    /// Front = most recently used, back = LRU eviction candidate.
    lru: Mutex<VecDeque<String>>,
    max_entries: usize,
}

impl KvCacheStore {
    /// Open (or create) a KV cache store at `data_dir`.
    ///
    /// On restart the persisted `kv_index.json` is loaded so all previously
    /// cached slabs are immediately available.
    pub fn new(data_dir: &str, max_entries: usize) -> Result<Self> {
        let data_dir = PathBuf::from(data_dir);
        fs::create_dir_all(&data_dir)?;

        let (index, lru) = Self::load_or_empty(&data_dir);

        Ok(Self {
            data_dir,
            index: Mutex::new(index),
            lru: Mutex::new(lru),
            max_entries,
        })
    }

    /// Write a KV slab to disk via a memory-mapped file, then update the index.
    ///
    /// Re-writing an existing entry overwrites the file and refreshes its LRU
    /// position (most-recently-used).
    pub fn write(&self, entry: &KvEntry) -> Result<()> {
        if entry.prefix_hash.is_empty() || entry.model_id.is_empty() {
            return Err(KvError::Index(
                "prefix_hash and model_id must be non-empty".into(),
            ));
        }

        let key = make_key(&entry.model_id, &entry.prefix_hash);
        let created_at_ms =
            if entry.created_at_ms == 0 { now_ms() } else { entry.created_at_ms };

        // File path: {data_dir}/{model_id_safe}/{prefix_hash_safe}.bin
        let model_dir = self.data_dir.join(safe_name(&entry.model_id));
        fs::create_dir_all(&model_dir)?;
        let file_path = model_dir.join(format!("{}.bin", safe_name(&entry.prefix_hash)));

        // Write via mmap — avoids double-buffering for large tensors.
        let len = entry.data.len();
        {
            let file = fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(true)
                .open(&file_path)?;
            if len > 0 {
                file.set_len(len as u64)?;
                let mut mmap = unsafe { MmapMut::map_mut(&file)? };
                mmap.copy_from_slice(&entry.data);
                mmap.flush()?;
            }
        }

        let meta = KvMeta {
            prefix_hash: entry.prefix_hash.clone(),
            model_id: entry.model_id.clone(),
            seq_len: entry.seq_len,
            file_path,
            file_size: len as u64,
            created_at_ms,
            ttl_ms: entry.ttl_ms,
        };

        // Update index + LRU; capture any key to evict after releasing the locks.
        let evict_key = {
            let mut index = self.index.lock().unwrap();
            let mut lru = self.lru.lock().unwrap();

            if let Some(pos) = lru.iter().position(|k| k == &key) {
                lru.remove(pos);
            }
            lru.push_front(key.clone());
            index.insert(key, meta);

            if index.len() > self.max_entries {
                lru.pop_back()
            } else {
                None
            }
        };

        if let Some(ref ek) = evict_key {
            self.delete_entry(ek)?;
        }

        self.persist_index()
    }

    /// Read a KV slab from disk via a memory-mapped file.
    ///
    /// Returns `None` if the entry is absent or has expired (lazy TTL check).
    /// On a cache hit the OS page cache serves the bytes without disk I/O.
    pub fn read(&self, prefix_hash: &str, model_id: &str) -> Result<Option<KvEntry>> {
        let key = make_key(model_id, prefix_hash);

        let meta = {
            let mut index = self.index.lock().unwrap();
            let meta = match index.get(&key) {
                Some(m) => m.clone(),
                None => return Ok(None),
            };
            // Lazy TTL eviction.
            if meta.ttl_ms > 0 && meta.created_at_ms + meta.ttl_ms < now_ms() {
                index.remove(&key);
                let mut lru = self.lru.lock().unwrap();
                if let Some(pos) = lru.iter().position(|k| k == &key) {
                    lru.remove(pos);
                }
                let _ = fs::remove_file(&meta.file_path);
                return Ok(None);
            }
            meta
        };

        // Promote to MRU position.
        {
            let mut lru = self.lru.lock().unwrap();
            if let Some(pos) = lru.iter().position(|k| k == &key) {
                lru.remove(pos);
            }
            lru.push_front(key);
        }

        // Read via mmap — OS page cache keeps hot slabs in memory.
        let data = if meta.file_size > 0 {
            let file = fs::File::open(&meta.file_path)?;
            let mmap = unsafe { Mmap::map(&file)? };
            mmap.to_vec()
        } else {
            Vec::new()
        };

        Ok(Some(KvEntry {
            prefix_hash: prefix_hash.to_string(),
            model_id: model_id.to_string(),
            seq_len: meta.seq_len,
            data,
            created_at_ms: meta.created_at_ms,
            ttl_ms: meta.ttl_ms,
        }))
    }

    /// Evict all cached entries for `model_id`.
    /// Pass an empty string to evict everything across all models.
    pub fn evict_model(&self, model_id: &str) -> Result<u32> {
        let keys: Vec<String> = {
            let index = self.index.lock().unwrap();
            index
                .iter()
                .filter(|(_, m)| model_id.is_empty() || m.model_id == model_id)
                .map(|(k, _)| k.clone())
                .collect()
        };
        let count = keys.len() as u32;
        for key in &keys {
            self.delete_entry(key)?;
        }
        if count > 0 {
            self.persist_index()?;
        }
        Ok(count)
    }

    /// Total number of entries currently in the cache.
    pub fn count(&self) -> usize {
        self.index.lock().unwrap().len()
    }

    // ---------- private helpers ----------

    fn delete_entry(&self, key: &str) -> Result<()> {
        let file_path = {
            let mut index = self.index.lock().unwrap();
            let path = index.get(key).map(|m| m.file_path.clone());
            index.remove(key);
            path
        };
        {
            let mut lru = self.lru.lock().unwrap();
            lru.retain(|k| k != key);
        }
        if let Some(path) = file_path {
            if path.exists() {
                fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    fn persist_index(&self) -> Result<()> {
        let index = self.index.lock().unwrap();
        let path = self.data_dir.join(INDEX_FILE);
        let json = serde_json::to_string(&*index)
            .map_err(|e| KvError::Index(format!("serialize index: {}", e)))?;
        fs::write(&path, json.as_bytes())?;
        Ok(())
    }

    fn load_or_empty(data_dir: &Path) -> (HashMap<String, KvMeta>, VecDeque<String>) {
        let path = data_dir.join(INDEX_FILE);
        if !path.exists() {
            return (HashMap::new(), VecDeque::new());
        }
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<HashMap<String, KvMeta>>(&bytes) {
                Ok(idx) => {
                    // Prune entries whose files no longer exist.
                    let idx: HashMap<_, _> =
                        idx.into_iter().filter(|(_, m)| m.file_path.exists()).collect();
                    let lru = idx.keys().cloned().collect();
                    (idx, lru)
                }
                Err(e) => {
                    tracing::warn!("KvCacheStore: corrupt index ({}), starting empty", e);
                    (HashMap::new(), VecDeque::new())
                }
            },
            Err(e) => {
                tracing::warn!("KvCacheStore: cannot read index ({}), starting empty", e);
                (HashMap::new(), VecDeque::new())
            }
        }
    }
}

/// Replace filesystem-unsafe characters to prevent path traversal.
fn safe_name(s: &str) -> String {
    s.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_proto::pb::KvEntry;

    fn temp_store(max: usize) -> KvCacheStore {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = format!("/tmp/keel-kvcache-{}-{}", ts, max);
        KvCacheStore::new(&dir, max).unwrap()
    }

    fn entry(hash: &str, model: &str, data: Vec<u8>) -> KvEntry {
        KvEntry {
            prefix_hash: hash.to_string(),
            model_id: model.to_string(),
            seq_len: 128,
            data,
            created_at_ms: 0,
            ttl_ms: 0,
        }
    }

    #[test]
    fn test_write_and_read_roundtrip() {
        let store = temp_store(10);
        let payload = vec![0xde, 0xad, 0xbe, 0xef];
        store.write(&entry("hash1", "llama", payload.clone())).unwrap();
        let got = store.read("hash1", "llama").unwrap().unwrap();
        assert_eq!(got.data, payload);
        assert_eq!(got.seq_len, 128);
    }

    #[test]
    fn test_read_missing_returns_none() {
        let store = temp_store(10);
        assert!(store.read("nonexistent", "model").unwrap().is_none());
    }

    #[test]
    fn test_empty_data_roundtrip() {
        let store = temp_store(10);
        store.write(&entry("empty", "model", vec![])).unwrap();
        let got = store.read("empty", "model").unwrap().unwrap();
        assert!(got.data.is_empty());
    }

    #[test]
    fn test_overwrite_updates_data() {
        let store = temp_store(10);
        store.write(&entry("h1", "m", vec![1, 2, 3])).unwrap();
        store.write(&entry("h1", "m", vec![9, 9])).unwrap();
        let got = store.read("h1", "m").unwrap().unwrap();
        assert_eq!(got.data, vec![9, 9]);
    }

    #[test]
    fn test_evict_model() {
        let store = temp_store(100);
        store.write(&entry("a", "mx", vec![1])).unwrap();
        store.write(&entry("b", "mx", vec![2])).unwrap();
        store.write(&entry("c", "my", vec![3])).unwrap();
        let n = store.evict_model("mx").unwrap();
        assert_eq!(n, 2);
        assert!(store.read("a", "mx").unwrap().is_none());
        assert!(store.read("c", "my").unwrap().is_some());
    }

    #[test]
    fn test_evict_all() {
        let store = temp_store(100);
        store.write(&entry("a", "m1", vec![1])).unwrap();
        store.write(&entry("b", "m2", vec![2])).unwrap();
        assert_eq!(store.evict_model("").unwrap(), 2);
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_lru_eviction_at_capacity() {
        let store = temp_store(3);
        for i in 0..4u8 {
            store.write(&entry(&format!("h{}", i), "m", vec![i])).unwrap();
        }
        assert_eq!(store.count(), 3);
        assert!(store.read("h0", "m").unwrap().is_none());
        assert!(store.read("h3", "m").unwrap().is_some());
    }

    #[test]
    fn test_ttl_expiry() {
        let store = temp_store(10);
        let mut e = entry("ttl", "m", vec![42]);
        e.created_at_ms = 1;
        e.ttl_ms = 1;
        store.write(&e).unwrap();
        assert!(store.read("ttl", "m").unwrap().is_none());
    }

    #[test]
    fn test_index_persists_and_reloads() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = format!("/tmp/keel-kvcache-persist-unit-{}", ts);
        {
            let s = KvCacheStore::new(&dir, 100).unwrap();
            s.write(&entry("ph", "mod", vec![7, 8, 9])).unwrap();
        }
        let s2 = KvCacheStore::new(&dir, 100).unwrap();
        let got = s2.read("ph", "mod").unwrap().unwrap();
        assert_eq!(got.data, vec![7, 8, 9]);
    }

    #[test]
    fn test_safe_name() {
        assert_eq!(safe_name("llama/3-8b"), "llama_3-8b");
        assert_eq!(safe_name("a\\b:c"), "a_b_c");
    }

    #[test]
    fn test_count() {
        let store = temp_store(100);
        assert_eq!(store.count(), 0);
        store.write(&entry("a", "m", vec![])).unwrap();
        store.write(&entry("b", "m", vec![])).unwrap();
        assert_eq!(store.count(), 2);
    }
}

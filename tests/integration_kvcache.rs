// Integration tests for Phase 3 KV cache prefix sharing.
//
// These tests exercise KvCacheStore through the public API exposed by the root
// `keel` crate, verifying that mmap write+read, LRU eviction, TTL, and
// index persistence all work end-to-end.

use keel::kvcache::KvCacheStore;
use keel::pb::KvEntry;
use uuid::Uuid;

fn make_store(max: usize) -> KvCacheStore {
    let dir = format!("/tmp/keel-kvcache-int-{}", Uuid::new_v4());
    KvCacheStore::new(&dir, max).expect("failed to create KvCacheStore")
}

fn kv(hash: &str, model: &str, data: &[u8]) -> KvEntry {
    KvEntry {
        prefix_hash: hash.to_string(),
        model_id: model.to_string(),
        seq_len: 64,
        data: data.to_vec(),
        created_at_ms: 0,
        ttl_ms: 0,
    }
}

#[test]
fn test_kv_write_and_read_roundtrip() {
    let store = make_store(100);
    let payload: Vec<u8> = (0u8..=15).collect();
    store.write(&kv("prefix-abc", "gpt-4", &payload)).unwrap();

    let result = store.read("prefix-abc", "gpt-4").unwrap();
    assert!(result.is_some());
    let got = result.unwrap();
    assert_eq!(got.data, payload);
    assert_eq!(got.seq_len, 64);
    assert_eq!(got.model_id, "gpt-4");
}

#[test]
fn test_kv_read_missing_returns_none() {
    let store = make_store(100);
    assert!(store.read("no-such-prefix", "llama").unwrap().is_none());
}

#[test]
fn test_kv_large_payload_mmap() {
    let store = make_store(10);
    // 4 MB — large enough to exercise the mmap path meaningfully.
    let big: Vec<u8> = (0u8..=255).cycle().take(4 * 1024 * 1024).collect();
    store.write(&kv("big-prefix", "llama-3-8b", &big)).unwrap();
    let got = store.read("big-prefix", "llama-3-8b").unwrap().unwrap();
    assert_eq!(got.data.len(), big.len());
    assert_eq!(got.data[0], 0);
    assert_eq!(got.data[255], 255);
    assert_eq!(got.data[256], 0);
}

#[test]
fn test_kv_overwrite_updates_data() {
    let store = make_store(100);
    store.write(&kv("h1", "m", &[1, 2, 3])).unwrap();
    store.write(&kv("h1", "m", &[9, 8, 7, 6])).unwrap();
    let got = store.read("h1", "m").unwrap().unwrap();
    assert_eq!(got.data, vec![9, 8, 7, 6]);
}

#[test]
fn test_kv_evict_model() {
    let store = make_store(100);
    store.write(&kv("a", "model-a", &[1])).unwrap();
    store.write(&kv("b", "model-a", &[2])).unwrap();
    store.write(&kv("c", "model-b", &[3])).unwrap();

    let n = store.evict_model("model-a").unwrap();
    assert_eq!(n, 2);
    assert!(store.read("a", "model-a").unwrap().is_none());
    assert!(store.read("b", "model-a").unwrap().is_none());
    // model-b entry survives.
    assert!(store.read("c", "model-b").unwrap().is_some());
}

#[test]
fn test_kv_evict_all() {
    let store = make_store(100);
    for i in 0..5u8 {
        store.write(&kv(&format!("h{}", i), "m", &[i])).unwrap();
    }
    let n = store.evict_model("").unwrap();
    assert_eq!(n, 5);
    assert_eq!(store.count(), 0);
}

#[test]
fn test_kv_lru_eviction_at_capacity() {
    let store = make_store(3);
    store.write(&kv("h0", "m", &[0])).unwrap();
    store.write(&kv("h1", "m", &[1])).unwrap();
    store.write(&kv("h2", "m", &[2])).unwrap();
    // h0 is LRU at this point; h3 triggers eviction of h0.
    store.write(&kv("h3", "m", &[3])).unwrap();

    assert_eq!(store.count(), 3);
    assert!(store.read("h0", "m").unwrap().is_none(), "h0 should be evicted");
    assert!(store.read("h3", "m").unwrap().is_some());
}

#[test]
fn test_kv_ttl_expiry() {
    let store = make_store(100);
    let mut e = kv("ttl-key", "m", &[42]);
    e.created_at_ms = 1; // epoch + 1 ms
    e.ttl_ms = 1; // expires at epoch + 2 ms (long past)
    store.write(&e).unwrap();
    assert!(store.read("ttl-key", "m").unwrap().is_none());
}

#[test]
fn test_kv_index_persists_across_reopen() {
    let dir = format!("/tmp/keel-kvcache-persist-{}", Uuid::new_v4());
    {
        let s = KvCacheStore::new(&dir, 100).unwrap();
        s.write(&kv("persisted", "model-p", &[10, 20, 30])).unwrap();
    }
    // Simulate process restart by opening a new instance over the same directory.
    let s2 = KvCacheStore::new(&dir, 100).unwrap();
    let got = s2.read("persisted", "model-p").unwrap().unwrap();
    assert_eq!(got.data, vec![10, 20, 30]);
}

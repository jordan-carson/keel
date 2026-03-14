// Integration tests for write and read operations

use keel::pb::MemoryChunk;
use keel::registry::{f32_to_bytes, MemoryRegistry};
use std::collections::HashMap;
use uuid::Uuid;

fn make_registry() -> MemoryRegistry {
    let path = format!("/tmp/keel-test-{}", Uuid::new_v4());
    MemoryRegistry::new(&path, 4, 10_000).expect("Failed to create registry")
}

fn chunk(payload: &[u8], session_id: &str) -> MemoryChunk {
    MemoryChunk {
        id: String::new(),
        embedding: vec![],
        payload: payload.to_vec(),
        session_id: session_id.to_string(),
        created_at_ms: 0,
        ttl_ms: 0,
        meta: HashMap::new(),
    }
}

#[tokio::test]
async fn test_write_and_read() {
    let registry = make_registry();

    let id = registry
        .write(chunk(b"hello keel", "session-1"))
        .await
        .expect("write failed");

    assert!(!id.is_empty());

    let result = registry.read(&id).await.expect("read failed");
    assert!(result.is_some());

    let returned = result.unwrap();
    assert_eq!(returned.payload, b"hello keel");
    assert_eq!(returned.session_id, "session-1");
}

#[tokio::test]
async fn test_write_with_embedding_and_search() {
    let registry = make_registry();

    // Write chunks with embeddings
    let vecs: &[&[f32]] = &[
        &[1.0, 0.0, 0.0, 0.0],
        &[0.0, 1.0, 0.0, 0.0],
        &[0.0, 0.0, 1.0, 0.0],
    ];
    for (i, v) in vecs.iter().enumerate() {
        let c = MemoryChunk {
            id: String::new(),
            embedding: f32_to_bytes(v),
            payload: format!("chunk-{}", i).into_bytes(),
            session_id: "session-search".to_string(),
            created_at_ms: 0,
            ttl_ms: 0,
            meta: HashMap::new(),
        };
        registry.write(c).await.expect("write failed");
    }

    // Search for nearest to [1, 0, 0, 0]
    let query = [1.0f32, 0.0, 0.0, 0.0];
    let results = registry.search(&query, 2, 1.0).await.expect("search failed");
    // usearch cosine distance 0 = identical; filter is score > min_score so min_score=1.0 passes all
    assert!(!results.is_empty());
}

#[tokio::test]
async fn test_read_missing_returns_none() {
    let registry = make_registry();
    let result = registry.read("nonexistent-id").await.expect("read failed");
    assert!(result.is_none());
}

#[tokio::test]
async fn test_ttl_expiry() {
    let registry = make_registry();

    // Write a chunk that expired 1ms ago
    let expired = MemoryChunk {
        id: String::new(),
        embedding: vec![],
        payload: b"expired".to_vec(),
        session_id: "s".to_string(),
        created_at_ms: 1, // epoch + 1ms
        ttl_ms: 1,         // ttl = 1ms, so expires at epoch + 2ms (long past)
        meta: HashMap::new(),
    };

    let id = registry.write(expired).await.expect("write failed");
    let result = registry.read(&id).await.expect("read failed");
    assert!(result.is_none(), "expired chunk should not be returned");
}

#[tokio::test]
async fn test_evict_session() {
    let registry = make_registry();

    // Write three chunks in "target" session and one in "other"
    for _ in 0..3 {
        registry.write(chunk(b"data", "target")).await.expect("write failed");
    }
    registry.write(chunk(b"data", "other")).await.expect("write failed");

    let evicted = registry.evict_session("target").await.expect("evict failed");
    assert_eq!(evicted, 3);

    let remaining = registry.count().await.expect("count failed");
    assert_eq!(remaining, 1);
}

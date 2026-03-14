// Integration tests for vector search operations

use keel::index::VectorIndex;

#[tokio::test]
async fn test_add_and_search() {
    let dim = 4;
    let index = VectorIndex::new(dim, 16, 200).expect("Failed to create index");
    
    // Add some vectors
    let vectors = vec![
        vec![1.0, 0.0, 0.0, 0.0],
        vec![0.0, 1.0, 0.0, 0.0],
        vec![0.0, 0.0, 1.0, 0.0],
        vec![0.0, 0.0, 0.0, 1.0],
    ];
    
    for (i, vec) in vectors.iter().enumerate() {
        index.add(&format!("vec_{}", i), vec).await.expect("Failed to add");
    }
    
    // Search for similar vectors
    let query = vec![0.9, 0.1, 0.0, 0.0];
    let results = index.search(&query, 2).await.expect("Failed to search");
    
    // Should find vec_0 as most similar
    assert!(!results.is_empty());
    assert_eq!(results[0].id, "vec_0");
}

#[tokio::test]
async fn test_dimension_mismatch() {
    let dim = 4;
    let index = VectorIndex::new(dim, 16, 200).expect("Failed to create index");
    
    // Try to add vector with wrong dimension
    let result = index.add("vec_1", &[1.0, 2.0, 3.0]).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_search_dimension_mismatch() {
    let dim = 4;
    let index = VectorIndex::new(dim, 16, 200).expect("Failed to create index");
    
    // Add a vector
    index.add("vec_0", &[1.0, 0.0, 0.0, 0.0]).await.expect("Failed to add");
    
    // Try to search with wrong dimension
    let result = index.search(&[1.0, 0.0, 0.0], 2).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_search_k_greater_than_size() {
    let dim = 4;
    let index = VectorIndex::new(dim, 16, 200).expect("Failed to create index");
    
    // Add only one vector
    index.add("vec_0", &[1.0, 0.0, 0.0, 0.0]).await.expect("Failed to add");
    
    // Search for more results than available
    let results = index.search(&[1.0, 0.0, 0.0, 0.0], 10).await.expect("Failed to search");
    
    // Should return at most 1 result
    assert!(results.len() <= 1);
}

#[tokio::test]
async fn test_index_size() {
    let dim = 4;
    let index = VectorIndex::new(dim, 16, 200).expect("Failed to create index");
    
    // Check empty size
    let size = index.size().await.expect("Failed to get size");
    assert_eq!(size, 0);
    
    // Add vectors
    index.add("vec_0", &[1.0, 0.0, 0.0, 0.0]).await.expect("Failed to add");
    index.add("vec_1", &[0.0, 1.0, 0.0, 0.0]).await.expect("Failed to add");
    
    // Check size
    let size = index.size().await.expect("Failed to get size");
    assert_eq!(size, 2);
}

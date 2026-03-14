// Integration tests for write and read operations

use keel::registry::MemoryRegistry;

#[tokio::test]
async fn test_write_and_read() {
    let registry = MemoryRegistry::new();
    
    // Write a memory entry
    let namespace = "test".to_string();
    let key = "test_key".to_string();
    let value = b"test_value".to_vec();
    
    let id = registry.write(namespace.clone(), key.clone(), value.clone(), None)
        .await
        .expect("Failed to write");
    
    // Read it back
    let entry = registry.read(&id)
        .await
        .expect("Failed to read")
        .expect("Entry not found");
    
    assert_eq!(entry.key, key);
    assert_eq!(entry.value, value);
}

#[tokio::test]
async fn test_list_entries() {
    let registry = MemoryRegistry::new();
    
    // Write multiple entries in the same namespace
    for i in 0..5 {
        registry.write(
            "test".to_string(),
            format!("key_{}", i),
            format!("value_{}", i).into_bytes(),
            None,
        ).await.expect("Failed to write");
    }
    
    // List entries
    let entries = registry.list("test").await.expect("Failed to list");
    assert_eq!(entries.len(), 5);
}

#[tokio::test]
async fn test_delete_entry() {
    let registry = MemoryRegistry::new();
    
    // Write an entry
    let id = registry.write(
        "test".to_string(),
        "key".to_string(),
        b"value".to_vec(),
        None,
    ).await.expect("Failed to write");
    
    // Delete it
    let deleted = registry.delete(&id).await.expect("Failed to delete");
    assert!(deleted);
    
    // Verify it's gone
    let entry = registry.read(&id).await.expect("Failed to read");
    assert!(entry.is_none());
}

#[tokio::test]
async fn test_access_count() {
    let registry = MemoryRegistry::new();
    
    // Write an entry
    let id = registry.write(
        "test".to_string(),
        "key".to_string(),
        b"value".to_vec(),
        None,
    ).await.expect("Failed to write");
    
    // Read it multiple times
    for _ in 0..3 {
        registry.read(&id).await.expect("Failed to read");
    }
    
    // Check access count
    let entry = registry.read(&id).await.expect("Failed to read")
        .expect("Entry not found");
    
    assert!(entry.access_count > 0);
}

// HNSW vector index wrapper around usearch

use crate::error::{KeelError, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use usearch::{Index, IndexOptions, MetricKind};

#[allow(dead_code)]
/// Result from a vector similarity search
pub struct SearchResult {
    pub id: String,
    pub score: f32,
}

#[allow(dead_code)]
/// Async wrapper around HnswIndex, safe for shared use across tasks
pub struct VectorIndex {
    inner: Arc<Mutex<HnswIndex>>,
}

#[allow(dead_code)]
impl VectorIndex {
    pub fn new(dimensions: usize, max_connections: usize, ef_construction: usize) -> Result<Self> {
        let inner = HnswIndex::new(dimensions, ef_construction, max_connections);
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    pub async fn add(&self, id: &str, vector: &[f32]) -> Result<()> {
        self.inner.lock().await.insert(id, vector)
    }

    pub async fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<SearchResult>> {
        let results = self.inner.lock().await.search(query, top_k)?;
        Ok(results
            .into_iter()
            .map(|(id, score)| SearchResult { id, score })
            .collect())
    }

    pub async fn size(&self) -> Result<usize> {
        Ok(self.inner.lock().await.len())
    }
}

/// HNSW vector index for semantic search, wrapping usearch
pub struct HnswIndex {
    /// The underlying usearch index
    index: Index,
    /// Vector dimensions
    dimensions: usize,
    /// Map from string IDs to usearch u64 keys
    id_map: HashMap<String, u64>,
    /// Reverse map from usearch u64 keys back to string IDs (O(1) lookup in search)
    key_to_id: HashMap<u64, String>,
    /// Counter for generating unique keys
    key_counter: u64,
}

impl HnswIndex {
    /// Create a new HNSW index with the given parameters
    ///
    /// # Arguments
    /// * `dimensions` - The dimensionality of the vectors
    /// * `ef_construction` - The construction-time search depth (default 200)
    /// * `max_connections` - Maximum connections per node in the graph (default 16)
    ///
    /// # Returns
    /// A new HnswIndex instance
    pub fn new(dimensions: usize, ef_construction: usize, max_connections: usize) -> Self {
        let mut opts = IndexOptions::default();
        opts.dimensions = dimensions;
        opts.metric = MetricKind::Cos;
        // Use expansion_add for ef_construction-like behavior
        opts.expansion_add = ef_construction;
        // Use connectivity for max_connections
        opts.connectivity = max_connections;

        let index = Index::new(&opts).expect("Failed to create usearch index");
        index.reserve(1024).expect("Failed to reserve usearch capacity");

        Self {
            index,
            dimensions,
            id_map: HashMap::new(),
            key_to_id: HashMap::new(),
            key_counter: 0,
        }
    }

    /// Insert a vector into the index with the given ID
    ///
    /// # Arguments
    /// * `id` - Unique string identifier for the vector
    /// * `vector` - The vector data as a slice of f32 values
    ///
    /// # Errors
    /// Returns an error if the vector dimensions don't match
    pub fn insert(&mut self, id: &str, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dimensions {
            return Err(KeelError::Index(format!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimensions,
                vector.len()
            )));
        }

        // Grow capacity if needed
        if self.index.size() >= self.index.capacity() {
            let new_cap = (self.index.capacity() * 2).max(1024);
            self.index
                .reserve(new_cap)
                .map_err(|e| KeelError::Index(e.to_string()))?;
        }

        // Generate a unique u64 key for this ID
        let key = self.key_counter;
        self.key_counter += 1;

        // Add to usearch index
        self.index
            .add(key, vector)
            .map_err(|e| KeelError::Index(e.to_string()))?;

        // Store both directions of the mapping
        self.id_map.insert(id.to_string(), key);
        self.key_to_id.insert(key, id.to_string());

        Ok(())
    }

    /// Search for the top-k nearest neighbors
    ///
    /// # Arguments
    /// * `query` - The query vector
    /// * `top_k` - Number of results to return
    ///
    /// # Returns
    /// A vector of (id, score) tuples, sorted by score descending
    ///
    /// # Errors
    /// Returns an error if the query vector dimensions don't match
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(String, f32)>> {
        if query.len() != self.dimensions {
            return Err(KeelError::Index(format!(
                "Query vector dimension mismatch: expected {}, got {}",
                self.dimensions,
                query.len()
            )));
        }

        let results = self
            .index
            .search(query, top_k)
            .map_err(|e| KeelError::Index(e.to_string()))?;

        // Convert u64 keys back to string IDs via reverse map — O(1) per result
        let keys_len = results.keys.len();
        let mut output = Vec::with_capacity(keys_len);

        for i in 0..keys_len {
            let key = results.keys[i];
            let distance = results.distances[i];
            if let Some(id) = self.key_to_id.get(&key) {
                output.push((id.clone(), distance));
            }
        }

        Ok(output)
    }

    /// Remove a vector from the index by its ID
    ///
    /// # Arguments
    /// * `id` - The ID of the vector to remove
    ///
    /// # Errors
    /// Returns an error if the removal fails
    pub fn remove(&mut self, id: &str) -> Result<()> {
        if let Some(key) = self.id_map.remove(id) {
            self.key_to_id.remove(&key);
            self.index
                .remove(key)
                .map_err(|e| KeelError::Index(e.to_string()))?;
        }
        Ok(())
    }

    /// Check if the index contains a vector with the given ID
    ///
    /// # Arguments
    /// * `id` - The ID to check
    ///
    /// # Returns
    /// True if the ID exists in the index
    #[allow(dead_code)]
    pub fn contains(&self, id: &str) -> bool {
        self.id_map.contains_key(id)
    }

    /// Get the number of vectors in the index
    ///
    /// # Returns
    /// The number of vectors stored
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.index.size()
    }

    /// Check if the index is empty
    ///
    /// # Returns
    /// True if the index contains no vectors
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.index.size() == 0
    }

    /// Get the dimensions of vectors in this index
    ///
    /// # Returns
    /// The vector dimensionality
    #[allow(dead_code)]
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hnsw_index_create() {
        let index = HnswIndex::new(128, 200, 16);
        assert_eq!(index.dimensions(), 128);
        assert!(index.is_empty());
    }

    #[test]
    fn test_hnsw_index_insert_and_search() {
        let mut index = HnswIndex::new(4, 200, 16);

        // Insert some vectors
        index.insert("vec1", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.insert("vec2", &[0.0, 1.0, 0.0, 0.0]).unwrap();
        index.insert("vec3", &[0.0, 0.0, 1.0, 0.0]).unwrap();

        assert_eq!(index.len(), 3);

        // Search for nearest to [1, 0, 0, 0]
        let results = index.search(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();

        assert!(!results.is_empty());
        // First result should be vec1 (identical)
        assert_eq!(results[0].0, "vec1");
    }

    #[test]
    fn test_hnsw_index_contains() {
        let mut index = HnswIndex::new(4, 200, 16);

        assert!(!index.contains("vec1"));

        index.insert("vec1", &[1.0, 0.0, 0.0, 0.0]).unwrap();

        assert!(index.contains("vec1"));
        assert!(!index.contains("vec2"));
    }

    #[test]
    fn test_hnsw_index_remove() {
        let mut index = HnswIndex::new(4, 200, 16);

        index.insert("vec1", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        assert!(index.contains("vec1"));

        index.remove("vec1").unwrap();
        assert!(!index.contains("vec1"));
    }

    #[test]
    fn test_hnsw_index_dimension_mismatch() {
        let mut index = HnswIndex::new(4, 200, 16);

        // Try to insert wrong dimension
        let result = index.insert("vec1", &[1.0, 2.0, 3.0]);
        assert!(result.is_err());

        // Try to search with wrong dimension
        let result = index.search(&[1.0, 2.0, 3.0], 1);
        assert!(result.is_err());
    }
}

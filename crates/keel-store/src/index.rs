// Pure-Rust flat vector index for semantic search

use crate::error::{Result, StoreError};
use std::collections::HashMap;

#[allow(dead_code)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
}

/// Async wrapper around FlatIndex, safe for shared use across tasks
pub struct VectorIndex {
    inner: std::sync::Arc<tokio::sync::Mutex<FlatIndex>>,
}

#[allow(dead_code)]
impl VectorIndex {
    pub fn new(dimensions: usize) -> Result<Self> {
        let inner = FlatIndex::new(dimensions);
        Ok(Self {
            inner: std::sync::Arc::new(tokio::sync::Mutex::new(inner)),
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

pub struct FlatIndex {
    dimensions: usize,
    vectors: HashMap<String, Vec<f32>>,
}

impl FlatIndex {
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions,
            vectors: HashMap::new(),
        }
    }

    pub fn insert(&mut self, id: &str, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dimensions {
            return Err(StoreError::Index(format!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimensions,
                vector.len()
            )));
        }
        let normalized = normalize(vector);
        self.vectors.insert(id.to_string(), normalized);
        Ok(())
    }

    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(String, f32)>> {
        if query.len() != self.dimensions {
            return Err(StoreError::Index(format!(
                "Query vector dimension mismatch: expected {}, got {}",
                self.dimensions,
                query.len()
            )));
        }
        let normalized_query = normalize(query);
        let mut scores: Vec<(String, f32)> = self
            .vectors
            .iter()
            .map(|(id, vec)| {
                let similarity = dot_product(&normalized_query, vec);
                (id.clone(), similarity)
            })
            .collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(top_k);
        Ok(scores)
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.vectors.remove(id);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn contains(&self, id: &str) -> bool {
        self.vectors.contains_key(id)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    #[allow(dead_code)]
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }
}

/// Compute cosine similarity between two vectors (without pre-normalization).
/// Returns a value in [-1, 1] where 1.0 = identical direction.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let na = normalize(a);
    let nb = normalize(b);
    dot_product(&na, &nb)
}

fn normalize(vector: &[f32]) -> Vec<f32> {
    let magnitude = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if magnitude == 0.0 {
        return vec![0.0; vector.len()];
    }
    vector.iter().map(|x| x / magnitude).collect()
}

fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flat_index_create() {
        let index = FlatIndex::new(128);
        assert_eq!(index.dimensions(), 128);
        assert!(index.is_empty());
    }

    #[test]
    fn test_flat_index_insert_and_search() {
        let mut index = FlatIndex::new(4);
        index.insert("vec1", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.insert("vec2", &[0.0, 1.0, 0.0, 0.0]).unwrap();
        index.insert("vec3", &[0.0, 0.0, 1.0, 0.0]).unwrap();
        assert_eq!(index.len(), 3);
        let results = index.search(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "vec1");
        assert!((results[0].1 - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_flat_index_contains() {
        let mut index = FlatIndex::new(4);
        assert!(!index.contains("vec1"));
        index.insert("vec1", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        assert!(index.contains("vec1"));
        assert!(!index.contains("vec2"));
    }

    #[test]
    fn test_flat_index_remove() {
        let mut index = FlatIndex::new(4);
        index.insert("vec1", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        assert!(index.contains("vec1"));
        index.remove("vec1").unwrap();
        assert!(!index.contains("vec1"));
    }

    #[test]
    fn test_flat_index_dimension_mismatch() {
        let mut index = FlatIndex::new(4);
        let result = index.insert("vec1", &[1.0, 2.0, 3.0]);
        assert!(result.is_err());
        let result = index.search(&[1.0, 2.0, 3.0], 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_normalize() {
        let v = &[3.0, 4.0];
        let normalized = normalize(v);
        assert!((normalized[0] - 0.6).abs() < 0.001);
        assert!((normalized[1] - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_dot_product() {
        let a = &[1.0, 0.0];
        let b = &[1.0, 0.0];
        assert!((dot_product(a, b) - 1.0).abs() < 0.001);
        let a = &[1.0, 0.0];
        let b = &[0.0, 1.0];
        assert!((dot_product(a, b) - 0.0).abs() < 0.001);
    }
}

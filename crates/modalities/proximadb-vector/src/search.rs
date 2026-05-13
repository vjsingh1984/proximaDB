//! # Vector Search Module
//!
//! Vector similarity search and Approximate Nearest Neighbor (ANN) algorithms.

use serde::{Deserialize, Serialize};

use super::distance::{DistanceMetric, DistanceMode, SimilarityResult};
use super::index::{Neighbor, VectorIndex};

/// Vector search engine
pub struct VectorSearchEngine {
    index: Box<dyn VectorIndex>,
    config: SearchConfig,
}

impl VectorSearchEngine {
    /// Create a new vector search engine
    pub fn new(index: Box<dyn VectorIndex>, config: SearchConfig) -> Self {
        Self { index, config }
    }

    /// Create a new search engine with default configuration
    pub fn with_index(index: Box<dyn VectorIndex>) -> Self {
        Self {
            index,
            config: SearchConfig::default(),
        }
    }

    /// Search for k nearest neighbors
    pub fn search(&self, vector: &[f32], k: usize) -> Result<Vec<Neighbor>, SearchError> {
        if k > self.config.max_k {
            return Err(SearchError::InvalidK(k));
        }
        self.index
            .search(vector, k.min(self.config.max_k))
            .map_err(|e| SearchError::IndexError(e.to_string()))
    }

    /// Search for k nearest neighbors and return similarity scores
    pub fn search_similar(
        &self,
        vector: &[f32],
        k: usize,
        mode: DistanceMode,
    ) -> Result<Vec<SimilarityResult>, SearchError> {
        let neighbors = self.search(vector, k)?;
        let metric = self.config.metric;
        Ok(neighbors
            .into_iter()
            .map(|n| {
                let (raw_distance, rank_value) = match mode {
                    DistanceMode::Distance => (n.distance, n.distance),
                    DistanceMode::Similarity => {
                        // Convert distance to similarity (higher = more similar)
                        let similarity = 1.0 / (1.0 + n.distance);
                        (similarity, n.distance)
                    }
                };
                SimilarityResult {
                    raw_distance,
                    rank_value,
                    metric,
                }
            })
            .collect())
    }

    /// Search within a radius
    pub fn search_radius(&self, vector: &[f32], radius: f32) -> Result<Vec<Neighbor>, SearchError> {
        self.index
            .search_radius(vector, radius)
            .map_err(|e| SearchError::IndexError(e.to_string()))
    }

    /// Batch search for multiple query vectors
    pub fn batch_search(
        &self,
        vectors: &[Vec<f32>],
        k: usize,
    ) -> Result<Vec<Vec<Neighbor>>, SearchError> {
        vectors.iter().map(|v| self.search(v, k)).collect()
    }

    /// Get the underlying index reference
    pub fn index(&self) -> &dyn VectorIndex {
        self.index.as_ref()
    }

    /// Get mutable reference to the underlying index
    pub fn index_mut(&mut self) -> &mut dyn VectorIndex {
        self.index.as_mut()
    }

    /// Get search engine statistics
    pub fn stats(&self) -> SearchStats {
        let index_stats = self.index.stats();
        SearchStats {
            total_vectors: index_stats.count,
            dimension: index_stats.dimension,
            index_type: index_stats.index_type,
            memory_bytes: index_stats.memory_bytes,
        }
    }
}

/// Search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Maximum number of results to return
    pub max_k: usize,
    /// Search timeout in milliseconds
    pub timeout_ms: Option<u64>,
    /// Whether to use SIMD acceleration
    pub use_simd: bool,
    /// Distance metric to use for similarity conversion
    pub metric: DistanceMetric,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_k: 100,
            timeout_ms: None,
            use_simd: true,
            metric: DistanceMetric::Euclidean,
        }
    }
}

/// Search error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchError {
    InvalidK(usize),
    IndexError(String),
    Timeout,
    InvalidDimension,
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::InvalidK(k) => write!(f, "Invalid k value: {}", k),
            SearchError::IndexError(msg) => write!(f, "Index error: {}", msg),
            SearchError::Timeout => write!(f, "Search timeout"),
            SearchError::InvalidDimension => write!(f, "Invalid dimension"),
        }
    }
}

impl std::error::Error for SearchError {}

/// Search statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchStats {
    /// Total number of vectors in the index
    pub total_vectors: usize,
    /// Vector dimension
    pub dimension: usize,
    /// Index type
    pub index_type: super::index::IndexType,
    /// Memory usage in bytes
    pub memory_bytes: usize,
}

/// Vector search parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchParams {
    /// Number of nearest neighbors to return
    pub k: usize,
    /// Whether to return the actual vectors
    pub include_vectors: bool,
    /// Filter expression
    pub filter: Option<String>,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            k: 10,
            include_vectors: false,
            filter: None,
        }
    }
}

/// Brute-force search for small datasets
#[derive(Debug, Clone)]
pub struct BruteForceSearch {
    vectors: Vec<(u64, Vec<f32>)>,
    metric: DistanceMetric,
}

impl BruteForceSearch {
    /// Create a new brute-force search
    pub fn new(metric: DistanceMetric) -> Self {
        Self {
            vectors: Vec::new(),
            metric,
        }
    }

    /// Add a vector
    pub fn add_vector(&mut self, id: u64, vector: Vec<f32>) {
        self.vectors.push((id, vector));
    }

    /// Search for k nearest neighbors
    pub fn search(&self, query: &[f32], k: usize) -> Vec<Neighbor> {
        let mut results = Vec::new();

        for &(id, ref vector) in &self.vectors {
            if vector.len() != query.len() {
                continue;
            }

            let distance = match self.metric {
                DistanceMetric::Euclidean => query
                    .iter()
                    .zip(vector.iter())
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt(),
                DistanceMetric::Cosine => {
                    let dot: f32 = query.iter().zip(vector.iter()).map(|(a, b)| a * b).sum();
                    let norm_q: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
                    let norm_v: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
                    1.0 - dot / (norm_q * norm_v)
                }
                DistanceMetric::DotProduct => -query
                    .iter()
                    .zip(vector.iter())
                    .map(|(a, b)| a * b)
                    .sum::<f32>(),
                DistanceMetric::Manhattan => query
                    .iter()
                    .zip(vector.iter())
                    .map(|(a, b)| (a - b).abs())
                    .sum(),
                // Fallback to Euclidean for unimplemented metrics
                _ => query
                    .iter()
                    .zip(vector.iter())
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt(),
            };

            results.push(Neighbor { id, distance });
        }

        results.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
        results.truncate(k);
        results
    }

    /// Get the number of vectors
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::FlatIndex;

    #[test]
    fn test_search_engine() {
        let mut index = FlatIndex::new(3, DistanceMetric::Euclidean);
        index.add_vector(1, vec![1.0, 2.0, 3.0]).unwrap();
        index.add_vector(2, vec![4.0, 5.0, 6.0]).unwrap();

        let engine = VectorSearchEngine::new(Box::new(index), SearchConfig::default());
        let results = engine.search(&[1.0, 2.0, 3.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, 1);
        assert_eq!(results[1].id, 2);
    }

    #[test]
    fn test_search_similar() {
        let mut search = BruteForceSearch::new(DistanceMetric::Euclidean);
        search.add_vector(1, vec![1.0, 2.0, 3.0]);
        search.add_vector(2, vec![4.0, 5.0, 6.0]);
        search.add_vector(3, vec![2.0, 3.0, 4.0]);

        let results = search.search(&[1.0, 2.0, 3.0], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, 1); // Closest
        assert_eq!(results[1].id, 3);
    }

    #[test]
    fn test_search_params() {
        let params = SearchParams::default();
        assert_eq!(params.k, 10);
        assert!(!params.include_vectors);
    }
}

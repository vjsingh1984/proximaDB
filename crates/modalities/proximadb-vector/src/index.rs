//! # Vector Index Module
//!
//! Vector indexing algorithms for Approximate Nearest Neighbor (ANN) search.
//!
//! ## Supported Index Types
//!
//! - **HNSW** - Hierarchical Navigable Small World graph
//! - **IVF** - Inverted File Index
//! - **PQ** - Product Quantization
//! - **Annoy** - Approximate Nearest Neighbors Oh Yeah
//! - **LSH** - Locality-Sensitive Hashing

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::distance::DistanceMetric;

/// Vector index configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfig {
    /// Index type
    pub index_type: IndexType,
    /// Distance metric to use
    pub metric: DistanceMetric,
    /// Index parameters
    pub parameters: IndexParameters,
}

impl IndexConfig {
    /// Create a new index configuration
    pub fn new(index_type: IndexType, metric: DistanceMetric) -> Self {
        Self {
            index_type,
            metric,
            parameters: IndexParameters::default_for_type(index_type),
        }
    }

    /// Create HNSW index configuration
    pub fn hnsw(metric: DistanceMetric, m: usize, ef_construction: usize) -> Self {
        Self {
            index_type: IndexType::HNSW,
            metric,
            parameters: IndexParameters::HNSW {
                m,
                ef_construction,
                ef_search: ef_construction,
            },
        }
    }

    /// Create IVF index configuration
    pub fn ivf(metric: DistanceMetric, nlist: usize) -> Self {
        Self {
            index_type: IndexType::IVF,
            metric,
            parameters: IndexParameters::IVF { nlist, nprobe: 1 },
        }
    }

    /// Create PQ index configuration
    pub fn pq(metric: DistanceMetric, nsubvector: usize, nbits: u8) -> Self {
        Self {
            index_type: IndexType::PQ,
            metric,
            parameters: IndexParameters::PQ { nsubvector, nbits },
        }
    }
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self::new(IndexType::HNSW, DistanceMetric::Euclidean)
    }
}

/// Index type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexType {
    /// Hierarchical Navigable Small World
    HNSW,
    /// Inverted File
    IVF,
    /// Product Quantization
    PQ,
    /// Approximate Nearest Neighbors Oh Yeah
    Annoy,
    /// Locality-Sensitive Hashing
    LSH,
}

/// Index parameters for different index types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IndexParameters {
    HNSW {
        /// Number of bidirectional links for each node
        m: usize,
        /// Size of dynamic candidate list for construction
        ef_construction: usize,
        /// Size of dynamic candidate list for search
        ef_search: usize,
    },
    IVF {
        /// Number of inverted lists (centroids)
        nlist: usize,
        /// Number of lists to probe
        nprobe: usize,
    },
    PQ {
        /// Number of subvectors
        nsubvector: usize,
        /// Number of bits per subvector
        nbits: u8,
    },
    Annoy {
        /// Number of trees
        n_trees: usize,
    },
    LSH {
        /// Number of hash tables
        n_tables: usize,
        /// Number of hash functions per table
        n_functions: usize,
    },
}

impl IndexParameters {
    fn default_for_type(index_type: IndexType) -> Self {
        match index_type {
            IndexType::HNSW => IndexParameters::HNSW {
                m: 16,
                ef_construction: 200,
                ef_search: 50,
            },
            IndexType::IVF => IndexParameters::IVF {
                nlist: 100,
                nprobe: 10,
            },
            IndexType::PQ => IndexParameters::PQ {
                nsubvector: 8,
                nbits: 8,
            },
            IndexType::Annoy => IndexParameters::Annoy { n_trees: 10 },
            IndexType::LSH => IndexParameters::LSH {
                n_tables: 10,
                n_functions: 5,
            },
        }
    }
}

/// Vector index trait
pub trait VectorIndex: Send + Sync {
    /// Add a vector to the index
    fn add_vector(&mut self, id: u64, vector: Vec<f32>) -> Result<(), IndexError>;

    /// Add multiple vectors to the index
    fn add_vectors(&mut self, vectors: Vec<(u64, Vec<f32>)>) -> Result<(), IndexError> {
        for (id, vec) in vectors {
            self.add_vector(id, vec)?;
        }
        Ok(())
    }

    /// Remove a vector from the index
    fn remove_vector(&mut self, id: u64) -> Result<(), IndexError>;

    /// Search for k nearest neighbors
    fn search(&self, vector: &[f32], k: usize) -> Result<Vec<Neighbor>, IndexError>;

    /// Search within a radius
    fn search_radius(&self, vector: &[f32], radius: f32) -> Result<Vec<Neighbor>, IndexError>;

    /// Get the number of vectors in the index
    fn len(&self) -> usize;

    /// Check if the index is empty
    fn is_empty(&self) -> bool;

    /// Get index statistics
    fn stats(&self) -> IndexStats;
}

/// Index error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexError {
    VectorNotFound,
    InvalidDimension,
    IndexFull,
    SearchError(String),
    BuildError(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::VectorNotFound => write!(f, "Vector not found"),
            IndexError::InvalidDimension => write!(f, "Invalid vector dimension"),
            IndexError::IndexFull => write!(f, "Index is full"),
            IndexError::SearchError(msg) => write!(f, "Search error: {}", msg),
            IndexError::BuildError(msg) => write!(f, "Build error: {}", msg),
        }
    }
}

impl std::error::Error for IndexError {}

/// Neighbor result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbor {
    /// Vector ID
    pub id: u64,
    /// Distance to query vector
    pub distance: f32,
}

impl PartialOrd for Neighbor {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Neighbor {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.distance
            .partial_cmp(&other.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl PartialEq for Neighbor {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.distance == other.distance
    }
}

impl Eq for Neighbor {}

/// Index statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    /// Total number of vectors
    pub count: usize,
    /// Vector dimension
    pub dimension: usize,
    /// Index type
    pub index_type: IndexType,
    /// Memory usage in bytes
    pub memory_bytes: usize,
    /// Index-specific metrics
    pub metrics: HashMap<String, String>,
}

/// Simple in-memory vector index (flat search)
#[derive(Debug, Clone)]
pub struct FlatIndex {
    vectors: HashMap<u64, Vec<f32>>,
    dimension: usize,
    metric: DistanceMetric,
}

impl FlatIndex {
    /// Create a new flat index
    pub fn new(dimension: usize, metric: DistanceMetric) -> Self {
        Self {
            vectors: HashMap::new(),
            dimension,
            metric,
        }
    }

    /// Get the metric
    pub fn metric(&self) -> DistanceMetric {
        self.metric
    }
}

impl VectorIndex for FlatIndex {
    fn add_vector(&mut self, id: u64, vector: Vec<f32>) -> Result<(), IndexError> {
        if vector.len() != self.dimension {
            return Err(IndexError::InvalidDimension);
        }
        self.vectors.insert(id, vector);
        Ok(())
    }

    fn remove_vector(&mut self, id: u64) -> Result<(), IndexError> {
        self.vectors.remove(&id).ok_or(IndexError::VectorNotFound)?;
        Ok(())
    }

    fn search(&self, vector: &[f32], k: usize) -> Result<Vec<Neighbor>, IndexError> {
        if vector.len() != self.dimension {
            return Err(IndexError::InvalidDimension);
        }

        let mut neighbors = Vec::new();
        for (&id, other) in &self.vectors {
            let distance = match self.metric {
                DistanceMetric::Euclidean => euclidean_distance_flat(vector, other),
                DistanceMetric::Cosine => cosine_distance_flat(vector, other),
                DistanceMetric::DotProduct => dot_product_flat(vector, other),
                DistanceMetric::Manhattan => manhattan_distance_flat(vector, other),
                // Fallback to Euclidean for unimplemented metrics
                _ => euclidean_distance_flat(vector, other),
            };
            neighbors.push(Neighbor { id, distance });
        }

        neighbors.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
        neighbors.truncate(k);
        Ok(neighbors)
    }

    fn search_radius(&self, vector: &[f32], radius: f32) -> Result<Vec<Neighbor>, IndexError> {
        if vector.len() != self.dimension {
            return Err(IndexError::InvalidDimension);
        }

        let mut neighbors = Vec::new();
        for (&id, other) in &self.vectors {
            let distance = match self.metric {
                DistanceMetric::Euclidean => euclidean_distance_flat(vector, other),
                DistanceMetric::Cosine => cosine_distance_flat(vector, other),
                DistanceMetric::DotProduct => dot_product_flat(vector, other),
                DistanceMetric::Manhattan => manhattan_distance_flat(vector, other),
                // Fallback to Euclidean for unimplemented metrics
                _ => euclidean_distance_flat(vector, other),
            };
            if distance <= radius {
                neighbors.push(Neighbor { id, distance });
            }
        }
        Ok(neighbors)
    }

    fn len(&self) -> usize {
        self.vectors.len()
    }

    fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    fn stats(&self) -> IndexStats {
        let mut metrics = HashMap::new();
        metrics.insert("type".to_string(), "flat".to_string());
        metrics.insert("metric".to_string(), format!("{:?}", self.metric));

        IndexStats {
            count: self.vectors.len(),
            dimension: self.dimension,
            index_type: IndexType::HNSW, // Using HNSW as placeholder
            memory_bytes: self.vectors.len() * self.dimension * 4,
            metrics,
        }
    }
}

/// Helper function for Euclidean distance
fn euclidean_distance_flat(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// Helper function for cosine distance
fn cosine_distance_flat(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    1.0 - dot / (norm_a * norm_b)
}

/// Helper function for dot product
fn dot_product_flat(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Helper function for Manhattan distance
fn manhattan_distance_flat(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_config() {
        let config = IndexConfig::hnsw(DistanceMetric::Euclidean, 16, 200);
        assert_eq!(config.index_type, IndexType::HNSW);
    }

    #[test]
    fn test_flat_index() {
        let mut index = FlatIndex::new(3, DistanceMetric::Euclidean);

        // Add vectors
        index.add_vector(1, vec![1.0, 2.0, 3.0]).unwrap();
        index.add_vector(2, vec![4.0, 5.0, 6.0]).unwrap();
        index.add_vector(3, vec![2.0, 3.0, 4.0]).unwrap();

        // Search
        let results = index.search(&[1.0, 2.0, 3.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, 1); // Closest to itself
        assert_eq!(results[1].id, 3);
    }

    #[test]
    fn test_index_stats() {
        let index = FlatIndex::new(3, DistanceMetric::Euclidean);
        let stats = index.stats();
        assert_eq!(stats.count, 0);
        assert_eq!(stats.dimension, 3);
    }
}

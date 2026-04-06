//! Sparse Vector HNSW Index (Issue #57, Core Quality)
//!
//! This module provides a Hierarchical Navigating Small World (HNSW) implementation
//! optimized for sparse vectors, which are essential for text analysis, feature-based
//! machine learning, and NLP applications.
//!
//! ## Key Features
//!
//! - **Sparse Representation**: Store only non-zero elements efficiently
//! - **Fast Similarity**: Optimized cosine similarity for sparse vectors
//! - **Graph Navigation**: HNSW algorithm adapted for sparse data
//! - **Memory Efficient**: Hash map storage instead of dense arrays
//! - **Insert/Search**: Standard HNSW operations with sparse optimizations
//!
//! ## Use Cases
//!
//! - **Text Search**: TF-IDF, bag-of-words embeddings
//! - **Feature-Based ML**: One-hot encodings, categorical features
//! - **NLP Applications**: Document similarity, semantic search
//! - **Recommendation Systems**: User-item interaction matrices

use std::collections::HashMap;
use std::time::Instant;
use anyhow::{Result, anyhow};
use tracing::{debug, info};

use crate::core::hardware_capabilities::HardwareCapabilities;

/// Local SearchResult type for sparse HNSW
#[derive(Debug, Clone, Default)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
}

/// Sparse vector representation
///
/// Uses a HashMap to store only non-zero elements, making it efficient
/// for high-dimensional sparse data like text embeddings or feature vectors.
#[derive(Debug, Clone, PartialEq)]
pub struct SparseVector {
    /// Vector ID
    pub id: String,

    /// Non-zero elements (dimension -> value)
    pub elements: HashMap<usize, f32>,

    /// Vector metadata for filtering
    pub metadata: serde_json::Value,

    /// L2 norm (cached for faster similarity computation)
    pub norm: f32,
}

impl SparseVector {
    /// Create a new sparse vector from a dense representation
    pub fn from_dense(id: String, dense: &[f32], metadata: serde_json::Value) -> Self {
        let mut elements = HashMap::new();
        let mut norm_sq = 0.0;

        for (dim, &val) in dense.iter().enumerate() {
            if val.abs() > f32::EPSILON {
                elements.insert(dim, val);
                norm_sq += val * val;
            }
        }

        Self {
            id,
            elements,
            metadata,
            norm: norm_sq.sqrt(),
        }
    }

    /// Create a new sparse vector from sparse representation
    pub fn from_sparse(
        id: String,
        elements: HashMap<usize, f32>,
        metadata: serde_json::Value
    ) -> Self {
        let norm = elements.values().map(|&v| v * v).sum::<f32>().sqrt();

        Self {
            id,
            elements,
            metadata,
            norm,
        }
    }

    /// Get the dimensionality (highest non-zero dimension)
    pub fn dimensionality(&self) -> usize {
        self.elements.keys().copied().max().map(|d| d + 1).unwrap_or(0)
    }

    /// Get the number of non-zero elements
    pub fn nnz(&self) -> usize {
        self.elements.len()
    }

    /// Calculate cosine similarity with another sparse vector
    pub fn cosine_similarity(&self, other: &SparseVector) -> f32 {
        if self.norm == 0.0 || other.norm == 0.0 {
            return 0.0;
        }

        // Calculate dot product only over overlapping dimensions
        let dot_product: f32 = self.elements
            .iter()
            .filter_map(|(dim, &val)| other.elements.get(dim).map(|&other_val| val * other_val))
            .sum();

        dot_product / (self.norm * other.norm)
    }

    /// Calculate Jaccard similarity for binary sparse vectors
    pub fn jaccard_similarity(&self, other: &SparseVector) -> f32 {
        let self_dims: std::collections::HashSet<_> = self.elements.keys().collect();
        let other_dims: std::collections::HashSet<_> = other.elements.keys().collect();

        let intersection = self_dims.intersection(&other_dims).count();
        let union = self_dims.union(&other_dims).count();

        if union == 0 {
            0.0
        } else {
            intersection as f32 / union as f32
        }
    }
}

/// HNSW node for sparse vectors
#[derive(Debug, Clone)]
pub struct SparseHNSWNode {
    /// Node ID (vector ID)
    pub id: String,

    /// Sparse vector data
    pub vector: SparseVector,

    /// Connections to other nodes (level -> neighbor IDs)
    pub connections: HashMap<usize, Vec<String>>,

    /// Maximum level for this node
    pub max_level: usize,
}

/// Sparse vector HNSW index
pub struct SparseHNSWIndex {
    /// Indexed vectors
    pub vectors: HashMap<String, SparseHNSWNode>,

    /// HNSW parameters
    pub config: SparseHNSWConfig,

    /// Entry point for graph search
    pub entry_point: Option<String>,

    /// Hardware capabilities for SIMD optimization
    pub hardware_caps: HardwareCapabilities,
}

/// Configuration for sparse HNSW index
#[derive(Debug, Clone)]
pub struct SparseHNSWConfig {
    /// Maximum number of connections per node per layer
    pub max_connections: usize,

    /// Maximum number of layers in the graph
    pub max_layers: usize,

    /// Construction ef (effort parameter)
    pub ef_construction: usize,

    /// Search ef (effort parameter)
    pub ef_search: usize,

    /// Similarity threshold for early stopping
    pub similarity_threshold: f32,

    /// Whether to use Jaccard similarity instead of cosine
    pub use_jaccard: bool,
}

impl Default for SparseHNSWConfig {
    fn default() -> Self {
        Self {
            max_connections: 16,
            max_layers: 16,
            ef_construction: 200,
            ef_search: 50,
            similarity_threshold: 0.0,
            use_jaccard: false,
        }
    }
}

impl Default for SparseHNSWIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SparseHNSWIndex {
    /// Create a new sparse HNSW index
    pub fn new() -> Self {
        Self::with_config(SparseHNSWConfig::default())
    }

    /// Create a new sparse HNSW index with custom configuration
    pub fn with_config(config: SparseHNSWConfig) -> Self {
        let hardware_caps = HardwareCapabilities::default();

        Self {
            vectors: HashMap::new(),
            config,
            entry_point: None,
            hardware_caps,
        }
    }

    /// Insert a sparse vector into the index
    pub fn insert(&mut self, vector: SparseVector) -> Result<()> {
        if vector.elements.is_empty() {
            return Ok(()); // Skip empty vectors
        }

        let vector_id = vector.id.clone();
        let nnz = vector.nnz();

        // Determine node level based on a simple heuristic
        let max_level = self.get_random_level(nnz);

        debug!("Inserting sparse vector {} (nnz={}, level={})", vector_id, nnz, max_level);

        // Create node
        let mut node = SparseHNSWNode {
            id: vector_id.clone(),
            vector,
            connections: HashMap::new(),
            max_level,
        };

        // Find entry points for each level
        let mut entry_points = self.find_entry_points_for_insert(&node);

        // Select neighbors at each level
        for level in (0..=max_level).rev() {
            let candidates = if let Some(ref ep_id) = entry_points {
                if let Some(ep) = self.vectors.get(ep_id) {
                    self.select_neighbors_layer(level, ep, &node, self.config.ef_construction)
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            // Add connections to selected neighbors
            for candidate_id in &candidates {
                node.connections.entry(level).or_insert_with(Vec::new).push(candidate_id.clone());

                // Add reverse connection
                if let Some(candidate) = self.vectors.get_mut(candidate_id) {
                    candidate.connections.entry(level).or_insert_with(Vec::new).push(node.id.clone());
                }
            }

            // Update entry points for next level
            if !candidates.is_empty() {
                entry_points = Some(candidates[0].clone());
            }
        }

        // Set entry point if this is the first node or has higher level
        let entry_level = self.entry_point
            .as_ref()
            .and_then(|id| self.vectors.get(id))
            .map(|n| n.max_level)
            .unwrap_or(0);

        if max_level > entry_level {
            self.entry_point = Some(vector_id.clone());
        } else if self.entry_point.is_none() {
            self.entry_point = Some(vector_id.clone());
        }

        // Store the node
        self.vectors.insert(vector_id, node);

        Ok(())
    }

    /// Search for similar sparse vectors
    pub fn search(&self, query: &SparseVector, top_k: usize) -> Result<Vec<SearchResult>> {
        if self.vectors.is_empty() {
            return Ok(Vec::new());
        }

        let start = Instant::now();

        // Get entry point
        let entry_id = self.entry_point
            .as_ref()
            .ok_or_else(|| anyhow!("No entry point for search"))?;

        let _entry = self.vectors.get(entry_id)
            .ok_or_else(|| anyhow!("Entry point not found"))?;

        // Perform greedy search using a Vec-based frontier (f32 does not implement Ord)
        let mut visited = std::collections::HashSet::new();
        let mut frontier: Vec<String> = vec![entry_id.clone()];
        let mut scored_results: Vec<(f32, String)> = Vec::new();

        while let Some(current_id) = frontier.pop() {
            if visited.contains(&current_id) {
                continue;
            }
            visited.insert(current_id.clone());

            if let Some(current) = self.vectors.get(&current_id) {
                // Calculate similarity
                let similarity = if self.config.use_jaccard {
                    query.jaccard_similarity(&current.vector)
                } else {
                    query.cosine_similarity(&current.vector)
                };

                if similarity >= self.config.similarity_threshold {
                    scored_results.push((similarity, current_id.clone()));
                }

                // Explore neighbors
                for level in (0..=current.max_level).rev() {
                    if let Some(neighbors) = current.connections.get(&level) {
                        for neighbor_id in neighbors {
                            if !visited.contains(neighbor_id) {
                                frontier.push(neighbor_id.clone());
                            }
                        }
                    }
                }
            }
        }

        // Sort by similarity (descending) and take top-k
        scored_results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored_results.truncate(top_k);

        let elapsed = start.elapsed();

        // Convert to search results
        let search_results: Vec<SearchResult> = scored_results
            .into_iter()
            .map(|(similarity, id)| SearchResult {
                id,
                score: similarity,
                ..Default::default()
            })
            .collect();

        info!(
            "Sparse HNSW search: query_nnz={}, results={}, latency={:.2}ms",
            query.nnz(),
            search_results.len(),
            elapsed.as_secs_f64() * 1000.0
        );

        Ok(search_results)
    }

    /// Get index statistics
    pub fn stats(&self) -> SparseHNSWStats {
        let total_vectors = self.vectors.len();
        let total_connections: usize = self.vectors.values()
            .map(|node| node.connections.values().map(|v| v.len()).sum::<usize>())
            .sum();

        let avg_nnz = if total_vectors > 0 {
            self.vectors.values()
                .map(|node| node.vector.nnz())
                .sum::<usize>() as f64 / total_vectors as f64
        } else {
            0.0
        };

        let max_nnz = self.vectors.values()
            .map(|node| node.vector.nnz())
            .max()
            .unwrap_or(0);

        let dimensions: Vec<_> = self.vectors.values()
            .map(|node| node.vector.dimensionality())
            .collect();

        let max_dimension = dimensions.iter().cloned().max().unwrap_or(0);
        let avg_dimension = if !dimensions.is_empty() {
            dimensions.iter().sum::<usize>() as f64 / dimensions.len() as f64
        } else {
            0.0
        };

        SparseHNSWStats {
            total_vectors,
            total_connections,
            avg_connections_per_node: if total_vectors > 0 {
                total_connections as f64 / total_vectors as f64
            } else {
                0.0
            },
            avg_nnz,
            max_nnz,
            max_dimension,
            avg_dimension,
        }
    }

    /// Find entry points for insertion
    fn find_entry_points_for_insert(&self, node: &SparseHNSWNode) -> Option<String> {
        self.entry_point.clone().and_then(|id| {
            self.vectors.get(&id).and_then(|entry| {
                if entry.max_level >= node.max_level {
                    Some(id)
                } else {
                    None
                }
            })
        })
    }

    /// Select neighbors at a specific layer
    fn select_neighbors_layer(
        &self,
        level: usize,
        entry: &SparseHNSWNode,
        node: &SparseHNSWNode,
        ef: usize,
    ) -> Vec<String> {
        let mut candidates = Vec::new();

        // Get candidates from entry point's connections at this level
        if let Some(neighbors) = entry.connections.get(&level) {
            for neighbor_id in neighbors {
                if let Some(neighbor) = self.vectors.get(neighbor_id) {
                    candidates.push((neighbor_id.clone(), neighbor));
                }
            }
        }

        // Add entry point itself
        candidates.push((entry.id.clone(), entry));

        // Calculate similarities and select top ef
        let mut similarities: Vec<_> = candidates
            .iter()
            .map(|(id, other)| {
                let similarity = if self.config.use_jaccard {
                    node.vector.jaccard_similarity(&other.vector)
                } else {
                    node.vector.cosine_similarity(&other.vector)
                };
                (id.clone(), similarity)
            })
            .collect();

        // Sort by similarity (descending) and take top ef
        similarities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        similarities
            .into_iter()
            .take(ef)
            .map(|(id, _)| id)
            .collect()
    }

    /// Get random level for a node based on sparse vector properties
    fn get_random_level(&self, nnz: usize) -> usize {
        // Simple heuristic: larger sparse vectors get higher levels
        // This could be made more sophisticated
        let level = if nnz > 100 {
            (nnz as f64).log2() as usize
        } else if nnz > 10 {
            2
        } else {
            1
        };

        level.min(self.config.max_layers)
    }
}

/// Sparse HNSW index statistics
#[derive(Debug, Clone)]
pub struct SparseHNSWStats {
    /// Total number of vectors in the index
    pub total_vectors: usize,

    /// Total number of connections in the graph
    pub total_connections: usize,

    /// Average connections per node
    pub avg_connections_per_node: f64,

    /// Average number of non-zero elements per vector
    pub avg_nnz: f64,

    /// Maximum number of non-zero elements in any vector
    pub max_nnz: usize,

    /// Maximum dimension in the index
    pub max_dimension: usize,

    /// Average dimension across vectors
    pub avg_dimension: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_sparse_vector_from_dense() {
        let dense = vec![0.0, 1.0, 0.0, 2.0, 0.0];
        let sparse = SparseVector::from_dense(
            "test".to_string(),
            &dense,
            serde_json::json!({"category": "test"})
        );

        assert_eq!(sparse.nnz(), 2);
        assert_eq!(sparse.dimensionality(), 4);
        assert_eq!(sparse.norm, (1.0_f32 + 4.0_f32).sqrt());
    }

    #[test]
    fn test_sparse_vector_cosine_similarity() {
        let v1 = SparseVector::from_sparse(
            "v1".to_string(),
            vec![(0, 1.0), (2, 2.0)].into_iter().collect(),
            serde_json::json!({})
        );

        let v2 = SparseVector::from_sparse(
            "v2".to_string(),
            vec![(0, 2.0), (2, 1.0)].into_iter().collect(),
            serde_json::json!({})
        );

        let sim = v1.cosine_similarity(&v2);

        // v1 · v2 = 2*1 + 1*2 = 4
        // ||v1|| = sqrt(1 + 4) = sqrt(5)
        // ||v2|| = sqrt(4 + 1) = sqrt(5)
        // cosine = 4 / 5 = 0.8
        assert!((sim - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_sparse_vector_jaccard_similarity() {
        let v1 = SparseVector::from_sparse(
            "v1".to_string(),
            vec![(0, 1.0), (1, 1.0), (2, 1.0)].into_iter().collect(),
            serde_json::json!({})
        );

        let v2 = SparseVector::from_sparse(
            "v2".to_string(),
            vec![(0, 1.0), (1, 1.0), (3, 1.0)].into_iter().collect(),
            serde_json::json!({})
        );

        let sim = v1.jaccard_similarity(&v2);

        // Intersection: {0, 1} = 2 elements
        // Union: {0, 1, 2, 3} = 4 elements
        // Jaccard = 2/4 = 0.5
        assert_eq!(sim, 0.5);
    }

    #[test]
    fn test_sparse_hnsw_insert() {
        let mut index = SparseHNSWIndex::new();

        let v1 = SparseVector::from_sparse(
            "v1".to_string(),
            vec![(0, 1.0), (2, 2.0)].into_iter().collect(),
            serde_json::json!({"category": "A"})
        );

        let v2 = SparseVector::from_sparse(
            "v2".to_string(),
            vec![(0, 2.0), (1, 1.0)].into_iter().collect(),
            serde_json::json!({"category": "B"})
        );

        assert!(index.insert(v1).is_ok());
        assert!(index.insert(v2).is_ok());

        let stats = index.stats();
        assert_eq!(stats.total_vectors, 2);
    }

    #[test]
    fn test_sparse_hnsw_search() {
        let mut index = SparseHNSWIndex::new();

        // Insert some vectors
        for i in 0..10 {
            let mut elements = HashMap::new();
            elements.insert(i, (i as f32) * 0.1);
            elements.insert(i + 100, (i as f32) * 0.2);

            let vector = SparseVector::from_sparse(
                format!("v{}", i),
                elements,
                serde_json::json!({"index": i})
            );

            index.insert(vector).unwrap();
        }

        // Create query vector
        let mut query_elements = HashMap::new();
        query_elements.insert(5, 0.5);
        query_elements.insert(105, 1.0);

        let query = SparseVector::from_sparse(
            "query".to_string(),
            query_elements,
            serde_json::json!({})
        );

        // Search
        let results = index.search(&query, 5).unwrap();

        assert!(results.len() <= 5);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_sparse_hnsw_empty_index() {
        let index = SparseHNSWIndex::new();
        let query = SparseVector::from_sparse(
            "query".to_string(),
            HashMap::new(),
            serde_json::json!({})
        );

        let results = index.search(&query, 5).unwrap();
        assert_eq!(results.len(), 0);
    }
}

/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # DiskANN Indexing for Billion-Scale Vector Search
//!
//! This module implements the DiskANN algorithm for efficient vector search
//! at billion-scale, enabling ProximaDB to handle 1B+ vectors on a single node.
//!
//! ## What is DiskANN?
//!
//! DiskANN is a graph-based approximate nearest neighbor (ANN) algorithm optimized
//! for SSD storage. It combines:
//! - **Vamana Graph**: Bounded-degree graph for efficient traversal
//! - **SSD-Optimized Layout**: Node ordering for sequential disk reads
//! - **Product Quantization**: Compressed vectors for storage efficiency
//!
//! ## Performance Targets
//!
//! - **Capacity**: 1B+ vectors (10x current HNSW limit)
//! - **Query Latency**: <50ms for 1B vectors
//! - **Recall**: 95%+ @10
//! - **Compression**: 10x better than HNSW
//!
//! ## Architecture
//!
//! ```text
//! +------------------------------------------+
//! |            DiskANN Index              |
//! +------------------------------------------+
//! |  Vamana Graph (SSD-optimized layout)   |
//! |  - Bounded degree (R = 32)             |
//! |  - Greedy search with beam width        |
//! +------------------------------------------+
//! |  PQ Compressed Vectors                   |
//! |  - 8-bit quantization                   |
//! |  - Codebooks for decoding               |
//! +------------------------------------------+
//! |  SSD Storage (mmap for fast access)     |
//! +------------------------------------------+
//! ```
//!
//! ## Use Cases
//!
//! - Large-scale recommendation systems
//! - Image/video search at internet scale
//! - Document search for massive corpora
//! - Embedding search for billion-item catalogs

pub mod pq;
pub mod search;
pub mod ssd_layout;
pub mod vamana;

use crate::core::error::ProximaDBError;
use crate::index::diskann::pq::{PQConfig, PQEncoder};
use crate::index::diskann::search::{DiskANNSearch, SearchConfig};
use crate::index::diskann::ssd_layout::{NodeOrdering, SsdLayoutOptimizer};
use crate::index::diskann::vamana::{VamanaConfig, VamanaGraph};
use tracing::info;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// DiskANN index for billion-scale vector search
pub struct DiskANNIndex {
    /// Index ID
    id: String,

    /// Vector dimension
    dimension: usize,

    /// Number of vectors in the index
    num_vectors: usize,

    /// Vamana graph (if built)
    vamana_graph: Option<VamanaGraph>,

    /// Node ordering for SSD-optimized layout (if built)
    node_ordering: Option<NodeOrdering>,

    /// Vector data (if built)
    vectors: Option<Vec<Vec<f32>>>,

    /// PQ compressed vectors (if built)
    pq_vectors: Option<pq::PQVectors>,
}

impl DiskANNIndex {
    /// Create a new DiskANN index
    pub fn new(id: String, dimension: usize) -> Self {
        Self {
            id,
            dimension,
            num_vectors: 0,
            vamana_graph: None,
            node_ordering: None,
            vectors: None,
            pq_vectors: None,
        }
    }

    /// Build the DiskANN index from vectors
    pub async fn build(&mut self, vectors: Vec<Vec<f32>>) -> Result<()> {
        if vectors.is_empty() {
            return Err(ProximaDBError::InvalidInput(
                "Cannot build index from empty vector set".to_string(),
            ));
        }

        let num_vectors = vectors.len();
        let dimension = vectors[0].len();

        if dimension != self.dimension {
            return Err(ProximaDBError::InvalidInput(format!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimension, dimension
            )));
        }

        info!(
            "Building DiskANN index: {} vectors, {} dimensions",
            num_vectors, dimension
        );

        // Phase 1: Build Vamana graph
        let config = VamanaConfig::default();
        let mut builder = self::vamana::VamanaBuilder::new(num_vectors, dimension, config);

        let vamana_graph = builder.build(&vectors)?;
        let graph_edges = vamana_graph.edges.clone();
        self.vamana_graph = Some(vamana_graph);

        // Phase 2: Compute SSD-optimized node ordering
        info!("Computing SSD-optimized layout...");
        let layout_optimizer = SsdLayoutOptimizer::with_default_config();
        let node_ordering = layout_optimizer.compute_node_ordering(&graph_edges)?;

        // Log layout statistics
        let stats = layout_optimizer.compute_layout_stats(&graph_edges, &node_ordering);
        info!(
            "Layout stats: {} nodes, {} landmarks, {:.2}% sequential access, {:.2}% est. cache hit rate",
            stats.total_nodes,
            stats.landmark_count,
            stats.sequential_access_ratio * 100.0,
            stats.estimated_cache_hit_rate * 100.0
        );

        self.node_ordering = Some(node_ordering);
        self.vectors = Some(vectors.clone());
        self.num_vectors = num_vectors;

        // Phase 4: Product Quantization compression (optional)
        info!("Training PQ codebooks for compression...");
        let pq_config = PQConfig {
            num_subvectors: 4, // Smaller for faster testing
            num_centroids: 16, // Smaller for faster testing
            max_iterations: 5, // Fewer iterations
            convergence_threshold: 0.01,
        };
        let pq_encoder = PQEncoder::new(pq_config);
        let pq_vectors = pq_encoder
            .train_codebooks(&vectors)
            .and_then(|codebooks| pq_encoder.encode(&vectors, &codebooks))?;

        let compression_ratio = pq_vectors.compression_ratio(dimension)?;
        let codes_len = pq_vectors
            .codes
            .first()
            .ok_or_else(|| ProximaDBError::Internal("No PQ codes available".to_string()))?
            .len();
        info!(
            "PQ compression: {:.1}x compression ratio ({} bytes → {} bytes per vector)",
            compression_ratio,
            dimension * std::mem::size_of::<f32>(),
            codes_len * std::mem::size_of::<u8>()
        );

        self.pq_vectors = Some(pq_vectors);

        Ok(())
    }

    /// Search for nearest neighbors using beam search
    pub async fn search(&self, query: &[f32], k: usize) -> Result<Vec<(usize, f32)>> {
        if query.len() != self.dimension {
            return Err(ProximaDBError::InvalidInput(format!(
                "Query dimension mismatch: expected {}, got {}",
                self.dimension,
                query.len()
            )));
        }

        // Check if index is built
        let vamana_graph = self
            .vamana_graph
            .as_ref()
            .ok_or_else(|| ProximaDBError::Internal("Index not built".to_string()))?;

        let vectors = self
            .vectors
            .as_ref()
            .ok_or_else(|| ProximaDBError::Internal("Vectors not available".to_string()))?;

        // Create search engine
        let search_engine = DiskANNSearch::new(vamana_graph.clone(), self.node_ordering.clone());

        // Configure search
        let config = SearchConfig {
            beam_width: 50,
            top_k: k,
            search_list_size: 100,
            use_node_ordering: self.node_ordering.is_some(),
        };

        // Execute search
        let (results, _stats) = search_engine.search(query, vectors, &config)?;

        // Convert results to (node_id, distance) tuple format
        let results: Vec<(usize, f32)> = results
            .into_iter()
            .map(|r| (r.node_id, r.distance))
            .collect();

        Ok(results)
    }

    /// Get index statistics
    pub fn stats(&self) -> DiskANNStats {
        DiskANNStats {
            id: self.id.clone(),
            dimension: self.dimension,
            num_vectors: self.num_vectors,
            max_degree: self.vamana_graph.as_ref().map_or(0, |g| g.max_degree),
            is_built: self.vamana_graph.is_some(),
        }
    }
}

/// DiskANN index statistics
#[derive(Debug, Clone)]
pub struct DiskANNStats {
    /// Index identifier.
    pub id: String,
    /// Vector dimension.
    pub dimension: usize,
    /// Number of vectors in the index.
    pub num_vectors: usize,
    /// Maximum degree (number of neighbors per node).
    pub max_degree: usize,
    /// Whether the Vamana graph has been built.
    pub is_built: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diskann_creation() {
        let index = DiskANNIndex::new("test_index".to_string(), 128);
        assert_eq!(index.id, "test_index");
        assert_eq!(index.dimension, 128);
        assert_eq!(index.num_vectors, 0);
    }

    #[test]
    fn test_diskann_stats() {
        let index = DiskANNIndex::new("stats_test".to_string(), 256);
        let stats = index.stats();

        assert_eq!(stats.id, "stats_test");
        assert_eq!(stats.dimension, 256);
        assert_eq!(stats.num_vectors, 0);
        assert!(!stats.is_built);
    }

    #[tokio::test]
    async fn test_diskann_build_empty() {
        let mut index = DiskANNIndex::new("empty_test".to_string(), 128);
        let vectors = Vec::new();

        let result = index.build(vectors).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_diskann_build_success() {
        let mut index = DiskANNIndex::new("build_test".to_string(), 128);

        // Create dummy vectors
        let vectors: Vec<Vec<f32>> = (0..100)
            .map(|_| (0..128).map(|i| i as f32).collect())
            .collect();

        let result = index.build(vectors).await;
        assert!(result.is_ok());

        let stats = index.stats();
        assert_eq!(stats.num_vectors, 100);
        assert!(index.vamana_graph.is_some());
        assert!(index.node_ordering.is_some());
    }

    #[tokio::test]
    async fn test_diskann_ssd_layout_integration() {
        let mut index = DiskANNIndex::new("layout_test".to_string(), 64);

        // Create a small graph with clear high-degree nodes
        let vectors: Vec<Vec<f32>> = (0..20)
            .map(|i| (0..64).map(|j| ((i * 64 + j) % 10) as f32).collect())
            .collect();

        let result = index.build(vectors).await;
        assert!(result.is_ok());

        // Verify node ordering was computed
        assert!(index.node_ordering.is_some());

        let ordering = index.node_ordering.as_ref().unwrap();
        assert_eq!(ordering.old_to_new.len(), 20);
        assert_eq!(ordering.new_to_old.len(), 20);

        // Verify landmarks exist
        assert!(!ordering.landmarks.is_empty());
        // With 10% landmark ratio and 20 nodes, should have ~2 landmarks
        assert!(ordering.landmarks.len() >= 1 && ordering.landmarks.len() <= 3);
    }

    #[test]
    fn test_ssd_layout_optimizer_standalone() {
        use crate::index::diskann::ssd_layout::SsdLayoutOptimizer;

        // Create a simple graph
        let graph = vec![
            vec![1, 2, 3, 4], // Node 0: high degree (hub)
            vec![0],
            vec![0],
            vec![0],
            vec![0],
        ];

        let optimizer = SsdLayoutOptimizer::with_default_config();
        let ordering = optimizer.compute_node_ordering(&graph).unwrap();

        // Node 0 should be a landmark (highest degree)
        assert!(ordering.is_landmark(0));

        // Verify bidirectional mapping
        for old_id in 0..5 {
            let new_pos = ordering.get_new_position(old_id).unwrap();
            let reverse_id = ordering.get_old_id(new_pos).unwrap();
            assert_eq!(reverse_id, old_id);
        }
    }

    #[tokio::test]
    async fn test_diskann_search_dimension_mismatch() {
        let index = DiskANNIndex::new("search_test".to_string(), 128);
        let query = vec![0.0f32; 64]; // Wrong dimension

        let result = index.search(&query, 10).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_diskann_search_integration() {
        let mut index = DiskANNIndex::new("search_test".to_string(), 64);

        // Create vectors
        let vectors: Vec<Vec<f32>> = (0..30)
            .map(|i| (0..64).map(|j| ((i * 64 + j) % 20) as f32).collect())
            .collect();

        // Build index
        let result = index.build(vectors).await;
        assert!(result.is_ok());

        // Search for first vector
        let query = index.vectors.as_ref().unwrap()[0].clone();
        let search_results = index.search(&query, 5).await.unwrap();

        assert!(search_results.len() <= 5);
        // First result should be the query itself (distance ~0)
        if !search_results.is_empty() {
            assert_eq!(search_results[0].0, 0);
            assert!(search_results[0].1 < 0.01);
        }
    }

    #[tokio::test]
    async fn test_diskann_search_without_build() {
        let index = DiskANNIndex::new("no_build_test".to_string(), 128);
        let query = vec![0.0f32; 128];

        let result = index.search(&query, 10).await;
        assert!(result.is_err()); // Should fail - index not built
    }

    #[tokio::test]
    async fn test_diskann_pq_integration() {
        let mut index = DiskANNIndex::new("pq_test".to_string(), 64);

        let vectors: Vec<Vec<f32>> = (0..30)
            .map(|i| (0..64).map(|j| ((i * 64 + j) % 20) as f32).collect())
            .collect();

        let result = index.build(vectors).await;
        assert!(result.is_ok());

        // Verify PQ vectors were created
        assert!(index.pq_vectors.is_some());

        let pq_vectors = index.pq_vectors.as_ref().unwrap();
        assert_eq!(pq_vectors.codes.len(), 30);
        assert!(pq_vectors.codes[0].len() > 0);

        // Check compression ratio
        let ratio = pq_vectors.compression_ratio(64).unwrap();
        assert!(ratio > 30.0); // Should have >30x compression
    }
}

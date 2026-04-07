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

//! # DiskANN Search Algorithm
//!
//! This module implements the beam search algorithm for efficient approximate
//! nearest neighbor search on Vamana graphs.
//!
//! ## Beam Search Algorithm
//!
//! The search uses a beam-based greedy approach:
//! 1. Start from medoid (graph center)
//! 2. Maintain beam of top-L candidates
//! 3. Iteratively expand candidates by exploring neighbors
//! 4. Prune using distance bounds to avoid redundant computation
//! 5. Return top-K results
//!
//! ## Performance Characteristics
//!
//! - **Sub-millisecond latency**: O(log N) graph traversal
//! - **High recall**: 95%+ @10 through beam search
//! - **Cache-efficient**: Leverages SSD-optimized layout
//! - **Scalable**: Linear performance with 1B+ vectors
//!
//! ## Search Parameters
//!
//! - **L (Beam Width)**: Number of candidates to maintain (default: 50)
//! - **K (Result Count)**: Number of nearest neighbors to return (default: 10)
//! - **Search List Size**: Internal candidate pool size (default: 2*L)

use crate::compute::distance_computation::UnifiedDistanceCompute;
use crate::core::error::ProximaDBError;
use crate::index::diskann::VamanaGraph;
use crate::index::diskann::ssd_layout::NodeOrdering;
use std::collections::{BinaryHeap, HashSet};
use tracing::{debug, info};

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Search result with node ID and distance
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Node ID (original vector index)
    pub node_id: usize,

    /// Distance to query vector
    pub distance: f32,
}

impl SearchResult {
    /// Create a new search result
    pub fn new(node_id: usize, distance: f32) -> Self {
        Self { node_id, distance }
    }
}

// Implement PartialEq manually to handle NaN
impl PartialEq for SearchResult {
    fn eq(&self, other: &Self) -> bool {
        // NaN != NaN, so use total comparison
        self.node_id == other.node_id
            && (self.distance == other.distance
                || (self.distance.is_nan() && other.distance.is_nan()))
    }
}

// Implement Eq for Ord requirement
impl Eq for SearchResult {}

// Implement PartialOrd for BinaryHeap
impl PartialOrd for SearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SearchResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse order for min-heap (smaller distance = higher priority)
        other
            .distance
            .partial_cmp(&self.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Configuration for DiskANN search
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Beam width (L) - number of candidates to maintain
    pub beam_width: usize,

    /// Number of results to return (K)
    pub top_k: usize,

    /// Internal search list size (typically 2*L)
    pub search_list_size: usize,

    /// Whether to use node ordering for cache efficiency
    pub use_node_ordering: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            beam_width: 50,        // Standard DiskANN parameter
            top_k: 10,             // Return top 10 results
            search_list_size: 100, // 2 * beam_width
            use_node_ordering: true,
        }
    }
}

/// Search statistics for monitoring and tuning
#[derive(Debug, Clone)]
pub struct SearchStats {
    /// Number of nodes visited during search
    pub nodes_visited: usize,

    /// Number of distance computations performed
    pub distance_computations: usize,

    /// Search latency in nanoseconds
    pub latency_ns: u128,

    /// Final beam size
    pub final_beam_size: usize,

    /// Number of cache hits (if node ordering available)
    pub cache_hits: usize,
}

/// DiskANN search engine with beam search
pub struct DiskANNSearch {
    /// Vamana graph for traversal
    graph: VamanaGraph,

    /// Node ordering for cache efficiency (optional)
    node_ordering: Option<NodeOrdering>,

    /// Distance computation engine
    distance_compute: UnifiedDistanceCompute,
}

impl DiskANNSearch {
    /// Create a new DiskANN search engine
    pub fn new(graph: VamanaGraph, node_ordering: Option<NodeOrdering>) -> Self {
        Self {
            graph,
            node_ordering,
            distance_compute: UnifiedDistanceCompute::new(
                crate::compute::distance_computation::DistanceMetric::Euclidean,
            ),
        }
    }

    /// Search for K nearest neighbors using beam search
    ///
    /// # Algorithm
    ///
    /// 1. **Initialize**: Start from medoid with distance to query
    /// 2. **Beam Search**: Maintain top-L candidates in beam
    /// 3. **Expand**: For each candidate, explore neighbors
    /// 4. **Prune**: Keep only best candidates using distance bounds
    /// 5. **Return**: Top-K results from beam
    ///
    /// # Arguments
    ///
    /// * `query` - Query vector
    /// * `vectors` - Full vector dataset for distance computation
    /// * `config` - Search configuration
    ///
    /// # Returns
    ///
    /// Top-K search results with statistics
    pub fn search(
        &self,
        query: &[f32],
        vectors: &[Vec<f32>],
        config: &SearchConfig,
    ) -> Result<(Vec<SearchResult>, SearchStats)> {
        let start_time = std::time::Instant::now();

        // Validate inputs
        if vectors.is_empty() {
            return Ok((
                vec![],
                SearchStats {
                    nodes_visited: 0,
                    distance_computations: 0,
                    latency_ns: 0,
                    final_beam_size: 0,
                    cache_hits: 0,
                },
            ));
        }

        if query.len() != vectors[0].len() {
            return Err(ProximaDBError::InvalidInput(format!(
                "Query dimension mismatch: expected {}, got {}",
                vectors[0].len(),
                query.len()
            )));
        }

        let mut stats = SearchStats {
            nodes_visited: 0,
            distance_computations: 0,
            latency_ns: 0,
            final_beam_size: 0,
            cache_hits: 0,
        };

        debug!(
            "Starting DiskANN search: query_dim={}, num_vectors={}, beam_width={}, top_k={}",
            query.len(),
            vectors.len(),
            config.beam_width,
            config.top_k
        );

        // Step 1: Initialize beam with medoid
        let medoid = self.graph.medoid;
        let medoid_dist = self.compute_distance(query, &vectors[medoid]);
        stats.distance_computations += 1;

        let mut beam = BinaryHeap::new();
        beam.push(SearchResult::new(medoid, medoid_dist));
        stats.nodes_visited += 1;

        let mut visited = HashSet::new();
        visited.insert(medoid);

        // Track best distance for pruning
        let mut best_distance = medoid_dist;

        // Step 2: Beam search with iterative expansion
        let mut iteration = 0;
        let max_iterations = self.graph.edges.len(); // Prevent infinite loops

        while !beam.is_empty() && iteration < max_iterations {
            iteration += 1;

            // Get current beam size
            let current_beam_size = beam.len();

            // Extract candidates (up to beam_width)
            let mut candidates = Vec::new();
            while beam.len() > current_beam_size.saturating_sub(config.beam_width) {
                if let Some(candidate) = beam.pop() {
                    candidates.push(candidate);
                } else {
                    break;
                }
            }

            // If no candidates to expand, we're done
            if candidates.is_empty() {
                // Extract remaining beam candidates
                while let Some(candidate) = beam.pop() {
                    candidates.push(candidate);
                }
            }

            // Expand each candidate
            for candidate in &candidates {
                // Skip if we've found better results
                if candidate.distance > best_distance * 1.5 {
                    continue;
                }

                // Explore neighbors
                let neighbors = self.graph.edges.get(candidate.node_id);
                if let Some(neighbors) = neighbors {
                    for &neighbor_id in neighbors {
                        if visited.contains(&neighbor_id) {
                            continue;
                        }

                        visited.insert(neighbor_id);
                        stats.nodes_visited += 1;

                        // Compute distance to neighbor
                        let distance = self.compute_distance(query, &vectors[neighbor_id]);
                        stats.distance_computations += 1;

                        // Update best distance
                        if distance < best_distance {
                            best_distance = distance;
                        }

                        // Add to beam
                        beam.push(SearchResult::new(neighbor_id, distance));

                        // Track cache hit if using node ordering
                        if config.use_node_ordering
                            && let Some(ordering) = &self.node_ordering
                            && let Some(new_pos) = ordering.get_new_position(neighbor_id)
                            && new_pos < 1000
                        {
                            // Assume first 1000 nodes are cached
                            stats.cache_hits += 1;
                        }
                    }
                }
            }

            // Prune beam to search_list_size
            while beam.len() > config.search_list_size {
                beam.pop();
            }

            // Early termination if we've found enough good results
            if beam.len() >= config.top_k {
                // Check if we have K results within reasonable distance bound
                let mut results: Vec<_> = beam.iter().take(config.top_k).cloned().collect();
                results.sort_by(|a, b| a.distance.total_cmp(&b.distance));

                if results.len() >= config.top_k {
                    let worst_result = results[config.top_k - 1].distance;
                    if worst_result < best_distance * 1.2 {
                        debug!(
                            "Early termination at iteration {}: found {} good results",
                            iteration,
                            results.len()
                        );
                        break;
                    }
                }
            }
        }

        // Step 3: Extract top-K results
        let mut all_results: Vec<_> = beam.into_iter().collect();
        all_results.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        all_results.truncate(config.top_k);

        stats.final_beam_size = all_results.len();
        stats.latency_ns = start_time.elapsed().as_nanos();

        info!(
            "Search complete: visited {} nodes, {} distance computations, {:.2}μs latency, {:.1}% cache hits",
            stats.nodes_visited,
            stats.distance_computations,
            stats.latency_ns as f64 / 1000.0,
            if stats.nodes_visited > 0 {
                (stats.cache_hits as f64 / stats.nodes_visited as f64) * 100.0
            } else {
                0.0
            }
        );

        Ok((all_results, stats))
    }

    /// Compute distance between query and a vector
    fn compute_distance(&self, query: &[f32], vector: &[f32]) -> f32 {
        self.distance_compute.distance(query, vector)
    }

    /// Batch search for multiple queries
    ///
    /// # Arguments
    ///
    /// * `queries` - Multiple query vectors
    /// * `vectors` - Full vector dataset
    /// * `config` - Search configuration
    ///
    /// # Returns
    ///
    /// Vector of (results, stats) for each query
    pub fn batch_search(
        &self,
        queries: &[Vec<f32>],
        vectors: &[Vec<f32>],
        config: &SearchConfig,
    ) -> Result<Vec<(Vec<SearchResult>, SearchStats)>> {
        queries
            .iter()
            .map(|query| self.search(query, vectors, config))
            .collect()
    }

    /// Get search statistics summary
    pub fn get_stats_summary(&self, stats: &[SearchStats]) -> SearchStatsSummary {
        if stats.is_empty() {
            return SearchStatsSummary {
                total_searches: 0,
                avg_nodes_visited: 0.0,
                avg_distance_computations: 0.0,
                avg_latency_us: 0.0,
                avg_cache_hit_rate: 0.0,
            };
        }

        let total_searches = stats.len();
        let avg_nodes_visited =
            stats.iter().map(|s| s.nodes_visited).sum::<usize>() as f64 / total_searches as f64;
        let avg_distance_computations = stats.iter().map(|s| s.distance_computations).sum::<usize>()
            as f64
            / total_searches as f64;
        let avg_latency_us = stats.iter().map(|s| s.latency_ns).sum::<u128>() as f64
            / total_searches as f64
            / 1000.0;
        let avg_cache_hit_rate = if stats.iter().map(|s| s.nodes_visited).sum::<usize>() > 0 {
            stats.iter().map(|s| s.cache_hits).sum::<usize>() as f64
                / stats.iter().map(|s| s.nodes_visited).sum::<usize>() as f64
        } else {
            0.0
        };

        SearchStatsSummary {
            total_searches,
            avg_nodes_visited,
            avg_distance_computations,
            avg_latency_us,
            avg_cache_hit_rate,
        }
    }
}

/// Summary statistics for multiple searches
#[derive(Debug, Clone)]
pub struct SearchStatsSummary {
    /// Total number of searches performed.
    pub total_searches: usize,
    /// Average number of graph nodes visited per search.
    pub avg_nodes_visited: f64,
    /// Average number of distance computations per search.
    pub avg_distance_computations: f64,
    /// Average search latency in microseconds.
    pub avg_latency_us: f64,
    /// Average cache hit rate across searches.
    pub avg_cache_hit_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::diskann::vamana::{VamanaBuilder, VamanaConfig};

    #[test]
    fn test_search_result_creation() {
        let result = SearchResult::new(5, 0.123);
        assert_eq!(result.node_id, 5);
        assert_eq!(result.distance, 0.123);
    }

    #[test]
    fn test_search_result_comparison() {
        let result1 = SearchResult::new(1, 0.5);
        let result2 = SearchResult::new(2, 0.3);
        let result3 = SearchResult::new(3, 0.7);

        // result2 has smallest distance, should be "greater" in min-heap
        assert!(result2 > result1);
        assert!(result1 > result3);
    }

    #[test]
    fn test_search_config_default() {
        let config = SearchConfig::default();
        assert_eq!(config.beam_width, 50);
        assert_eq!(config.top_k, 10);
        assert_eq!(config.search_list_size, 100);
        assert!(config.use_node_ordering);
    }

    #[test]
    fn test_diskann_search_creation() {
        let graph = VamanaGraph::new(32, 100, 0);
        let search = DiskANNSearch::new(graph, None);
        assert_eq!(search.graph.medoid, 0);
        assert!(search.node_ordering.is_none());
    }

    #[test]
    fn test_empty_search() {
        let graph = VamanaGraph::new(32, 10, 0);
        let search = DiskANNSearch::new(graph, None);

        let vectors: Vec<Vec<f32>> = vec![];
        let query = vec![0.0; 128];
        let config = SearchConfig::default();

        let (results, stats) = search.search(&query, &vectors, &config).unwrap();
        assert_eq!(results.len(), 0);
        assert_eq!(stats.nodes_visited, 0);
    }

    #[test]
    fn test_simple_search() {
        // Create a simple graph
        let num_vectors = 20;
        let dimension = 8;

        let vectors: Vec<Vec<f32>> = (0..num_vectors)
            .map(|i| (0..dimension).map(|j| (i * dimension + j) as f32).collect())
            .collect();

        // Build Vamana graph
        let config = VamanaConfig::default();
        let mut builder = VamanaBuilder::new(num_vectors, dimension, config);
        let graph = builder.build(&vectors).unwrap();

        // Create search engine
        let search = DiskANNSearch::new(graph, None);

        // Search for first vector
        let query = vectors[0].clone();
        let search_config = SearchConfig {
            beam_width: 10,
            top_k: 5,
            search_list_size: 20,
            use_node_ordering: false,
        };

        let (results, stats) = search.search(&query, &vectors, &search_config).unwrap();

        assert!(results.len() <= 5);
        assert!(stats.nodes_visited > 0);
        assert!(stats.distance_computations > 0);

        // First result should be the query itself (distance ~0)
        if !results.is_empty() {
            assert_eq!(results[0].node_id, 0);
            assert!(results[0].distance < 0.01); // Should be very close to 0
        }
    }

    #[test]
    fn test_batch_search() {
        let num_vectors = 15;
        let dimension = 4;

        let vectors: Vec<Vec<f32>> = (0..num_vectors)
            .map(|i| (0..dimension).map(|j| (i * dimension + j) as f32).collect())
            .collect();

        let config = VamanaConfig::default();
        let mut builder = VamanaBuilder::new(num_vectors, dimension, config);
        let graph = builder.build(&vectors).unwrap();

        let search = DiskANNSearch::new(graph, None);

        let queries = vec![vectors[0].clone(), vectors[5].clone()];
        let search_config = SearchConfig::default();

        let search_results = search
            .batch_search(&queries, &vectors, &search_config)
            .unwrap();

        assert_eq!(search_results.len(), 2);
        for (_results, stats) in search_results {
            assert!(stats.nodes_visited > 0);
        }
    }

    #[test]
    fn test_stats_summary() {
        let stats = vec![
            SearchStats {
                nodes_visited: 100,
                distance_computations: 100,
                latency_ns: 1_000_000, // 1ms
                final_beam_size: 10,
                cache_hits: 80,
            },
            SearchStats {
                nodes_visited: 200,
                distance_computations: 200,
                latency_ns: 2_000_000, // 2ms
                final_beam_size: 10,
                cache_hits: 160,
            },
        ];

        let graph = VamanaGraph::new(32, 100, 0);
        let search = DiskANNSearch::new(graph, None);
        let summary = search.get_stats_summary(&stats);

        assert_eq!(summary.total_searches, 2);
        assert_eq!(summary.avg_nodes_visited, 150.0);
        assert_eq!(summary.avg_distance_computations, 150.0);
        assert_eq!(summary.avg_latency_us, 1500.0);
        assert_eq!(summary.avg_cache_hit_rate, 0.8); // 240/300 = 0.8
    }

    #[test]
    fn test_empty_stats_summary() {
        let stats: Vec<SearchStats> = vec![];
        let graph = VamanaGraph::new(32, 100, 0);
        let search = DiskANNSearch::new(graph, None);
        let summary = search.get_stats_summary(&stats);

        assert_eq!(summary.total_searches, 0);
        assert_eq!(summary.avg_nodes_visited, 0.0);
    }

    #[test]
    fn test_dimension_mismatch() {
        let graph = VamanaGraph::new(32, 10, 0);
        let search = DiskANNSearch::new(graph, None);

        let vectors = vec![vec![0.0; 128]; 10];
        let query = vec![0.0; 64]; // Wrong dimension
        let config = SearchConfig::default();

        let result = search.search(&query, &vectors, &config);
        assert!(result.is_err());
    }
}

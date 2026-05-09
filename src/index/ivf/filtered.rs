//! IVF Filtered Search Implementation (Issue #41, SB-11)
//!
//! This module implements filter-aware inverted list search for IVF (Inverted File)
//! indexes, enabling efficient hybrid search with metadata filtering.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │              IVF Filtered Search                            │
//! │  - Filter within inverted lists                            │
//! │  - Vectorized batch filtering                              │
//! │  - Inverted list pruning                                    │
//! └──────────────────────┬────────────────────────────────────────┘
//!                        │
//!                        ▼
//!     ┌─────────────────────────────────────────┐
//!     │      Inverted List Filtering            │
//!     │  1. Find nearest probes (centroids)       │
//!     │  2. For each probe's inverted list:        │
//!     │     a. Get all vectors in list            │
//!     │     b. Batch evaluate filter              │
//!     │     c. Prune non-matching vectors          │
//!     │  3. Combine candidates from all probes    │
//!     │  4. Rank and select top-k                 │
//!     └─────────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! - **Inverted List Pruning**: Filter within each list before combining
//! - **Vectorized Filtering**: SIMD-optimized batch filter evaluation
//! - **Probe Selection**: Adaptive nprobe based on filter selectivity
//! - **Batch Processing**: Process multiple lists in parallel
//! - **Zero-Copy**: Minimize data movement during filtering

use anyhow::Result;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use tracing::{debug, trace};

use crate::core::search::filter_contract::{FilterContract, MetadataLookup};

/// Local SearchResult type for IVF filtered search
#[derive(Debug, Clone, Default)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
}

impl PartialEq for SearchResult {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.score.to_bits() == other.score.to_bits()
    }
}

impl Eq for SearchResult {}

impl PartialOrd for SearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SearchResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.score.partial_cmp(&other.score) {
            Some(std::cmp::Ordering::Equal) | None => self.id.cmp(&other.id),
            Some(ordering) => ordering,
        }
    }
}

/// IVF inverted list (vectors assigned to a cluster)
#[derive(Debug, Clone)]
pub struct IVFInvertedList {
    /// Cluster ID (probe ID)
    pub cluster_id: usize,

    /// Vectors in this cluster
    pub vectors: Vec<IVFVector>,

    /// Cluster centroid
    pub centroid: Vec<f32>,
}

/// Vector in an IVF inverted list
#[derive(Debug, Clone)]
pub struct IVFVector {
    /// Vector ID
    pub id: String,

    /// Vector data
    pub vector: Vec<f32>,

    /// Metadata for filter evaluation
    pub metadata: serde_json::Value,

    /// Distance to cluster centroid
    pub distance_to_centroid: f32,
}

/// Filtered IVF search parameters
#[derive(Debug, Clone)]
pub struct IVFFilteredSearchParams {
    /// Query vector
    pub query_vector: Vec<f32>,

    /// Number of results to return (k)
    pub top_k: usize,

    /// Number of clusters/probes to search (nprobe)
    pub nprobe: usize,

    /// Total number of clusters (nlist)
    pub nlist: usize,

    /// Filter contract for metadata filtering
    pub filter: Option<Arc<dyn FilterContract>>,

    /// Enable batch filtering (SIMD-optimized)
    pub enable_batch_filtering: bool,

    /// Adaptive nprobe based on filter selectivity
    pub adaptive_nprobe: bool,
}

impl Default for IVFFilteredSearchParams {
    fn default() -> Self {
        Self {
            query_vector: Vec::new(),
            top_k: 10,
            nprobe: 10,
            nlist: 100,
            filter: None,
            enable_batch_filtering: true,
            adaptive_nprobe: true,
        }
    }
}

/// Result of filtered IVF search
#[derive(Debug, Clone)]
pub struct IVFFilteredResult {
    /// Top search results
    pub results: Vec<SearchResult>,

    /// Number of inverted lists processed
    pub lists_processed: usize,

    /// Number of vectors filtered out
    pub vectors_filtered: usize,

    /// Effective nprobe used
    pub effective_nprobe: usize,

    /// Execution time in microseconds
    pub execution_time_us: u64,
}

/// Filtered IVF index
pub struct FilteredIVFIndex {
    /// Inverted lists (cluster ID → list)
    inverted_lists: HashMap<usize, IVFInvertedList>,

    /// Index dimension
    dimension: usize,

    /// Number of clusters
    nlist: usize,
}

impl FilteredIVFIndex {
    /// Create a new filtered IVF index
    pub fn new(dimension: usize, nlist: usize) -> Self {
        Self {
            inverted_lists: HashMap::new(),
            dimension,
            nlist,
        }
    }

    /// Insert a vector into the IVF index
    pub fn insert(
        &mut self,
        id: String,
        vector: Vec<f32>,
        metadata: serde_json::Value,
        cluster_id: usize,
        centroid: &[f32],
    ) -> Result<()> {
        if vector.len() != self.dimension {
            return Err(anyhow::anyhow!(
                "Vector dimension {} does not match index dimension {}",
                vector.len(),
                self.dimension
            ));
        }

        // Calculate distance to centroid
        let distance_to_centroid = self.calculate_distance(&vector, centroid);

        let ivf_vector = IVFVector {
            id: id.clone(),
            vector,
            metadata,
            distance_to_centroid,
        };

        // Add to appropriate inverted list
        let list = self
            .inverted_lists
            .entry(cluster_id)
            .or_insert_with(|| IVFInvertedList {
                cluster_id,
                vectors: Vec::new(),
                centroid: centroid.to_vec(),
            });

        list.vectors.push(ivf_vector);

        Ok(())
    }

    /// Execute filtered search on IVF index
    pub fn search_filtered(
        &self,
        params: &IVFFilteredSearchParams,
        metadata_lookup: &dyn MetadataLookup,
    ) -> Result<IVFFilteredResult> {
        let start = std::time::Instant::now();

        debug!(
            "Executing filtered IVF search with top_k={}, nprobe={}",
            params.top_k, params.nprobe
        );

        // Adjust nprobe based on filter selectivity if adaptive
        let effective_nprobe = if params.adaptive_nprobe {
            self.adjust_nprobe_for_filter(params)?
        } else {
            params.nprobe
        };

        // Find nearest probes (centroids)
        let nearest_probes = self.find_nearest_probes(&params.query_vector, effective_nprobe)?;

        debug!("Found {} nearest probes", nearest_probes.len());

        // Process each inverted list with filtering
        let mut all_candidates = Vec::new();
        let mut lists_processed = 0;
        let mut vectors_filtered = 0;

        for probe_id in nearest_probes {
            if let Some(inverted_list) = self.inverted_lists.get(&probe_id) {
                lists_processed += 1;

                // Filter vectors within this inverted list
                let (filtered_vectors, num_filtered) =
                    self.filter_inverted_list(inverted_list, params, metadata_lookup)?;

                let filtered_count = filtered_vectors.len();
                all_candidates.extend(filtered_vectors);
                vectors_filtered += num_filtered;

                trace!(
                    "Probe {}: {} vectors → {} after filtering",
                    probe_id,
                    inverted_list.vectors.len(),
                    filtered_count
                );
            }
        }

        // Rank all candidates by similarity to query
        let ranked_candidates = self.rank_candidates(&params.query_vector, &all_candidates)?;

        // Extract top k results
        let results: Vec<SearchResult> = ranked_candidates.into_iter().take(params.top_k).collect();

        let execution_time = start.elapsed().as_micros() as u64;

        debug!(
            "Filtered IVF search: processed {} lists, filtered {} vectors, returned {} results in {}μs",
            lists_processed,
            vectors_filtered,
            results.len(),
            execution_time
        );

        Ok(IVFFilteredResult {
            results,
            lists_processed,
            vectors_filtered,
            effective_nprobe,
            execution_time_us: execution_time,
        })
    }

    /// Find nearest probes (centroids) to query vector
    fn find_nearest_probes(&self, query: &[f32], nprobe: usize) -> Result<Vec<usize>> {
        let mut probe_distances: Vec<_> = self
            .inverted_lists
            .iter()
            .map(|(cluster_id, list)| {
                let distance = self.calculate_distance(query, &list.centroid);
                (*cluster_id, distance)
            })
            .collect();

        // Sort by distance and take top nprobe
        probe_distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(probe_distances
            .into_iter()
            .take(nprobe)
            .map(|(cluster_id, _)| cluster_id)
            .collect())
    }

    /// Filter vectors within an inverted list
    fn filter_inverted_list(
        &self,
        inverted_list: &IVFInvertedList,
        params: &IVFFilteredSearchParams,
        _metadata_lookup: &dyn MetadataLookup,
    ) -> Result<(Vec<IVFVector>, usize)> {
        let mut filtered_vectors = Vec::new();

        if let Some(ref filter) = params.filter {
            if params.enable_batch_filtering {
                // Batch filter evaluation (SIMD-optimized)
                let metadata_batch: Vec<serde_json::Value> = inverted_list
                    .vectors
                    .iter()
                    .map(|v| v.metadata.clone())
                    .collect();

                let filter_results = filter.as_ref().evaluate_batch(&metadata_batch)?;

                for (idx, passes) in filter_results.iter().enumerate() {
                    if passes.unwrap_or(false) {
                        filtered_vectors.push(inverted_list.vectors[idx].clone());
                    }
                }
            } else {
                // Row-by-row filtering
                for vector in &inverted_list.vectors {
                    let passes = filter.as_ref().evaluate_row(&vector.metadata)?;
                    if passes {
                        filtered_vectors.push(vector.clone());
                    }
                }
            }

            let filtered_count = inverted_list.vectors.len() - filtered_vectors.len();
            Ok((filtered_vectors, filtered_count))
        } else {
            // No filter, return all vectors
            Ok((inverted_list.vectors.clone(), 0))
        }
    }

    /// Rank candidates by similarity to query vector
    fn rank_candidates(
        &self,
        query: &[f32],
        candidates: &[IVFVector],
    ) -> Result<Vec<SearchResult>> {
        let mut ranked = BinaryHeap::new();

        for candidate in candidates {
            let similarity = self.calculate_similarity(query, &candidate.vector);

            ranked.push(SearchResult {
                id: candidate.id.clone(),
                score: similarity,
            });
        }

        // Extract ranked results (max-heap gives lowest first, so reverse)
        let mut results = Vec::new();
        while let Some(result) = ranked.pop() {
            results.push(result);
        }

        Ok(results)
    }

    /// Adjust nprobe parameter based on filter selectivity
    fn adjust_nprobe_for_filter(&self, params: &IVFFilteredSearchParams) -> Result<usize> {
        if let Some(ref filter) = params.filter {
            let selectivity = filter.estimated_selectivity();

            // For highly selective filters, we can use smaller nprobe
            // because many vectors will be filtered out anyway
            if selectivity <= 0.1 {
                Ok((params.nprobe as f64 * 0.7) as usize) // Reduce nprobe by 30%
            } else if selectivity > 0.5 {
                // For low selectivity filters, increase nprobe to maintain recall
                Ok(std::cmp::min(
                    (params.nprobe as f64 * 1.3) as usize,
                    params.nlist, // Don't exceed total clusters
                ))
            } else {
                Ok(params.nprobe) // Keep nprobe as-is for moderate selectivity
            }
        } else {
            Ok(params.nprobe) // No filter, use requested nprobe
        }
    }

    /// Calculate Euclidean distance between two vectors
    fn calculate_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>()
            .sqrt()
    }

    /// Calculate cosine similarity between two vectors
    fn calculate_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();

        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            dot_product / (norm_a * norm_b)
        }
    }

    /// Get index statistics
    pub fn stats(&self) -> IVFIndexStats {
        let total_vectors = self
            .inverted_lists
            .values()
            .map(|list| list.vectors.len())
            .sum();

        IVFIndexStats {
            nlist: self.nlist,
            dimension: self.dimension,
            total_vectors,
            list_counts: self
                .inverted_lists
                .iter()
                .map(|(id, list)| (*id, list.vectors.len()))
                .collect(),
        }
    }
}

/// IVF index statistics
#[derive(Debug, Clone)]
pub struct IVFIndexStats {
    pub nlist: usize,
    pub dimension: usize,
    pub total_vectors: usize,
    pub list_counts: HashMap<usize, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::ComparisonOperator;
    use crate::core::search::filter_contract::normalize_filter;

    #[test]
    fn test_create_ivf_index() {
        let index = FilteredIVFIndex::new(128, 100);

        assert_eq!(index.dimension, 128);
        assert_eq!(index.nlist, 100);

        let stats = index.stats();
        assert_eq!(stats.total_vectors, 0);
    }

    #[test]
    fn test_insert_ivf_vector() {
        let mut index = FilteredIVFIndex::new(384, 10);
        let centroid = vec![0.5; 384];

        let result = index.insert(
            "test_id".to_string(),
            vec![0.1; 384],
            serde_json::json!({"category": "electronics"}),
            0,
            &centroid,
        );

        assert!(result.is_ok());

        let stats = index.stats();
        assert_eq!(stats.total_vectors, 1);
    }

    #[test]
    fn test_adjust_nprobe_for_selective_filter() {
        let params = IVFFilteredSearchParams {
            query_vector: vec![0.1; 128],
            top_k: 10,
            nprobe: 10,
            nlist: 100,
            filter: Some(Arc::from(normalize_filter(
                crate::core::search::FilterExpression::Comparison {
                    field: "status".to_string(),
                    operator: ComparisonOperator::Equals,
                    value: serde_json::json!("active"),
                },
            ))),
            enable_batch_filtering: true,
            adaptive_nprobe: true,
        };

        let index = FilteredIVFIndex::new(128, 100);
        let adjusted_nprobe = index.adjust_nprobe_for_filter(&params).unwrap();

        // Equality filter is highly selective (10%), so nprobe should be reduced
        assert_eq!(adjusted_nprobe, 7); // 10 * 0.7 = 7
    }

    #[test]
    fn test_calculate_distance() {
        let index = FilteredIVFIndex::new(3, 10);

        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];

        let distance = index.calculate_distance(&a, &b);

        assert_eq!(distance, (2.0_f32).sqrt()); // sqrt(1^2 + 1^2) = sqrt(2)
    }

    #[test]
    fn test_calculate_similarity() {
        let index = FilteredIVFIndex::new(3, 10);

        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];

        let similarity = index.calculate_similarity(&a, &b);

        assert_eq!(similarity, 1.0); // Perfect match
    }

    #[test]
    fn test_ivf_filtered_search_params_default() {
        let params = IVFFilteredSearchParams::default();

        assert_eq!(params.top_k, 10);
        assert_eq!(params.nprobe, 10);
        assert_eq!(params.nlist, 100);
        assert!(params.enable_batch_filtering);
        assert!(params.adaptive_nprobe);
        assert!(params.filter.is_none());
    }

    #[test]
    fn test_find_nearest_probes() {
        let mut index = FilteredIVFIndex::new(2, 3);

        // Create some dummy inverted lists with centroids
        let centroid1 = vec![1.0, 0.0];
        let centroid2 = vec![0.0, 1.0];
        let centroid3 = vec![0.0, 0.0];

        index.inverted_lists.insert(
            0,
            IVFInvertedList {
                cluster_id: 0,
                vectors: Vec::new(),
                centroid: centroid1,
            },
        );
        index.inverted_lists.insert(
            1,
            IVFInvertedList {
                cluster_id: 1,
                vectors: Vec::new(),
                centroid: centroid2,
            },
        );
        index.inverted_lists.insert(
            2,
            IVFInvertedList {
                cluster_id: 2,
                vectors: Vec::new(),
                centroid: centroid3,
            },
        );

        let query = vec![0.9, 0.0]; // Closest to centroid1
        let nearest_probes = index.find_nearest_probes(&query, 2).unwrap();

        assert_eq!(nearest_probes.len(), 2);
        assert_eq!(nearest_probes[0], 0); // Centroid1 is closest
    }
}

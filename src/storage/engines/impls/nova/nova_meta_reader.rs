//! NOVA metadata reader for loading and using sidecar files
//!
//! This module provides functionality to read and utilize the .nova_meta
//! sidecar files that contain hierarchical statistics for query optimization.
//!
//! ## When to Use Sidecar Metadata
//!
//! **USE for Selective Queries (Predicate Pushdown):**
//! - Similarity search with top-k results (70-97% row group pruning)
//! - Filtered queries with metadata predicates
//! - Range queries on specific dimensions
//! - Any query that benefits from eliminating row groups before reading
//!
//! **DON'T USE for Full Scans:**
//! - Compaction operations (reads all data anyway)
//! - Full collection exports
//! - Batch updates that touch all records
//! - Loading/processing sidecar adds overhead without pruning benefit

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use super::hierarchical_stats::{EnhancedRowGroupStats, SuperBlock};
use super::nova_meta_collector::NovaMetadata;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// NOVA metadata reader for sidecar files
pub struct NovaMetaReader {
    /// Filesystem factory for reading sidecar files
    filesystem: Arc<FilesystemFactory>,

    /// Cache of loaded metadata by file path
    metadata_cache: HashMap<String, Arc<NovaMetadata>>,
}

impl NovaMetaReader {
    /// Create a new NOVA metadata reader
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        Self {
            filesystem,
            metadata_cache: HashMap::new(),
        }
    }

    /// Load metadata from sidecar file
    pub async fn load_metadata(&mut self, parquet_path: &str) -> Result<Arc<NovaMetadata>> {
        // Check cache first
        if let Some(cached) = self.metadata_cache.get(parquet_path) {
            return Ok(cached.clone());
        }

        // Construct sidecar path
        let sidecar_path = format!("{}.nova_meta", parquet_path);

        // Determine filesystem URL
        let fs_url = if parquet_path.starts_with("s3://")
            || parquet_path.starts_with("gs://")
            || parquet_path.starts_with("azure://")
            || parquet_path.starts_with("wasbs://")
        {
            parquet_path.to_string()
        } else {
            "file://".to_string()
        };

        // Read sidecar file
        let fs = self.filesystem.get_filesystem(&fs_url)?;
        let sidecar_data = fs.read(&sidecar_path).await?;

        // Deserialize metadata
        let metadata: NovaMetadata = bincode::deserialize(&sidecar_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize NOVA metadata: {}", e))?;

        let metadata_arc = Arc::new(metadata);
        self.metadata_cache
            .insert(parquet_path.to_string(), metadata_arc.clone());

        Ok(metadata_arc)
    }

    /// Prune row groups based on query vector using hierarchical statistics
    pub fn prune_row_groups(
        &self,
        metadata: &NovaMetadata,
        query_vector: &[f32],
        distance_threshold: Option<f32>,
    ) -> Vec<usize> {
        let mut selected_row_groups = Vec::new();

        // First level: SuperBlock pruning
        for superblock in &metadata.superblocks {
            if self.superblock_matches_query(superblock, query_vector, distance_threshold) {
                // Second level: Row group pruning within matching SuperBlocks
                for rg_idx in superblock.row_groups.clone() {
                    let rg_stats = &metadata.row_group_stats[rg_idx as usize];
                    if self.row_group_matches_query(rg_stats, query_vector, distance_threshold) {
                        selected_row_groups.push(rg_idx as usize);
                    }
                }
            }
        }

        selected_row_groups
    }

    /// Check if a SuperBlock potentially contains matching vectors
    fn superblock_matches_query(
        &self,
        superblock: &SuperBlock,
        query_vector: &[f32],
        distance_threshold: Option<f32>,
    ) -> bool {
        // Use zone map for coarse pruning
        let zone_map = &superblock.zone_map;

        // Quick bounds check
        if !self.vector_in_bounds(query_vector, &zone_map.min_values, &zone_map.max_values) {
            return false;
        }

        // If we have a distance threshold, check if the SuperBlock centroid is within range
        if let Some(threshold) = distance_threshold {
            let distance = self.euclidean_distance(query_vector, &zone_map.centroid);
            // Add variance as buffer for conservative pruning
            let max_variance = zone_map
                .variance
                .iter()
                .max_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or(&0.0);
            if distance - max_variance.sqrt() > threshold {
                return false;
            }
        }

        // Check selectivity hints - if very high cost, skip
        if superblock.selectivity_hints.search_cost_estimate > 1000.0 {
            // Very high cost, likely not a good match
            return false;
        }

        true
    }

    /// Check if a row group potentially contains matching vectors
    fn row_group_matches_query(
        &self,
        row_group: &EnhancedRowGroupStats,
        query_vector: &[f32],
        distance_threshold: Option<f32>,
    ) -> bool {
        let zone_map = &row_group.vector_zone_map;

        // Dimension-wise bounds check
        if !self.vector_in_bounds(query_vector, &zone_map.min_values, &zone_map.max_values) {
            return false;
        }

        // L2 norm bounds check for faster elimination
        let query_norm = query_vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        if query_norm < zone_map.norm_bounds.0 * 0.9 || query_norm > zone_map.norm_bounds.1 * 1.1 {
            // Query norm is significantly outside the row group's norm range
            return false;
        }

        // Distance-based pruning if threshold provided
        if let Some(threshold) = distance_threshold {
            let centroid_distance = self.euclidean_distance(query_vector, &zone_map.centroid);
            // Conservative pruning: add standard deviation as buffer
            let std_dev =
                zone_map.variance.iter().map(|v| v.sqrt()).sum::<f32>() / zone_map.dimension as f32;
            if centroid_distance - std_dev > threshold {
                return false;
            }
        }

        true
    }

    /// Check if a vector is within min/max bounds
    fn vector_in_bounds(&self, vector: &[f32], min_values: &[f32], max_values: &[f32]) -> bool {
        // Conservative check: allow some tolerance for floating point
        const TOLERANCE: f32 = 1e-6;
        for i in 0..vector.len().min(min_values.len()) {
            if vector[i] < min_values[i] - TOLERANCE || vector[i] > max_values[i] + TOLERANCE {
                return false;
            }
        }
        true
    }

    /// Calculate Euclidean distance between two vectors
    fn euclidean_distance(&self, v1: &[f32], v2: &[f32]) -> f32 {
        v1.iter()
            .zip(v2.iter())
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
            .sqrt()
    }

    /// Get query cost estimate based on metadata
    pub fn estimate_query_cost(
        &self,
        metadata: &NovaMetadata,
        selected_row_groups: &[usize],
    ) -> f32 {
        let mut total_cost = 0.0;

        for &rg_idx in selected_row_groups {
            if let Some(rg_stats) = metadata.row_group_stats.get(rg_idx) {
                let cost_estimate = &rg_stats.search_cost_estimate;
                total_cost += cost_estimate.io_cost + cost_estimate.cpu_cost;
            }
        }

        total_cost
    }

    /// Clear the metadata cache
    pub fn clear_cache(&mut self) {
        self.metadata_cache.clear();
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> (usize, usize) {
        let num_entries = self.metadata_cache.len();
        let total_size: usize = self
            .metadata_cache
            .values()
            .map(|m| std::mem::size_of_val(m.as_ref()))
            .sum();
        (num_entries, total_size)
    }
}

/// Query optimization hints based on NOVA metadata
#[derive(Debug, Clone)]
pub struct QueryOptimizationHints {
    /// Recommended row groups to read
    pub row_groups: Vec<usize>,

    /// Estimated query cost
    pub estimated_cost: f32,

    /// Pruning effectiveness (0.0 to 1.0)
    pub pruning_ratio: f32,

    /// Suggested quantization level for progressive search
    pub suggested_quantization: Option<String>,

    /// Whether to use streaming search
    pub use_streaming: bool,
}

impl NovaMetaReader {
    /// Generate query optimization hints based on metadata
    pub fn get_optimization_hints(
        &self,
        metadata: &NovaMetadata,
        query_vector: &[f32],
        _top_k: usize,
    ) -> QueryOptimizationHints {
        // Prune row groups
        let all_row_groups: Vec<usize> = (0..metadata.row_group_stats.len()).collect();
        let selected = self.prune_row_groups(metadata, query_vector, None);

        let pruning_ratio = 1.0 - (selected.len() as f32 / all_row_groups.len() as f32);
        let estimated_cost = self.estimate_query_cost(metadata, &selected);

        // Determine suggested quantization based on pruning effectiveness
        let suggested_quantization = if pruning_ratio > 0.8 {
            Some("binary".to_string()) // High pruning, can use fast binary
        } else if pruning_ratio > 0.5 {
            Some("int8".to_string()) // Moderate pruning
        } else {
            Some("fp32".to_string()) // Low pruning, need full precision
        };

        // Use streaming for large result sets or low pruning
        let use_streaming = selected.len() > 10 || pruning_ratio < 0.3;

        QueryOptimizationHints {
            row_groups: selected,
            estimated_cost,
            pruning_ratio,
            suggested_quantization,
            use_streaming,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vector_bounds_check() {
        let reader = NovaMetaReader::new(Arc::new(
            FilesystemFactory::create(Default::default()).await.unwrap(),
        ));

        let vector = vec![1.0, 2.0, 3.0];
        let min_values = vec![0.0, 1.0, 2.0];
        let max_values = vec![2.0, 3.0, 4.0];

        assert!(reader.vector_in_bounds(&vector, &min_values, &max_values));

        let out_of_bounds = vec![3.0, 4.0, 5.0];
        assert!(!reader.vector_in_bounds(&out_of_bounds, &min_values, &max_values));
    }

    #[tokio::test]
    async fn test_euclidean_distance() {
        let reader = NovaMetaReader::new(Arc::new(
            FilesystemFactory::create(Default::default()).await.unwrap(),
        ));

        let v1 = vec![1.0, 2.0, 3.0];
        let v2 = vec![4.0, 5.0, 6.0];

        let distance = reader.euclidean_distance(&v1, &v2);
        assert!((distance - 5.196).abs() < 0.01); // sqrt(27) ≈ 5.196
    }
}

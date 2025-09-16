//! Zone maps for dimension-level pruning in HELIX
//!
//! This module implements zone maps that track min/max values per dimension
//! to enable fine-grained pruning during query execution.

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

use crate::core::VectorRecord;

/// Zone map for a block of vectors
#[derive(Debug, Clone)]
pub struct ZoneMap {
    /// Block identifier
    pub block_id: u32,
    /// Minimum values per dimension
    pub dim_min: Vec<f32>,
    /// Maximum values per dimension
    pub dim_max: Vec<f32>,
    /// Number of vectors in block
    pub vector_count: usize,
    /// Null count per dimension (for sparse vectors)
    pub null_counts: Option<Vec<u32>>,
    /// Bloom filter for vector IDs in block
    pub id_bloom: Option<Vec<u8>>,
    /// Statistics per dimension
    pub dim_stats: Option<DimensionStatistics>,
}

/// Detailed statistics per dimension
#[derive(Debug, Clone)]
pub struct DimensionStatistics {
    /// Mean value per dimension
    pub mean: Vec<f32>,
    /// Standard deviation per dimension
    pub std_dev: Vec<f32>,
    /// Cardinality estimate per dimension
    pub cardinality: Vec<u32>,
    /// Skewness per dimension
    pub skewness: Vec<f32>,
}

impl ZoneMap {
    /// Create zone map from a block of vectors
    pub fn from_vectors(block_id: u32, vectors: &[VectorRecord]) -> Result<Self> {
        if vectors.is_empty() {
            anyhow::bail!("Cannot create zone map from empty vectors");
        }

        let dimensions = vectors[0].vector.len();
        let mut dim_min = vec![f32::INFINITY; dimensions];
        let mut dim_max = vec![f32::NEG_INFINITY; dimensions];
        let mut dim_sum = vec![0.0; dimensions];
        let mut dim_sum_sq = vec![0.0; dimensions];

        // Calculate min/max and sums for statistics
        for record in vectors {
            for (i, &value) in record.vector.iter().enumerate() {
                dim_min[i] = dim_min[i].min(value);
                dim_max[i] = dim_max[i].max(value);
                dim_sum[i] += value;
                dim_sum_sq[i] += value * value;
            }
        }

        // Calculate statistics
        let n = vectors.len() as f32;
        let mut mean = vec![0.0; dimensions];
        let mut std_dev = vec![0.0; dimensions];

        for i in 0..dimensions {
            mean[i] = dim_sum[i] / n;
            let variance = (dim_sum_sq[i] / n) - (mean[i] * mean[i]);
            std_dev[i] = variance.max(0.0).sqrt();
        }

        // Create bloom filter for IDs
        let id_bloom = Self::create_id_bloom(vectors);

        Ok(Self {
            block_id,
            dim_min,
            dim_max,
            vector_count: vectors.len(),
            null_counts: None,
            id_bloom: Some(id_bloom),
            dim_stats: Some(DimensionStatistics {
                mean,
                std_dev,
                cardinality: vec![0; dimensions], // Placeholder
                skewness: vec![0.0; dimensions],  // Placeholder
            }),
        })
    }

    /// Create bloom filter for vector IDs
    fn create_id_bloom(vectors: &[VectorRecord]) -> Vec<u8> {
        use crate::core::bloom::{BloomFilterConfig, factory::BloomFilterFactory};

        let config = BloomFilterConfig {
            expected_items: vectors.len(),
            false_positive_rate: Some(0.01),
            ..Default::default()
        };

        let mut bloom = BloomFilterFactory::create(&config);
        for record in vectors {
            bloom.insert(record.id.as_bytes());
        }

        bloom.serialize().unwrap_or_default()
    }

    /// Check if a query vector might be in this block
    pub fn might_contain(&self, query_vector: &[f32]) -> bool {
        if query_vector.len() != self.dim_min.len() {
            return false;
        }

        // Check if query falls within min/max bounds
        for i in 0..query_vector.len() {
            if query_vector[i] < self.dim_min[i] || query_vector[i] > self.dim_max[i] {
                // Query is outside bounds in this dimension
                // But we can't definitively exclude the block
                // as nearest neighbors might still be here
            }
        }

        true // Conservative: include unless certain to exclude
    }

    /// Calculate pruning score for a query
    pub fn pruning_score(&self, query_vector: &[f32], radius: f32) -> f32 {
        if query_vector.len() != self.dim_min.len() {
            return f32::INFINITY;
        }

        let mut min_distance = 0.0;

        // Calculate minimum possible distance to block
        for i in 0..query_vector.len() {
            let q = query_vector[i];
            let min = self.dim_min[i];
            let max = self.dim_max[i];

            if q < min {
                min_distance += (min - q).powi(2);
            } else if q > max {
                min_distance += (q - max).powi(2);
            }
            // If q is within [min, max], contributes 0 to min distance
        }

        min_distance.sqrt()
    }

    /// Estimate selectivity for a range query
    pub fn estimate_selectivity(&self, min_bounds: &[f32], max_bounds: &[f32]) -> f32 {
        if min_bounds.len() != self.dim_min.len() || max_bounds.len() != self.dim_max.len() {
            return 0.0;
        }

        let mut selectivity = 1.0;

        for i in 0..self.dim_min.len() {
            let block_min = self.dim_min[i];
            let block_max = self.dim_max[i];
            let query_min = min_bounds[i];
            let query_max = max_bounds[i];

            // Calculate overlap
            let overlap_min = block_min.max(query_min);
            let overlap_max = block_max.min(query_max);

            if overlap_min > overlap_max {
                return 0.0; // No overlap in this dimension
            }

            // Estimate selectivity for this dimension
            let block_range = block_max - block_min;
            let overlap_range = overlap_max - overlap_min;

            if block_range > 0.0 {
                selectivity *= overlap_range / block_range;
            }
        }

        selectivity
    }
}

/// Zone map index for efficient pruning
#[derive(Debug, Default)]
pub struct ZoneMapIndex {
    /// Zone maps by block ID
    pub maps: HashMap<u32, ZoneMap>,
    /// Global min/max across all blocks
    pub global_min: Vec<f32>,
    pub global_max: Vec<f32>,
    /// Total vectors indexed
    pub total_vectors: usize,
}

impl ZoneMapIndex {
    /// Add a zone map to the index
    pub fn add_zone_map(&mut self, zone_map: ZoneMap) {
        // Update global bounds
        if self.global_min.is_empty() {
            self.global_min = zone_map.dim_min.clone();
            self.global_max = zone_map.dim_max.clone();
        } else {
            for i in 0..zone_map.dim_min.len() {
                self.global_min[i] = self.global_min[i].min(zone_map.dim_min[i]);
                self.global_max[i] = self.global_max[i].max(zone_map.dim_max[i]);
            }
        }

        self.total_vectors += zone_map.vector_count;
        self.maps.insert(zone_map.block_id, zone_map);
    }

    /// Prune blocks based on query vector
    pub fn prune_blocks(&self, query_vector: &[f32], k: usize) -> Vec<u32> {
        // Calculate pruning scores for all blocks
        let mut block_scores: Vec<(u32, f32)> = self
            .maps
            .iter()
            .map(|(id, map)| {
                let score = map.pruning_score(query_vector, f32::INFINITY);
                (*id, score)
            })
            .collect();

        // Sort by score (lower is better)
        block_scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        // Select blocks likely to contain top-k results
        let blocks_to_scan = (k as f32 * 1.5).ceil() as usize; // Scan 1.5x blocks
        block_scores
            .into_iter()
            .take(blocks_to_scan)
            .map(|(id, _)| id)
            .collect()
    }

    /// Estimate query selectivity
    pub fn estimate_query_selectivity(&self, query_vector: &[f32], radius: f32) -> f32 {
        let mut selected_vectors = 0;

        for zone_map in self.maps.values() {
            let distance = zone_map.pruning_score(query_vector, radius);
            if distance <= radius {
                selected_vectors += zone_map.vector_count;
            }
        }

        selected_vectors as f32 / self.total_vectors.max(1) as f32
    }

    /// Get dimension-wise statistics
    pub fn get_dimension_stats(&self) -> DimensionSummary {
        let dimensions = self.global_min.len();
        let mut summary = DimensionSummary {
            dimensions,
            range_per_dim: vec![0.0; dimensions],
            avg_selectivity: vec![0.0; dimensions],
            cardinality_estimate: vec![0; dimensions],
        };

        for i in 0..dimensions {
            summary.range_per_dim[i] = self.global_max[i] - self.global_min[i];

            // Estimate cardinality from zone maps
            let unique_values: std::collections::HashSet<u32> = self
                .maps
                .values()
                .flat_map(|map| {
                    vec![
                        (map.dim_min[i] * 1000.0) as u32,
                        (map.dim_max[i] * 1000.0) as u32,
                    ]
                })
                .collect();
            summary.cardinality_estimate[i] = unique_values.len() as u32;
        }

        summary
    }
}

/// Summary statistics per dimension
#[derive(Debug)]
pub struct DimensionSummary {
    pub dimensions: usize,
    pub range_per_dim: Vec<f32>,
    pub avg_selectivity: Vec<f32>,
    pub cardinality_estimate: Vec<u32>,
}

/// Zone map builder for creating zone maps during flush/compaction
pub struct ZoneMapBuilder {
    block_size: usize,
    current_block: Vec<VectorRecord>,
    current_block_id: u32,
    zone_maps: Vec<ZoneMap>,
}

impl ZoneMapBuilder {
    pub fn new(block_size: usize) -> Self {
        Self {
            block_size,
            current_block: Vec::new(),
            current_block_id: 0,
            zone_maps: Vec::new(),
        }
    }

    /// Add a vector to the builder
    pub fn add_vector(&mut self, record: VectorRecord) -> Result<()> {
        self.current_block.push(record);

        if self.current_block.len() >= self.block_size {
            self.finalize_block()?;
        }

        Ok(())
    }

    /// Finalize current block and create zone map
    fn finalize_block(&mut self) -> Result<()> {
        if !self.current_block.is_empty() {
            let zone_map = ZoneMap::from_vectors(self.current_block_id, &self.current_block)?;
            self.zone_maps.push(zone_map);
            self.current_block.clear();
            self.current_block_id += 1;
        }
        Ok(())
    }

    /// Build final zone map index
    pub fn build(mut self) -> Result<ZoneMapIndex> {
        // Finalize any remaining vectors
        self.finalize_block()?;

        let mut index = ZoneMapIndex::default();
        for zone_map in self.zone_maps {
            index.add_zone_map(zone_map);
        }

        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zone_map_creation() {
        let vectors = vec![
            VectorRecord {
                id: "v1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: None,
                timestamp: 0,
                expires_at: None,
            },
            VectorRecord {
                id: "v2".to_string(),
                vector: vec![4.0, 5.0, 6.0],
                metadata: None,
                timestamp: 0,
                expires_at: None,
            },
        ];

        let zone_map = ZoneMap::from_vectors(0, &vectors).unwrap();

        assert_eq!(zone_map.dim_min, vec![1.0, 2.0, 3.0]);
        assert_eq!(zone_map.dim_max, vec![4.0, 5.0, 6.0]);
        assert_eq!(zone_map.vector_count, 2);
    }

    #[test]
    fn test_pruning_score() {
        let zone_map = ZoneMap {
            block_id: 0,
            dim_min: vec![0.0, 0.0],
            dim_max: vec![10.0, 10.0],
            vector_count: 100,
            null_counts: None,
            id_bloom: None,
            dim_stats: None,
        };

        // Query inside bounds
        let score1 = zone_map.pruning_score(&[5.0, 5.0], 10.0);
        assert_eq!(score1, 0.0);

        // Query outside bounds
        let score2 = zone_map.pruning_score(&[15.0, 15.0], 10.0);
        assert!(score2 > 0.0);
    }

    #[test]
    fn test_zone_map_builder() {
        let mut builder = ZoneMapBuilder::new(2);

        for i in 0..5 {
            builder
                .add_vector(VectorRecord {
                    id: format!("v{}", i),
                    vector: vec![i as f32, i as f32 * 2.0],
                    metadata: None,
                    timestamp: 0,
                    expires_at: None,
                })
                .unwrap();
        }

        let index = builder.build().unwrap();
        assert_eq!(index.maps.len(), 3); // 5 vectors with block size 2 = 3 blocks
        assert_eq!(index.total_vectors, 5);
    }
}

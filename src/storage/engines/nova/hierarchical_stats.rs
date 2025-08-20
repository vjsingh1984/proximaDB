// Hierarchical statistics for NOVA engine optimization
// Implements SuperBlock and enhanced row group statistics for efficient pruning

use anyhow::Result;
use parquet::file::metadata::{RowGroupMetaData, ColumnChunkMetaData};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use crate::compute::distance_computation::DistanceMetric;

/// SuperBlock: Aggregate of multiple row groups for coarse-grained pruning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperBlock {
    /// SuperBlock identifier
    pub id: u32,
    
    /// Range of row group indices (e.g., 0-9, 10-19)
    pub row_groups: Range<u32>,
    
    /// Total vectors across all row groups in this SuperBlock
    pub vector_count: u64,
    
    /// Aggregate zone map across all row groups
    pub zone_map: ZoneMap,
    
    /// Quantization statistics for optimization
    pub quantization_stats: QuantizationStats,
    
    /// Selectivity hints for cost-based optimization
    pub selectivity_hints: SelectivityHints,
    
    /// Storage statistics
    pub storage_stats: StorageStats,
    
    /// Last updated timestamp
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

/// Enhanced row group statistics with vector-specific optimizations
#[derive(Debug, Clone)]
pub struct EnhancedRowGroupStats {
    /// Row group index
    pub row_group_id: u32,
    
    /// Native Parquet metadata
    pub parquet_metadata: Option<RowGroupMetaData>,
    
    /// Vector-specific zone map
    pub vector_zone_map: ZoneMap,
    
    /// Quantized column selectivity
    pub quantized_selectivity: QuantizedSelectivity,
    
    /// Compression effectiveness
    pub compression_ratio: f32,
    
    /// Search cost estimates
    pub search_cost_estimate: SearchCostEstimate,
    
    /// Access patterns and usage statistics
    pub access_stats: AccessStats,
}

/// Zone map for efficient dimensional pruning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneMap {
    /// Minimum values per dimension
    pub min_values: Vec<f32>,
    
    /// Maximum values per dimension
    pub max_values: Vec<f32>,
    
    /// Centroid (average) vector
    pub centroid: Vec<f32>,
    
    /// Variance per dimension
    pub variance: Vec<f32>,
    
    /// L2 norm bounds
    pub norm_bounds: (f32, f32),
    
    /// Dimension count
    pub dimension: usize,
}

/// Quantization statistics for progressive search optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationStats {
    /// Binary quantization effectiveness
    pub binary_selectivity: f32,
    
    /// INT8 quantization quality
    pub int8_reconstruction_error: f32,
    
    /// PQ quantization effectiveness
    pub pq_selectivity: f32,
    
    /// Overall compression ratio
    pub compression_ratio: f32,
    
    /// Quantization overhead (ms)
    pub quantization_overhead_ms: u64,
}

/// Selectivity hints for cost-based optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectivityHints {
    /// Expected candidate reduction at binary stage
    pub binary_reduction_factor: f32,
    
    /// Expected candidate reduction at INT8 stage
    pub int8_reduction_factor: f32,
    
    /// Expected candidate reduction at PQ stage
    pub pq_reduction_factor: f32,
    
    /// Estimated search cost (relative units)
    pub search_cost_estimate: f32,
    
    /// Memory requirement estimate (bytes)
    pub memory_requirement: usize,
}

/// Storage-level statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    /// Total compressed size
    pub compressed_size: u64,
    
    /// Total uncompressed size
    pub uncompressed_size: u64,
    
    /// Number of pages across all row groups
    pub total_pages: u32,
    
    /// Average page size
    pub avg_page_size: u32,
    
    /// Bloom filter sizes
    pub bloom_filter_size: u32,
}

/// Quantized column selectivity metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizedSelectivity {
    /// Binary column filtering effectiveness (0.0-1.0)
    pub binary_effectiveness: f32,
    
    /// INT8 column accuracy vs full precision
    pub int8_accuracy: f32,
    
    /// PQ column quality metrics
    pub pq_quality: f32,
    
    /// Progressive search efficiency
    pub progressive_efficiency: f32,
}

/// Search cost estimation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCostEstimate {
    /// I/O cost (relative units)
    pub io_cost: f32,
    
    /// CPU cost (relative units)
    pub cpu_cost: f32,
    
    /// Memory cost (relative units)
    pub memory_cost: f32,
    
    /// Estimated latency (milliseconds)
    pub estimated_latency_ms: f32,
    
    /// Confidence in estimate (0.0-1.0)
    pub confidence: f32,
}

/// Access patterns and usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessStats {
    /// Number of times accessed
    pub access_count: u64,
    
    /// Last access time
    pub last_access: chrono::DateTime<chrono::Utc>,
    
    /// Average query selectivity
    pub avg_selectivity: f32,
    
    /// Cache hit rate
    pub cache_hit_rate: f32,
    
    /// Access frequency (accesses per hour)
    pub access_frequency: f32,
}

impl SuperBlock {
    /// Create a new SuperBlock from row group statistics
    pub fn new(
        id: u32,
        row_groups: Range<u32>,
        enhanced_stats: &[EnhancedRowGroupStats],
    ) -> Result<Self> {
        if enhanced_stats.is_empty() {
            return Err(anyhow::anyhow!("Cannot create SuperBlock from empty row group stats"));
        }
        
        // Aggregate vector count
        let vector_count = enhanced_stats.iter()
            .map(|stats| stats.parquet_metadata.as_ref()
                .map(|md| md.num_rows() as u64)
                )
            .sum();
        
        // Create aggregate zone map
        let zone_map = Self::aggregate_zone_maps(enhanced_stats)?;
        
        // Aggregate quantization stats
        let quantization_stats = Self::aggregate_quantization_stats(enhanced_stats);
        
        // Calculate selectivity hints
        let selectivity_hints = Self::calculate_selectivity_hints(enhanced_stats);
        
        // Aggregate storage stats
        let storage_stats = Self::aggregate_storage_stats(enhanced_stats);
        
        Ok(Self {
            id,
            row_groups,
            vector_count,
            zone_map,
            quantization_stats,
            selectivity_hints,
            storage_stats,
            last_updated: chrono::Utc::now(),
        })
    }
    
    /// Check if a query vector might have candidates in this SuperBlock
    pub fn can_contain_candidates(
        &self,
        query: &[f32],
        distance_metric: DistanceMetric,
        max_similarity: f32,
    ) -> bool {
        // Use zone map for quick pruning
        self.zone_map.intersects_query(query, distance_metric, max_similarity)
    }
    
    /// Estimate search cost for this SuperBlock
    pub fn estimate_search_cost(&self, query_selectivity: f32) -> f32 {
        let base_cost = self.selectivity_hints.search_cost_estimate;
        let selectivity_adjustment = query_selectivity * self.selectivity_hints.binary_reduction_factor;
        base_cost * selectivity_adjustment
    }
    
    /// Get ordered row groups by estimated search cost
    pub fn get_ordered_row_groups(&self, enhanced_stats: &[EnhancedRowGroupStats]) -> Vec<u32> {
        let mut row_group_costs: Vec<(u32, f32)> = enhanced_stats.iter()
            .filter(|stats| self.row_groups.contains(&stats.row_group_id))
            .map(|stats| (stats.row_group_id, stats.search_cost_estimate.estimated_latency_ms))
            .collect();
        
        // Sort by cost (ascending)
        row_group_costs.sort_by(|a, b| a.1.partial_cmp(&b.1));
        
        row_group_costs.into_iter().map(|(id, _)| id).collect()
    }
    
    fn aggregate_zone_maps(stats: &[EnhancedRowGroupStats]) -> Result<ZoneMap> {
        if stats.is_empty() {
            return Err(anyhow::anyhow!("Cannot aggregate empty zone maps"));
        }
        
        let dimension = stats[0].vector_zone_map.dimension;
        let mut min_values = vec![f32::INFINITY; dimension];
        let mut max_values = vec![f32::NEG_INFINITY; dimension];
        let mut centroid = vec![0.0; dimension];
        let mut variance = vec![0.0; dimension];
        let mut min_norm = f32::INFINITY;
        let mut max_norm = f32::NEG_INFINITY;
        
        let count = stats.len() as f32;
        
        for stat in stats {
            let zone_map = &stat.vector_zone_map;
            
            // Aggregate min/max values
            for i in 0..dimension {
                min_values[i] = min_values[i].min(zone_map.min_values[i]);
                max_values[i] = max_values[i].max(zone_map.max_values[i]);
                centroid[i] += zone_map.centroid[i] / count;
                variance[i] += zone_map.variance[i] / count;
            }
            
            // Aggregate norm bounds
            min_norm = min_norm.min(zone_map.norm_bounds.0);
            max_norm = max_norm.max(zone_map.norm_bounds.1);
        }
        
        Ok(ZoneMap {
            min_values,
            max_values,
            centroid,
            variance,
            norm_bounds: (min_norm, max_norm),
            dimension,
        })
    }
    
    fn aggregate_quantization_stats(stats: &[EnhancedRowGroupStats]) -> QuantizationStats {
        let count = stats.len() as f32;
        let mut agg = QuantizationStats {
            binary_selectivity: 0.0,
            int8_reconstruction_error: 0.0,
            pq_selectivity: 0.0,
            compression_ratio: 0.0,
            quantization_overhead_ms: 0,
        };
        
        for stat in stats {
            agg.binary_selectivity += stat.quantized_selectivity.binary_effectiveness / count;
            agg.int8_reconstruction_error += stat.quantized_selectivity.int8_accuracy / count;
            agg.pq_selectivity += stat.quantized_selectivity.pq_quality / count;
            agg.compression_ratio += stat.compression_ratio / count;
        }
        
        agg
    }
    
    fn calculate_selectivity_hints(stats: &[EnhancedRowGroupStats]) -> SelectivityHints {
        let count = stats.len() as f32;
        let mut hints = SelectivityHints {
            binary_reduction_factor: 0.0,
            int8_reduction_factor: 0.0,
            pq_reduction_factor: 0.0,
            search_cost_estimate: 0.0,
            memory_requirement: 0,
        };
        
        for stat in stats {
            hints.binary_reduction_factor += stat.quantized_selectivity.binary_effectiveness / count;
            hints.int8_reduction_factor += stat.quantized_selectivity.int8_accuracy / count;
            hints.pq_reduction_factor += stat.quantized_selectivity.pq_quality / count;
            hints.search_cost_estimate += stat.search_cost_estimate.estimated_latency_ms / count;
        }
        
        // Estimate memory requirement (rough calculation)
        hints.memory_requirement = stats.iter()
            .map(|s| s.parquet_metadata.as_ref()
                .map(|md| md.total_byte_size() as usize)
                )
            .sum();
        
        hints
    }
    
    fn aggregate_storage_stats(stats: &[EnhancedRowGroupStats]) -> StorageStats {
        let mut storage = StorageStats {
            compressed_size: 0,
            uncompressed_size: 0,
            total_pages: 0,
            avg_page_size: 0,
            bloom_filter_size: 0,
        };
        
        let mut total_page_size = 0u64;
        
        for stat in stats {
            if let Some(metadata) = &stat.parquet_metadata {
                storage.compressed_size += metadata.compressed_size() as u64;
                storage.uncompressed_size += metadata.total_byte_size() as u64;
                
                // Count pages across all columns
                for column in metadata.columns() {
                    // Estimate pages per column (simplified)
                    let estimated_pages = (column.uncompressed_size() / 1024 / 1024).max(1); // 1MB per page
                    storage.total_pages += estimated_pages as u32;
                    total_page_size += column.uncompressed_size() as u64;
                }
            }
        }
        
        // Calculate average page size
        if storage.total_pages > 0 {
            storage.avg_page_size = (total_page_size / storage.total_pages as u64) as u32;
        }
        
        storage
    }
}

impl ZoneMap {
    /// Create a zone map from a set of vectors
    pub fn from_vectors(vectors: &[Vec<f32>]) -> Result<Self> {
        if vectors.is_empty() {
            return Err(anyhow::anyhow!("Cannot create zone map from empty vectors"));
        }
        
        let dimension = vectors[0].len();
        let mut min_values = vec![f32::INFINITY; dimension];
        let mut max_values = vec![f32::NEG_INFINITY; dimension];
        let mut centroid = vec![0.0; dimension];
        let mut variance = vec![0.0; dimension];
        let mut min_norm = f32::INFINITY;
        let mut max_norm = f32::NEG_INFINITY;
        
        let count = vectors.len() as f32;
        
        // First pass: calculate min, max, centroid, and norms
        for vector in vectors {
            let mut norm_sq = 0.0;
            
            for (i, &value) in vector.iter().enumerate() {
                min_values[i] = min_values[i].min(value);
                max_values[i] = max_values[i].max(value);
                centroid[i] += value / count;
                norm_sq += value * value;
            }
            
            let norm = norm_sq.sqrt();
            min_norm = min_norm.min(norm);
            max_norm = max_norm.max(norm);
        }
        
        // Second pass: calculate variance
        for vector in vectors {
            for (i, &value) in vector.iter().enumerate() {
                let diff = value - centroid[i];
                variance[i] += (diff * diff) / count;
            }
        }
        
        Ok(Self {
            min_values,
            max_values,
            centroid,
            variance,
            norm_bounds: (min_norm, max_norm),
            dimension,
        })
    }
    
    /// Check if this zone map might intersect with a query
    pub fn intersects_query(
        &self,
        query: &[f32],
        distance_metric: DistanceMetric,
        max_similarity: f32,
    ) -> bool {
        match distance_metric {
            DistanceMetric::Euclidean => self.intersects_euclidean(query, max_similarity),
            DistanceMetric::Cosine => self.intersects_cosine(query, max_similarity),
            DistanceMetric::DotProduct => self.intersects_dot_product(query, max_similarity),
            _ => true, // Conservative: assume intersection for unknown metrics
        }
    }
    
    fn intersects_euclidean(&self, query: &[f32], max_similarity: f32) -> bool {
        let mut min_distance_sq = 0.0;
        
        for (i, &q) in query.iter().enumerate() {
            if i >= self.dimension {
                break;
            }
            
            let min_val = self.min_values[i];
            let max_val = self.max_values[i];
            
            if q < min_val {
                let diff = min_val - q;
                min_distance_sq += diff * diff;
            } else if q > max_val {
                let diff = q - max_val;
                min_distance_sq += diff * diff;
            }
            // If q is within [min_val, max_val], it contributes 0 to min distance
        }
        
        min_distance_sq.sqrt() <= max_similarity
    }
    
    fn intersects_cosine(&self, query: &[f32], max_similarity: f32) -> bool {
        // For cosine distance, we need to be more conservative
        // Use norm bounds as a rough approximation
        let query_norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        
        // Very rough approximation - could be improved with tighter bounds
        let min_possible_cosine = 1.0 - max_similarity;
        let max_possible_dot = query_norm * self.norm_bounds.1;
        let min_possible_dot = query_norm * self.norm_bounds.0;
        
        // Conservative check
        max_possible_dot >= min_possible_cosine * query_norm * self.norm_bounds.0
    }
    
    fn intersects_dot_product(&self, query: &[f32], max_similarity: f32) -> bool {
        // For dot product (as distance, so negative dot product)
        // We want to check if any vector in this zone could have dot product >= -max_distance
        
        let mut max_possible_dot = 0.0;
        
        for (i, &q) in query.iter().enumerate() {
            if i >= self.dimension {
                break;
            }
            
            let min_val = self.min_values[i];
            let max_val = self.max_values[i];
            
            if q > 0.0 {
                max_possible_dot += q * max_val;
            } else {
                max_possible_dot += q * min_val;
            }
        }
        
        -max_possible_dot <= max_similarity
    }
}

impl Default for QuantizationStats {
    fn default() -> Self {
        Self {
            binary_selectivity: 0.5,
            int8_reconstruction_error: 0.1,
            pq_selectivity: 0.8,
            compression_ratio: 4.0,
            quantization_overhead_ms: 10,
        }
    }
}

impl Default for SelectivityHints {
    fn default() -> Self {
        Self {
            binary_reduction_factor: 0.9,
            int8_reduction_factor: 0.7,
            pq_reduction_factor: 0.5,
            search_cost_estimate: 100.0,
            memory_requirement: 64 * 1024 * 1024, // 64MB default
        }
    }
}

/// Basic zone maps for simplified NOVA design (optimized version)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasicZoneMaps {
    /// Per-dimension range statistics  
    pub dimension_ranges: Vec<DimensionRange>,
    
    /// Total vectors covered by these zone maps
    pub total_vectors: u64,
    
    /// When these zone maps were created
    pub creation_time: chrono::DateTime<chrono::Utc>,
}

/// Range information for a single dimension
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionRange {
    /// Dimension index
    pub dimension_index: usize,
    
    /// Minimum value in this dimension
    pub min_value: f32,
    
    /// Maximum value in this dimension  
    pub max_value: f32,
    
    /// Selectivity (0.0-1.0) for this dimension
    pub selectivity: f32,
}

/// Simplified enhanced row group stats for the optimized NOVA design
impl EnhancedRowGroupStats {
    /// Create basic enhanced stats with simplified fields (optimized design)
    pub fn create_basic(
        row_group_id: u32,
        vector_count: u64,
        dimension: usize,
        min_values: Vec<f32>,
        max_values: Vec<f32>,
        centroid: Vec<f32>,
        null_counts: Vec<u64>,
        estimated_selectivity: f32,
        compression_ratio: f32,
        access_frequency: u64,
    ) -> Self {
        Self {
            row_group_id,
            parquet_metadata: None,
            vector_zone_map: ZoneMap {
                min_values,
                max_values,
                centroid,
                variance: vec![0.0; dimension], // Simplified - not computed in basic version
                norm_bounds: (0.0, 0.0), // Simplified - not computed in basic version
                dimension,
            },
            quantized_selectivity: QuantizedSelectivity {
                binary_effectiveness: estimated_selectivity * 0.8,
                int8_accuracy: estimated_selectivity * 0.9,
                pq_quality: estimated_selectivity * 0.85,
                progressive_efficiency: estimated_selectivity * 0.75,
            },
            compression_ratio,
            search_cost_estimate: SearchCostEstimate {
                io_cost: vector_count as f32 * 0.1,
                cpu_cost: vector_count as f32 * 0.05,
                memory_cost: vector_count as f32 * 0.02,
                estimated_latency_ms: vector_count as f32 * 0.001, // 1μs per vector estimate
                confidence: 0.7, // Medium confidence for basic stats
            },
            access_stats: AccessStats {
                access_count: 0,
                last_access: chrono::Utc::now(),
                avg_selectivity: estimated_selectivity,
                cache_hit_rate: 0.0,
                access_frequency: access_frequency as f32,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_zone_map_creation() {
        let vectors = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
            vec![7.0, 8.0, 9.0],
        ];
        
        let zone_map = ZoneMap::from_vectors(&vectors).unwrap();
        
        assert_eq!(zone_map.min_values, vec![1.0, 2.0, 3.0]);
        assert_eq!(zone_map.max_values, vec![7.0, 8.0, 9.0]);
        assert_eq!(zone_map.centroid, vec![4.0, 5.0, 6.0]);
        assert_eq!(zone_map.dimension, 3);
    }
    
    #[test]
    fn test_zone_map_euclidean_intersection() {
        let vectors = vec![
            vec![0.0, 0.0],
            vec![2.0, 2.0],
        ];
        
        let zone_map = ZoneMap::from_vectors(&vectors).unwrap();
        
        // Query inside the zone
        assert!(zone_map.intersects_euclidean(&[1.0, 1.0], 1.0));
        
        // Query outside but within distance
        assert!(zone_map.intersects_euclidean(&[3.0, 3.0], 2.0));
        
        // Query too far away
        assert!(!zone_map.intersects_euclidean(&[10.0, 10.0], 1.0));
    }
    
    #[test]
    fn test_superblock_creation() {
        let enhanced_stats = vec![
            EnhancedRowGroupStats {
                row_group_id: 0,
                parquet_metadata: None,
                vector_zone_map: ZoneMap::from_vectors(&[vec![1.0, 2.0, 3.0]]).unwrap(),
                quantized_selectivity: QuantizedSelectivity {
                    binary_effectiveness: 0.8,
                    int8_accuracy: 0.9,
                    pq_quality: 0.85,
                    progressive_efficiency: 0.75,
                },
                compression_ratio: 4.0,
                search_cost_estimate: SearchCostEstimate {
                    io_cost: 10.0,
                    cpu_cost: 20.0,
                    memory_cost: 15.0,
                    estimated_latency_ms: 50.0,
                },
            },
        ];
        
        let superblock = SuperBlock::new(0, 0..10, &enhanced_stats).unwrap();
        
        assert_eq!(superblock.id, 0);
        assert_eq!(superblock.row_groups, 0..10);
        assert_eq!(superblock.zone_map.dimension, 3);
    }
}
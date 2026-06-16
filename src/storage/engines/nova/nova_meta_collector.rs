//! NOVA metadata collector for hierarchical zone maps and SuperBlocks
//!
//! This collector gathers statistics during Parquet writes to create
//! NOVA's advanced metadata structures without re-reading the file.

use anyhow::Result;
use arrow_array::{Array, Float32Array, RecordBatch};
// These parquet metadata types are used internally for low-level operations
// The columnar module doesn't re-export them as they're implementation details
use parquet::file::metadata::RowGroupMetaData;
use serde::{Deserialize, Serialize};
use std::ops::Range;

use super::hierarchical_stats::{
    AccessStats, EnhancedRowGroupStats, QuantizationStats, QuantizedSelectivity,
    SearchCostEstimate, SelectivityHints, StorageStats, SuperBlock, ZoneMap,
};

/// NOVA metadata collector implementation
pub struct NovaMetadataCollector {
    /// Configuration
    config: NovaCollectorConfig,

    /// Current row group being processed
    current_row_group: Option<RowGroupBuilder>,

    /// Completed row group stats
    row_group_stats: Vec<EnhancedRowGroupStats>,

    /// SuperBlocks (aggregate of multiple row groups)
    superblocks: Vec<SuperBlock>,

    /// Vector dimension (detected from first batch)
    dimension: Option<usize>,
}

/// Configuration for NOVA metadata collection
#[derive(Debug, Clone)]
pub struct NovaCollectorConfig {
    /// Number of row groups per SuperBlock
    pub row_groups_per_superblock: usize,

    /// Whether to compute detailed vector statistics
    pub compute_vector_stats: bool,

    /// Sample rate for expensive statistics
    pub sample_rate: f32,
}

impl Default for NovaCollectorConfig {
    fn default() -> Self {
        Self {
            row_groups_per_superblock: 10,
            compute_vector_stats: true,
            sample_rate: 0.1, // Sample 10% for expensive stats
        }
    }
}

/// Builder for accumulating row group statistics
///
/// Incrementally computes statistics (min, max, mean, variance) for vectors
/// within a row group during the write process, avoiding a second pass over the data.
struct RowGroupBuilder {
    /// Row group identifier
    _row_group_id: usize,
    /// Number of vectors processed
    vector_count: usize,
    /// Minimum values per dimension
    min_values: Vec<f32>,
    /// Maximum values per dimension
    max_values: Vec<f32>,
    /// Sum of values per dimension (for mean calculation)
    sum_values: Vec<f64>,
    /// Sum of squared values per dimension (for variance calculation)
    sum_squares: Vec<f64>,
}

impl RowGroupBuilder {
    fn new(row_group_id: usize, dimension: usize) -> Self {
        Self {
            _row_group_id: row_group_id,
            vector_count: 0,
            min_values: vec![f32::MAX; dimension],
            max_values: vec![f32::MIN; dimension],
            sum_values: vec![0.0; dimension],
            sum_squares: vec![0.0; dimension],
        }
    }

    fn update(&mut self, vectors: &Float32Array, dimension: usize) {
        let values = vectors.values();
        let num_vectors = vectors.len() / dimension;

        for vec_idx in 0..num_vectors {
            let start = vec_idx * dimension;
            let end = start + dimension;
            let vector = &values[start..end];

            // Update min/max per dimension
            for (dim_idx, &val) in vector.iter().enumerate() {
                self.min_values[dim_idx] = self.min_values[dim_idx].min(val);
                self.max_values[dim_idx] = self.max_values[dim_idx].max(val);
                self.sum_values[dim_idx] += val as f64;
                self.sum_squares[dim_idx] += (val as f64) * (val as f64);
            }
        }

        self.vector_count += num_vectors;
    }

    fn build_zone_map(&self) -> ZoneMap {
        let dimension = self.min_values.len();
        let count = self.vector_count as f64;

        // Calculate centroid and variance
        let centroid: Vec<f32> = self
            .sum_values
            .iter()
            .map(|&sum| (sum / count) as f32)
            .collect();

        let variance: Vec<f32> = self
            .sum_values
            .iter()
            .zip(&self.sum_squares)
            .map(|(&sum, &sum_sq)| {
                let mean = sum / count;
                let var = (sum_sq / count) - (mean * mean);
                var.max(0.0) as f32 // Ensure non-negative due to floating point errors
            })
            .collect();

        // Calculate L2 norm bounds
        let min_norm = self.min_values.iter().map(|&v| v * v).sum::<f32>().sqrt();
        let max_norm = self.max_values.iter().map(|&v| v * v).sum::<f32>().sqrt();

        ZoneMap {
            min_values: self.min_values.clone(),
            max_values: self.max_values.clone(),
            centroid,
            variance,
            norm_bounds: (min_norm, max_norm),
            dimension,
        }
    }
}

impl NovaMetadataCollector {
    /// Create a new NOVA metadata collector
    pub fn new(config: NovaCollectorConfig) -> Self {
        Self {
            config,
            current_row_group: None,
            row_group_stats: Vec::new(),
            superblocks: Vec::new(),
            dimension: None,
        }
    }

    /// Build SuperBlock from a range of row groups
    fn build_superblock(&self, row_groups: Range<u32>) -> SuperBlock {
        let start = row_groups.start as usize;
        let end = row_groups.end as usize;

        // Handle empty stats case
        if self.row_group_stats.is_empty() || start >= self.row_group_stats.len() {
            return SuperBlock {
                id: 0,
                row_groups: row_groups.clone(),
                vector_count: 0,
                zone_map: ZoneMap {
                    min_values: vec![],
                    max_values: vec![],
                    centroid: vec![],
                    variance: vec![],
                    norm_bounds: (0.0, 0.0),
                    dimension: 0,
                },
                quantization_stats: Default::default(),
                selectivity_hints: Default::default(),
                storage_stats: StorageStats {
                    compressed_size: 0,
                    uncompressed_size: 0,
                    total_pages: 0,
                    avg_page_size: 0,
                    bloom_filter_size: 0,
                },
                last_updated: chrono::Utc::now(),
            };
        }

        let end = end.min(self.row_group_stats.len());
        let rg_stats = &self.row_group_stats[start..end];

        // Aggregate zone maps
        let mut min_values = vec![f32::MAX; self.dimension.unwrap_or(0)];
        let mut max_values = vec![f32::MIN; self.dimension.unwrap_or(0)];
        let mut sum_centroid = vec![0.0; self.dimension.unwrap_or(0)];
        let mut total_vectors = 0u64;

        for stats in rg_stats {
            let zone_map = &stats.vector_zone_map;
            for i in 0..zone_map.dimension {
                min_values[i] = min_values[i].min(zone_map.min_values[i]);
                max_values[i] = max_values[i].max(zone_map.max_values[i]);
                sum_centroid[i] += zone_map.centroid[i] as f64;
            }
            total_vectors += stats.access_stats.access_count;
        }

        let num_groups = rg_stats.len() as f64;
        let centroid = sum_centroid
            .iter()
            .map(|&s| (s / num_groups) as f32)
            .collect();

        SuperBlock {
            id: start as u32 / self.config.row_groups_per_superblock as u32,
            row_groups,
            vector_count: total_vectors,
            zone_map: ZoneMap {
                min_values,
                max_values,
                centroid,
                variance: vec![0.0; self.dimension.unwrap_or(0)], // Simplified
                norm_bounds: (0.0, 0.0),                          // Would need to recalculate
                dimension: self.dimension.unwrap_or(0),
            },
            quantization_stats: QuantizationStats {
                binary_selectivity: 0.8,
                int8_reconstruction_error: 0.1,
                pq_selectivity: 0.9,
                compression_ratio: 0.5,
                quantization_overhead_ms: 0,
            },
            selectivity_hints: SelectivityHints {
                binary_reduction_factor: 0.5,
                int8_reduction_factor: 0.3,
                pq_reduction_factor: 0.1,
                search_cost_estimate: 10.0,
                memory_requirement: 1024,
            },
            storage_stats: StorageStats {
                compressed_size: 0,
                uncompressed_size: 0,
                total_pages: 0,
                avg_page_size: 0,
                bloom_filter_size: 0,
            },
            last_updated: chrono::Utc::now(),
        }
    }
}

impl crate::storage::engines::core::formats::columnar::metadata_collector::MetadataCollector
    for NovaMetadataCollector
{
    fn on_row_group_start(&mut self, row_group_index: usize) -> Result<()> {
        if let Some(dim) = self.dimension {
            self.current_row_group = Some(RowGroupBuilder::new(row_group_index, dim));
        }
        Ok(())
    }

    fn on_batch_write(
        &mut self,
        batch: &RecordBatch,
        row_group_index: usize,
        _batch_index_in_group: usize,
    ) -> Result<()> {
        // Extract vector column
        if let Some(vector_col) = batch.column_by_name("vector")
            && let Some(float_array) = vector_col.as_any().downcast_ref::<Float32Array>()
        {
            // Detect dimension from first batch
            if self.dimension.is_none() && !float_array.is_empty() {
                // Assume vectors are stored flat, dimension = total_values / num_rows
                let dimension = float_array.len() / batch.num_rows();
                self.dimension = Some(dimension);

                // Initialize current row group if needed
                if self.current_row_group.is_none() {
                    self.current_row_group = Some(RowGroupBuilder::new(row_group_index, dimension));
                }
            }

            // Update statistics
            if let Some(ref mut builder) = self.current_row_group
                && let Some(dim) = self.dimension
            {
                builder.update(float_array, dim);
            }
        }

        Ok(())
    }

    fn on_row_group_complete(
        &mut self,
        row_group_index: usize,
        metadata: &RowGroupMetaData,
    ) -> Result<()> {
        if let Some(builder) = self.current_row_group.take() {
            let zone_map = builder.build_zone_map();

            let stats = EnhancedRowGroupStats {
                row_group_id: row_group_index as u32,
                parquet_metadata: Some(metadata.clone()),
                vector_zone_map: zone_map,
                quantized_selectivity: QuantizedSelectivity {
                    binary_effectiveness: 0.7,
                    int8_accuracy: 0.85,
                    pq_quality: 0.95,
                    progressive_efficiency: 0.75,
                },
                compression_ratio: metadata.compressed_size() as f32
                    / metadata.total_byte_size() as f32,
                search_cost_estimate: SearchCostEstimate {
                    io_cost: metadata.compressed_size() as f32,
                    cpu_cost: builder.vector_count as f32 * 0.1,
                    memory_cost: metadata.total_byte_size() as f32,
                    estimated_latency_ms: builder.vector_count as f32 * 0.001,
                    confidence: 0.8,
                },
                access_stats: AccessStats {
                    access_count: builder.vector_count as u64,
                    last_access: chrono::Utc::now(),
                    avg_selectivity: 0.5,
                    cache_hit_rate: 0.0,
                    access_frequency: 0.0,
                },
            };

            self.row_group_stats.push(stats);
        }

        Ok(())
    }

    fn finalize(&mut self, total_row_groups: usize) -> Result<()> {
        // Build SuperBlocks
        let superblock_count = total_row_groups.div_ceil(self.config.row_groups_per_superblock);

        for sb_idx in 0..superblock_count {
            let start = (sb_idx * self.config.row_groups_per_superblock) as u32;
            let end =
                ((sb_idx + 1) * self.config.row_groups_per_superblock).min(total_row_groups) as u32;

            if start < end {
                let superblock = self.build_superblock(start..end);
                self.superblocks.push(superblock);
            }
        }

        Ok(())
    }

    fn serialize_metadata(&self) -> Result<Vec<u8>> {
        let metadata = NovaMetadata {
            version: 1,
            dimension: self.dimension.unwrap_or(0),
            row_group_stats: self.row_group_stats.clone(),
            superblocks: self.superblocks.clone(),
            row_groups_per_superblock: self.config.row_groups_per_superblock,
        };

        // Serialize with bincode for efficiency
        bincode::serialize(&metadata).map_err(|e| anyhow::anyhow!("Serialization error: {}", e))
    }

    fn sidecar_extension(&self) -> &str {
        "nova_meta"
    }
}

/// Serializable NOVA metadata structure
///
/// Complete metadata structure stored in .nova_meta sidecar files,
/// containing hierarchical statistics for query optimization.
#[derive(Serialize, Deserialize, Clone)]
pub struct NovaMetadata {
    /// Metadata format version
    pub version: u32,
    /// Vector dimension
    pub dimension: usize,
    /// Per-row group enhanced statistics
    pub row_group_stats: Vec<EnhancedRowGroupStats>,
    /// SuperBlock aggregates for multi-level pruning
    pub superblocks: Vec<SuperBlock>,
    /// Number of row groups per SuperBlock
    pub row_groups_per_superblock: usize,
}

/// Build a `.nova_meta` sidecar (serialized `NovaMetadata`) directly from the
/// vectors of each PHYSICAL parquet row group (TD-040). Used at flush time,
/// keyed off the written file's footer row-group counts, so the sidecar's
/// row-group bounds line up exactly with the row groups the cold-read path
/// fetches by index — unlike the streaming collector, whose row groups track
/// logical write-batches, not the parquet's physical row groups.
/// `row_group_vectors[i]` holds the fp32 vectors of physical row group `i`
/// (in file order).
pub fn nova_sidecar_from_row_groups(
    dimension: usize,
    row_group_vectors: &[Vec<Vec<f32>>],
) -> Result<Vec<u8>> {
    let row_group_stats: Vec<EnhancedRowGroupStats> = row_group_vectors
        .iter()
        .enumerate()
        .map(|(rg_id, vectors)| EnhancedRowGroupStats {
            row_group_id: rg_id as u32,
            parquet_metadata: None,
            vector_zone_map: zone_map_from_vectors(dimension, vectors),
            quantized_selectivity: QuantizedSelectivity {
                binary_effectiveness: 0.7,
                int8_accuracy: 0.85,
                pq_quality: 0.95,
                progressive_efficiency: 0.75,
            },
            compression_ratio: 1.0,
            search_cost_estimate: SearchCostEstimate {
                io_cost: 0.0,
                cpu_cost: vectors.len() as f32 * 0.1,
                memory_cost: 0.0,
                estimated_latency_ms: vectors.len() as f32 * 0.001,
                confidence: 0.8,
            },
            access_stats: AccessStats {
                access_count: vectors.len() as u64,
                last_access: chrono::Utc::now(),
                avg_selectivity: 0.5,
                cache_hit_rate: 0.0,
                access_frequency: 0.0,
            },
        })
        .collect();

    let metadata = NovaMetadata {
        version: 1,
        dimension,
        row_group_stats,
        superblocks: Vec::new(),
        row_groups_per_superblock: 4,
    };
    bincode::serialize(&metadata).map_err(|e| anyhow::anyhow!("Serialization error: {}", e))
}

/// Per-dimension min/max + centroid + L2 norm bounds over a row group's vectors
/// (mirrors `RowGroupBuilder::build_zone_map`). A row group with no usable
/// vectors collapses to a zero box (L2 lower bound 0 ⇒ never wrongly pruned).
fn zone_map_from_vectors(dimension: usize, vectors: &[Vec<f32>]) -> ZoneMap {
    let mut min_values = vec![f32::MAX; dimension];
    let mut max_values = vec![f32::MIN; dimension];
    let mut sum = vec![0f64; dimension];
    let mut count = 0u64;
    for v in vectors {
        if v.is_empty() {
            continue;
        }
        count += 1;
        for (i, &x) in v.iter().enumerate().take(dimension) {
            if x < min_values[i] {
                min_values[i] = x;
            }
            if x > max_values[i] {
                max_values[i] = x;
            }
            sum[i] += x as f64;
        }
    }
    if count == 0 {
        min_values = vec![0.0; dimension];
        max_values = vec![0.0; dimension];
    }
    let denom = count.max(1) as f64;
    let centroid: Vec<f32> = sum.iter().map(|&s| (s / denom) as f32).collect();
    let min_norm = min_values.iter().map(|&v| v * v).sum::<f32>().sqrt();
    let max_norm = max_values.iter().map(|&v| v * v).sum::<f32>().sqrt();
    ZoneMap {
        min_values,
        max_values,
        centroid,
        variance: vec![0.0; dimension],
        norm_bounds: (min_norm, max_norm),
        dimension,
    }
}

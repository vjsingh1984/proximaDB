//! VIPER metadata collector for row group centroids and radius
//!
//! This collector gathers vector statistics during Parquet writes to create
//! centroid-based metadata for efficient row group pruning during search.

use anyhow::Result;
use arrow_array::{Array, Float32Array, RecordBatch};
use parquet::file::metadata::RowGroupMetaData;
use serde::{Deserialize, Serialize};

use super::unified_metadata_serializer::RowGroupMetadata;

/// VIPER metadata collector implementation
/// Computes centroids and radius for each row group during flush
pub struct ViperMetadataCollector {
    /// Configuration
    config: ViperCollectorConfig,

    /// Current row group being processed
    current_row_group: Option<RowGroupBuilder>,

    /// Completed row group metadata
    row_group_metadata: Vec<RowGroupMetadata>,

    /// Vector dimension (detected from first batch)
    dimension: Option<usize>,
}

/// Configuration for VIPER metadata collection
///
/// Controls what statistics are computed during Parquet writes for
/// efficient row group pruning during search.
#[derive(Debug, Clone)]
pub struct ViperCollectorConfig {
    /// Whether to compute centroids
    pub compute_centroids: bool,

    /// Whether to compute radius (max distance from centroid)
    pub compute_radius: bool,

    /// Sample rate for expensive statistics (1.0 = all, 0.1 = 10%)
    pub sample_rate: f32,
}

impl Default for ViperCollectorConfig {
    fn default() -> Self {
        Self {
            compute_centroids: true,
            compute_radius: true,
            sample_rate: 1.0, // Compute for all vectors (centroids are cheap)
        }
    }
}

/// Builder for accumulating row group statistics
struct RowGroupBuilder {
    row_group_id: usize,
    vector_count: usize,
    sum_values: Vec<f64>,
    vectors_for_radius: Vec<Vec<f32>>,
    file_offset: u64,
    total_byte_size: u64,
    compressed_size: u64,
}

impl RowGroupBuilder {
    fn new(row_group_id: usize, dimension: usize) -> Self {
        Self {
            row_group_id,
            vector_count: 0,
            sum_values: vec![0.0; dimension],
            vectors_for_radius: Vec::new(),
            file_offset: 0,
            total_byte_size: 0,
            compressed_size: 0,
        }
    }

    fn update(&mut self, vectors: &Float32Array, dimension: usize, sample_rate: f32) {
        let values = vectors.values();
        let num_vectors = vectors.len() / dimension;

        for vec_idx in 0..num_vectors {
            let start = vec_idx * dimension;
            let end = start + dimension;
            let vector = &values[start..end];

            // Update sum for centroid calculation
            for (dim_idx, &val) in vector.iter().enumerate() {
                self.sum_values[dim_idx] += val as f64;
            }

            // Store vectors for radius calculation (with sampling)
            if sample_rate >= 1.0 || rand::random::<f32>() < sample_rate {
                self.vectors_for_radius.push(vector.to_vec());
            }
        }

        self.vector_count += num_vectors;
    }

    fn compute_centroid(&self) -> Vec<f32> {
        if self.vector_count == 0 {
            return vec![];
        }

        let count = self.vector_count as f64;
        self.sum_values
            .iter()
            .map(|&sum| (sum / count) as f32)
            .collect()
    }

    fn compute_radius(&self, centroid: &[f32]) -> f32 {
        if self.vectors_for_radius.is_empty() || centroid.is_empty() {
            return 0.0;
        }

        let mut max_distance: f32 = 0.0;

        for vector in &self.vectors_for_radius {
            let distance = compute_l2_distance(vector, centroid);
            if distance > max_distance {
                max_distance = distance;
            }
        }

        max_distance
    }

    fn build_metadata(&self, compute_radius: bool) -> RowGroupMetadata {
        let centroid = self.compute_centroid();
        let radius = if compute_radius && !centroid.is_empty() {
            Some(self.compute_radius(&centroid))
        } else {
            None
        };

        RowGroupMetadata {
            id: self.row_group_id as u32,
            row_count: self.vector_count,
            file_offset: self.file_offset,
            total_byte_size: self.total_byte_size,
            compressed_size: self.compressed_size,
            centroid: if centroid.is_empty() {
                None
            } else {
                Some(centroid)
            },
            radius,
        }
    }
}

/// Compute L2 (Euclidean) distance between two vectors
#[inline]
fn compute_l2_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        let diff = x - y;
        sum += diff * diff;
    }
    sum.sqrt()
}

impl ViperMetadataCollector {
    /// Create a new VIPER metadata collector
    pub fn new(config: ViperCollectorConfig) -> Self {
        Self {
            config,
            current_row_group: None,
            row_group_metadata: Vec::new(),
            dimension: None,
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(ViperCollectorConfig::default())
    }

    /// Get the collected row group metadata
    pub fn get_row_group_metadata(&self) -> &[RowGroupMetadata] {
        &self.row_group_metadata
    }

    /// Get dimension
    pub fn dimension(&self) -> Option<usize> {
        self.dimension
    }
}

impl crate::storage::engines::core::formats::columnar::metadata_collector::MetadataCollector
    for ViperMetadataCollector
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
        // Try to find vector column - check multiple possible names
        let vector_col = batch
            .column_by_name("vector")
            .or_else(|| batch.column_by_name("vector_fp32"))
            .or_else(|| batch.column_by_name("vectors"));

        if let Some(vector_col) = vector_col {
            // For FixedSizeListArray, we need to get the values
            if let Some(list_array) = vector_col
                .as_any()
                .downcast_ref::<arrow_array::FixedSizeListArray>()
            {
                // Get the underlying Float32Array values
                if let Some(float_array) =
                    list_array.values().as_any().downcast_ref::<Float32Array>()
                {
                    // Detect dimension from first batch
                    if self.dimension.is_none() && !float_array.is_empty() {
                        let dimension = list_array.value_length() as usize;
                        self.dimension = Some(dimension);

                        // Initialize current row group if needed
                        if self.current_row_group.is_none() {
                            self.current_row_group =
                                Some(RowGroupBuilder::new(row_group_index, dimension));
                        }
                    }

                    // Update statistics
                    if let Some(ref mut builder) = self.current_row_group
                        && let Some(dim) = self.dimension {
                            builder.update(float_array, dim, self.config.sample_rate);
                        }
                }
            }
            // Also handle direct Float32Array (less common)
            else if let Some(float_array) = vector_col.as_any().downcast_ref::<Float32Array>() {
                // Detect dimension from first batch
                if self.dimension.is_none() && !float_array.is_empty() {
                    let dimension = float_array.len() / batch.num_rows();
                    self.dimension = Some(dimension);

                    if self.current_row_group.is_none() {
                        self.current_row_group =
                            Some(RowGroupBuilder::new(row_group_index, dimension));
                    }
                }

                if let Some(ref mut builder) = self.current_row_group
                    && let Some(dim) = self.dimension {
                        builder.update(float_array, dim, self.config.sample_rate);
                    }
            }
        }

        Ok(())
    }

    fn on_row_group_complete(
        &mut self,
        _row_group_index: usize,
        metadata: &RowGroupMetaData,
    ) -> Result<()> {
        if let Some(mut builder) = self.current_row_group.take() {
            // Update file offset and sizes from Parquet metadata
            builder.compressed_size = metadata.compressed_size() as u64;
            builder.total_byte_size = metadata.total_byte_size() as u64;

            // Build metadata with centroid and radius
            let row_group_meta = builder.build_metadata(self.config.compute_radius);
            self.row_group_metadata.push(row_group_meta);
        }
        Ok(())
    }

    fn finalize(&mut self, _total_row_groups: usize) -> Result<()> {
        // Nothing special to do - metadata is already collected
        Ok(())
    }

    fn serialize_metadata(&self) -> Result<Vec<u8>> {
        let metadata = ViperSidecarMetadata {
            version: 1,
            dimension: self.dimension.unwrap_or(0),
            row_group_metadata: self.row_group_metadata.clone(),
        };

        bincode::serialize(&metadata).map_err(|e| anyhow::anyhow!("Serialization error: {}", e))
    }

    fn sidecar_extension(&self) -> &str {
        "viper_meta"
    }
}

/// Serializable VIPER sidecar metadata structure
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ViperSidecarMetadata {
    pub version: u32,
    pub dimension: usize,
    pub row_group_metadata: Vec<RowGroupMetadata>,
}

impl ViperSidecarMetadata {
    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        bincode::deserialize(bytes).map_err(|e| anyhow::anyhow!("Deserialization error: {}", e))
    }

    /// Get row groups that could potentially contain results
    /// Uses centroid-based pruning to filter out row groups that are too far
    pub fn select_row_groups_for_search(
        &self,
        query_vector: &[f32],
        distance_threshold: Option<f32>,
    ) -> Vec<u32> {
        let mut selected = Vec::new();

        for rg in &self.row_group_metadata {
            // If we have centroid and radius, use them for pruning
            if let (Some(centroid), Some(radius)) = (&rg.centroid, rg.radius) {
                let distance_to_centroid = compute_l2_distance(query_vector, centroid);

                // If the query is closer than threshold + radius, this row group might contain results
                if let Some(threshold) = distance_threshold {
                    if distance_to_centroid <= threshold + radius {
                        selected.push(rg.id);
                    }
                } else {
                    // No threshold - always include (will sort by distance to centroid later)
                    selected.push(rg.id);
                }
            } else {
                // No centroid data - must include to avoid missing results
                selected.push(rg.id);
            }
        }

        selected
    }

    /// Get row groups sorted by distance to query (closest first)
    /// Useful for progressive search - try closest row groups first
    pub fn sort_row_groups_by_distance(&self, query_vector: &[f32]) -> Vec<(u32, f32)> {
        let mut distances: Vec<(u32, f32)> = self
            .row_group_metadata
            .iter()
            .map(|rg| {
                let dist = if let Some(ref centroid) = rg.centroid {
                    compute_l2_distance(query_vector, centroid)
                } else {
                    f32::MAX // No centroid - put at end
                };
                (rg.id, dist)
            })
            .collect();

        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        distances
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::formats::columnar::metadata_collector::MetadataCollector;
    use arrow_array::{FixedSizeListArray, Float32Array, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    #[test]
    fn test_centroid_computation() {
        let config = ViperCollectorConfig::default();
        let mut collector = ViperMetadataCollector::new(config);

        // Simulate a batch with 4 vectors of dimension 3
        let vectors = vec![
            1.0f32, 0.0, 0.0, // Vector 1
            0.0, 1.0, 0.0, // Vector 2
            0.0, 0.0, 1.0, // Vector 3
            1.0, 1.0, 1.0, // Vector 4
        ];

        let values_array = Float32Array::from(vectors);
        let list_field = Field::new("item", DataType::Float32, false);
        let list_array = FixedSizeListArray::try_new(
            Arc::new(list_field),
            3, // dimension
            Arc::new(values_array),
            None,
        )
        .unwrap();

        // Create a RecordBatch
        let schema = Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, false)), 3),
                false,
            ),
        ]);

        let ids = StringArray::from(vec!["v1", "v2", "v3", "v4"]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(ids), Arc::new(list_array)])
                .unwrap();

        // Process the batch
        MetadataCollector::on_row_group_start(&mut collector, 0).unwrap();
        MetadataCollector::on_batch_write(&mut collector, &batch, 0, 0).unwrap();

        // Check dimension was detected
        assert_eq!(collector.dimension(), Some(3));

        // Build metadata manually to verify centroid
        if let Some(ref builder) = collector.current_row_group {
            let centroid = builder.compute_centroid();
            // Expected centroid: (1+0+0+1)/4, (0+1+0+1)/4, (0+0+1+1)/4 = (0.5, 0.5, 0.5)
            assert!((centroid[0] - 0.5).abs() < 0.001);
            assert!((centroid[1] - 0.5).abs() < 0.001);
            assert!((centroid[2] - 0.5).abs() < 0.001);
        }
    }

    #[test]
    fn test_radius_computation() {
        let centroid = vec![0.0f32, 0.0, 0.0];
        let vectors = vec![vec![1.0f32, 0.0, 0.0], vec![0.0, 2.0, 0.0]];

        let mut max_dist: f32 = 0.0;
        for v in &vectors {
            let dist = compute_l2_distance(v, &centroid);
            if dist > max_dist {
                max_dist = dist;
            }
        }

        // Second vector is farthest (distance = 2.0)
        assert!((max_dist - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_row_group_selection() {
        let metadata = ViperSidecarMetadata {
            version: 1,
            dimension: 3,
            row_group_metadata: vec![
                RowGroupMetadata {
                    id: 0,
                    row_count: 100,
                    file_offset: 0,
                    total_byte_size: 1024,
                    compressed_size: 512,
                    centroid: Some(vec![0.0, 0.0, 0.0]),
                    radius: Some(1.0),
                },
                RowGroupMetadata {
                    id: 1,
                    row_count: 100,
                    file_offset: 1024,
                    total_byte_size: 1024,
                    compressed_size: 512,
                    centroid: Some(vec![10.0, 10.0, 10.0]),
                    radius: Some(1.0),
                },
            ],
        };

        let query = vec![0.5f32, 0.5, 0.5];

        // With threshold of 2.0, only row group 0 should be selected
        // (distance to centroid 0 is ~0.866, + radius 1.0 < 2.0)
        // (distance to centroid 1 is ~16.4, + radius 1.0 > 2.0)
        let selected = metadata.select_row_groups_for_search(&query, Some(2.0));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0], 0);

        // Without threshold, both should be included
        let all_selected = metadata.select_row_groups_for_search(&query, None);
        assert_eq!(all_selected.len(), 2);
    }
}

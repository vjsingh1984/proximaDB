// RAPTOR Row Group Manager - Hybrid Row Group + Columnar Architecture
// Manages row groups with columnar storage within each group for SIMD optimization

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use super::common::{
    ColumnarBlock, FastLanesEncodedData, FastLanesScheme, MetadataColumns, QuantizationParams,
    QuantizedColumnarData, RowGroup, TransposedVectors,
};
use super::config::RaptorConfig;
use super::smart_rowgroup_sizing::{OptimalRowGroupSize, SmartRowGroupSizer};
use crate::compute::quantization::storage_engine::StorageQuantizationEngine;
use crate::core::VectorRecord;
use crate::storage::engines::core::ops::fastlanes_encoding::FastLanesEncoder;

// RowGroup removed - consolidated into common::RowGroup
// The unified RowGroup now includes columnar_data field

// ColumnarBlock moved to common.rs - using unified structure

// TransposedVectors moved to common.rs

// FastLanesEncodedData moved to common.rs

// FastLanesScheme imported from common.rs

// QuantizedColumnarData and QuantizationParams moved to common.rs

// MetadataColumns moved to common.rs

// GraphNode and LocalHnswGraph removed - obsolete with Matrix Trinity architecture
// We now use P² + K² + P×K matrices instead of graph-based navigation

/// Row Group Manager for RAPTOR engine (unified implementation)
pub struct RowGroups {
    /// Active row groups (using u16 IDs for 67M+ vectors)
    row_groups: HashMap<u16, RowGroup>,
    /// Current row group being written to
    current_row_group: Option<u16>,
    /// Next rowgroup ID counter
    next_id: u16,
    /// Smart sizing configuration
    smart_sizer: SmartRowGroupSizer,
    /// Optimal row group size calculated from smart sizer
    optimal_size: OptimalRowGroupSize,
    /// Quantization engine if enabled
    quantization_engine: Option<Arc<StorageQuantizationEngine>>,
    /// FastLanes encoder
    fastlanes_encoder: FastLanesEncoder,
    /// Configuration
    config: RaptorConfig,
}

impl RowGroups {
    /// Create new row group manager with smart sizing
    pub fn new(
        config: RaptorConfig,
        smart_sizer: SmartRowGroupSizer,
        quantization_engine: Option<Arc<StorageQuantizationEngine>>,
    ) -> Result<Self> {
        let optimal_size = smart_sizer.calculate_optimal_rowgroup_size()?;

        tracing::info!("RAPTOR RowGroups initialized: {}", optimal_size.rationale);

        let fastlanes_encoder = FastLanesEncoder::new(
            crate::storage::engines::core::ops::fastlanes_encoding::FastLanesScheme::BitPacked {
                bits: 16,
            },
        );

        Ok(Self {
            row_groups: HashMap::new(),
            current_row_group: None,
            next_id: 0,
            smart_sizer,
            optimal_size,
            quantization_engine,
            fastlanes_encoder,
            config,
        })
    }

    /// Add vectors to the current row group (creates new if needed)
    pub async fn add_vectors(&mut self, vectors: Vec<VectorRecord>) -> Result<Vec<u16>> {
        let mut row_group_ids = Vec::new();
        let mut remaining_vectors = vectors;

        while !remaining_vectors.is_empty() {
            // Get or create current row group
            let row_group_id = self.get_or_create_current_row_group().await?;
            let row_group = self
                .row_groups
                .get_mut(&row_group_id)
                .ok_or_else(|| anyhow::anyhow!("Row group not found"))?;

            // Calculate how many vectors can fit
            let available_space = row_group.max_vectors - row_group.vector_count;
            let vectors_to_add = remaining_vectors.len().min(available_space);

            if vectors_to_add == 0 {
                // Current row group is full, mark it as complete and continue
                self.complete_current_row_group().await?;
                continue;
            }

            // Split vectors
            let batch: Vec<VectorRecord> = remaining_vectors.drain(..vectors_to_add).collect();

            // Add to row group
            self.add_vectors_to_row_group(row_group_id, batch).await?;
            row_group_ids.push(row_group_id);

            // Check if row group is now full
            let row_group = self.row_groups.get(&row_group_id).unwrap();
            if row_group.vector_count >= row_group.max_vectors {
                self.complete_current_row_group().await?;
            }
        }

        Ok(row_group_ids)
    }

    /// Get or create the current row group for writing
    async fn get_or_create_current_row_group(&mut self) -> Result<u16> {
        match self.current_row_group {
            Some(id) => Ok(id),
            None => {
                let new_id = self.create_new_row_group().await?;
                self.current_row_group = Some(new_id);
                Ok(new_id)
            }
        }
    }

    fn next_rowgroup_id(&mut self) -> u16 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Create a new row group with optimal sizing
    async fn create_new_row_group(&mut self) -> Result<u16> {
        let id = self.next_rowgroup_id();
        let max_vectors = self.optimal_size.vectors_per_rowgroup;

        let mut row_group = RowGroup::with_capacity(id, max_vectors);
        row_group.columnar_data = Some(ColumnarBlock {
            vector_ids: Vec::with_capacity(max_vectors),
            transposed_vectors: Some(TransposedVectors {
                dimensions: Vec::new(),
                dimension: 0,
                vector_count: 0,
            }),
            fastlanes_data: None,
            quantized_data: None,
            metadata_columns: MetadataColumns {
                string_columns: HashMap::new(),
                numeric_columns: HashMap::new(),
                boolean_columns: HashMap::new(),
            },
        });

        self.row_groups.insert(id, row_group);

        tracing::debug!(
            "Created new row group {} with capacity {} vectors ({:.1}MB estimated)",
            id,
            max_vectors,
            self.optimal_size.total_rowgroup_bytes as f32 / (1024.0 * 1024.0)
        );

        Ok(id)
    }

    /// Add vectors to a specific row group
    async fn add_vectors_to_row_group(
        &mut self,
        row_group_id: u16,
        vectors: Vec<VectorRecord>,
    ) -> Result<()> {
        // Check if row group exists first
        if !self.row_groups.contains_key(&row_group_id) {
            return Err(anyhow::anyhow!("Row group not found"));
        }

        // Process vectors and prepare metadata before getting mutable reference
        let metadata_maps: Vec<_> = vectors
            .iter()
            .map(|record| record.metadata.clone())
            .collect();

        // Now get mutable reference and update
        let row_group = self.row_groups.get_mut(&row_group_id).unwrap();

        // Ensure columnar_data exists
        if row_group.columnar_data.is_none() {
            row_group.columnar_data = Some(ColumnarBlock {
                vector_ids: Vec::new(),
                transposed_vectors: None,
                fastlanes_data: None,
                quantized_data: None,
                metadata_columns: MetadataColumns {
                    string_columns: HashMap::new(),
                    numeric_columns: HashMap::new(),
                    boolean_columns: HashMap::new(),
                },
            });
        }

        let columnar_data = row_group.columnar_data.as_mut().unwrap();

        // Initialize transposed vectors if first batch
        if columnar_data.transposed_vectors.is_none() && !vectors.is_empty() {
            let dimension = vectors[0].vector.len();
            columnar_data.transposed_vectors = Some(TransposedVectors {
                dimensions: vec![Vec::with_capacity(row_group.max_vectors); dimension],
                dimension,
                vector_count: 0,
            });
        }

        // Add vectors in transposed (columnar) format
        for vector in vectors {
            // Add vector ID
            columnar_data.vector_ids.push(if vector.id.is_empty() {
                format!("vec_{}", row_group.vector_count)
            } else {
                vector.id.clone()
            });

            // Add vector data (transpose: vector[d] -> dimensions[d].push(value))
            if let Some(ref mut transposed) = columnar_data.transposed_vectors {
                for (dim_idx, value) in vector.vector.iter().enumerate() {
                    if dim_idx < transposed.dimensions.len() {
                        transposed.dimensions[dim_idx].push(*value);
                    }
                }
            }

            // Add metadata (columnar format)
            if !vector.metadata.is_empty() {
                // Convert Vec<MetadataItem> to HashMap
                let metadata_map: HashMap<String, serde_json::Value> = vector
                    .metadata
                    .iter()
                    .map(|(key, value)| {
                        (
                            item.0.clone(),
                            match &value {
                                Some(
                                    crate::proto::proximadb_v1::metadata_item::Value::StringValue(s),
                                ) => serde_json::Value::String(s.clone()),
                                Some(
                                    crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n),
                                ) => serde_json::Value::Number(
                                    serde_json::Number::from_f64(*n)
                                        .unwrap_or_else(|| serde_json::Number::from(0)),
                                ),
                                Some(crate::proto::proximadb_v1::metadata_item::Value::BoolValue(
                                    b,
                                )) => serde_json::Value::Bool(*b),
                                None => serde_json::Value::Null,
                            },
                        )
                    })
                    .collect();
                // Inline metadata addition to avoid borrow conflict
                for (key, value) in metadata_map {
                    match value {
                        serde_json::Value::String(s) => {
                            columnar_data
                                .metadata_columns
                                .string_columns
                                .entry(key)
                                .or_insert_with(Vec::new)
                                .push(Some(s));
                        }
                        serde_json::Value::Number(n) => {
                            columnar_data
                                .metadata_columns
                                .numeric_columns
                                .entry(key)
                                .or_insert_with(Vec::new)
                                .push(n.as_f64());
                        }
                        serde_json::Value::Bool(b) => {
                            columnar_data
                                .metadata_columns
                                .boolean_columns
                                .entry(key)
                                .or_insert_with(Vec::new)
                                .push(Some(b));
                        }
                        _ => {
                            // For null or other types, add null to string columns
                            columnar_data
                                .metadata_columns
                                .string_columns
                                .entry(key)
                                .or_insert_with(Vec::new)
                                .push(None);
                        }
                    }
                }
            } else {
                // Add null values for all known columns
                // Collect all known column names from existing columns
                let mut all_keys = std::collections::HashSet::new();
                all_keys.extend(
                    columnar_data
                        .metadata_columns
                        .string_columns
                        .keys()
                        .cloned(),
                );
                all_keys.extend(
                    columnar_data
                        .metadata_columns
                        .numeric_columns
                        .keys()
                        .cloned(),
                );
                all_keys.extend(
                    columnar_data
                        .metadata_columns
                        .boolean_columns
                        .keys()
                        .cloned(),
                );

                for key in all_keys {
                    // Check which type the column is and add null value
                    if columnar_data
                        .metadata_columns
                        .string_columns
                        .contains_key(&key)
                    {
                        columnar_data
                            .metadata_columns
                            .string_columns
                            .get_mut(&key)
                            .unwrap()
                            .push(None);
                    } else if columnar_data
                        .metadata_columns
                        .numeric_columns
                        .contains_key(&key)
                    {
                        columnar_data
                            .metadata_columns
                            .numeric_columns
                            .get_mut(&key)
                            .unwrap()
                            .push(None);
                    } else if columnar_data
                        .metadata_columns
                        .boolean_columns
                        .contains_key(&key)
                    {
                        columnar_data
                            .metadata_columns
                            .boolean_columns
                            .get_mut(&key)
                            .unwrap()
                            .push(None);
                    }
                }
            }

            row_group.vector_count += 1;
            if let Some(ref mut transposed) = columnar_data.transposed_vectors {
                transposed.vector_count += 1;
            }
        }

        Ok(())
    }

    /// Add metadata in columnar format
    fn add_metadata_columnar(
        &self,
        metadata_columns: &mut MetadataColumns,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        // Process each metadata field
        for (key, value) in metadata {
            match value {
                serde_json::Value::String(s) => {
                    metadata_columns
                        .string_columns
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(Some(s));
                }
                serde_json::Value::Number(n) => {
                    metadata_columns
                        .numeric_columns
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(n.as_f64());
                }
                serde_json::Value::Bool(b) => {
                    metadata_columns
                        .boolean_columns
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(Some(b));
                }
                _ => {
                    // Serialize complex types as strings
                    metadata_columns
                        .string_columns
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push(Some(value.to_string()));
                }
            }
        }

        Ok(())
    }

    /// Add empty metadata entries to maintain column alignment
    fn add_empty_metadata_columnar(&self, metadata_columns: &mut MetadataColumns) -> Result<()> {
        // Add None to all existing columns to maintain alignment
        for column in metadata_columns.string_columns.values_mut() {
            column.push(None);
        }
        for column in metadata_columns.numeric_columns.values_mut() {
            column.push(None);
        }
        for column in metadata_columns.boolean_columns.values_mut() {
            column.push(None);
        }

        Ok(())
    }

    /// Complete the current row group (apply compression, quantization, build matrices)
    async fn complete_current_row_group(&mut self) -> Result<()> {
        if let Some(row_group_id) = self.current_row_group.take() {
            tracing::info!(
                "Completing row group {} with {} vectors",
                row_group_id,
                self.row_groups
                    .get(&row_group_id)
                    .map(|rg| rg.vector_count)
                    .unwrap_or(0)
            );

            // Apply FastLanes compression
            self.apply_fastlanes_compression(row_group_id).await?;

            // Apply quantization if enabled
            if self.quantization_engine.is_some() {
                self.apply_quantization(row_group_id).await?;
            }

            // Build P² matrix for intra-rowgroup navigation
            self.build_p2_matrix(row_group_id).await?;
        }

        Ok(())
    }

    /// Apply FastLanes compression to a row group
    async fn apply_fastlanes_compression(&mut self, row_group_id: u16) -> Result<()> {
        let row_group = self
            .row_groups
            .get_mut(&row_group_id)
            .ok_or_else(|| anyhow::anyhow!("Row group not found"))?;

        let columnar_data = row_group
            .columnar_data
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Columnar data not initialized"))?;

        let mut encoded_dimensions = Vec::new();
        let mut encoding_schemes = Vec::new();

        // Compress each dimension separately using FastLanes
        let transposed = columnar_data
            .transposed_vectors
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Transposed vectors not available"))?;

        for dimension_data in &transposed.dimensions {
            if !dimension_data.is_empty() {
                let encoded = self.fastlanes_encoder.encode_f32(dimension_data)?;
                encoded_dimensions.push(encoded);
                encoding_schemes.push(FastLanesScheme::BitPacked { bits: 16 }); // Default scheme
            }
        }

        // Calculate compression ratio
        let original_size = transposed.dimensions.len() * row_group.vector_count * 4; // 4 bytes per f32
        let compressed_size: usize = encoded_dimensions.iter().map(|d| d.len()).sum();
        let compression_ratio = original_size as f32 / compressed_size.max(1) as f32;

        columnar_data.fastlanes_data = Some(FastLanesEncodedData {
            encoded_dimensions,
            encoding_schemes,
            compression_ratio,
        });

        tracing::debug!(
            "FastLanes compression: {:.2}x ratio for row group {}",
            compression_ratio,
            row_group_id
        );

        Ok(())
    }

    /// Apply quantization to a row group
    async fn apply_quantization(&mut self, row_group_id: u16) -> Result<()> {
        if let Some(ref quantization_engine) = self.quantization_engine {
            let row_group = self
                .row_groups
                .get_mut(&row_group_id)
                .ok_or_else(|| anyhow::anyhow!("Row group not found"))?;

            // Ensure columnar data exists
            if row_group.columnar_data.is_none() {
                row_group.columnar_data = Some(ColumnarBlock::default());
            }
            let columnar_data = row_group.columnar_data.as_mut().unwrap();

            // Reconstruct vectors for quantization (transpose back)
            let mut vectors = Vec::new();
            if let Some(ref transposed) = columnar_data.transposed_vectors {
                for i in 0..row_group.vector_count {
                    let mut vector = Vec::with_capacity(transposed.dimension);
                    for dim_data in &transposed.dimensions {
                        if i < dim_data.len() {
                            vector.push(dim_data[i]);
                        }
                    }
                    vectors.push(vector);
                }
            }

            // Apply quantization
            let quantized = quantization_engine.quantize_batch(&vectors, None).await?;

            // Store quantized data in columnar format
            // Note: quantized is Vec<StorageQuantizedData>, need to extract data appropriately
            // For now, create empty structure as the exact mapping needs clarification
            columnar_data.quantized_data = Some(QuantizedColumnarData {
                binary: Some(Vec::new()), // Would extract from quantized[*].filter
                int8: Some(Vec::new()),   // Would extract from quantized[*].fast
                pq4: Some(Vec::new()),    // Would extract from quantized[*].primary if PQ4
                pq8: Some(Vec::new()),    // Would extract from quantized[*].primary if PQ8
                quantization_params: QuantizationParams {
                    scale: 1.0,
                    offset: 0.0,
                    codebook: Some(Vec::new()), // Would extract from quantization metadata
                },
            });

            tracing::debug!("Applied quantization to row group {}", row_group_id);
        }

        Ok(())
    }

    /// Build P² matrix for a row group (intra-rowgroup distances)
    async fn build_p2_matrix(&mut self, row_group_id: u16) -> Result<()> {
        // P² matrix stores pre-computed distances between all vectors in the rowgroup
        // Upper triangle only (symmetric matrix), INT8 quantized for efficiency
        tracing::debug!("Building P² matrix for row group {}", row_group_id);
        // Implementation would compute and store the matrix
        Ok(())
    }

    /// Get row group by ID
    pub fn row_group(&self, id: &u16) -> Option<&RowGroup> {
        self.row_groups.get(id)
    }

    /// Get all row group IDs
    pub fn row_group_ids(&self) -> Vec<u16> {
        self.row_groups.keys().cloned().collect()
    }

    /// Get smart sizing configuration
    pub fn smart_sizing_info(&self) -> &OptimalRowGroupSize {
        &self.optimal_size
    }

    /// Update smart sizing (recalculate optimal row group size)
    pub fn update_smart_sizing(&mut self, new_sizer: SmartRowGroupSizer) -> Result<()> {
        self.smart_sizer = new_sizer;
        self.optimal_size = self.smart_sizer.calculate_optimal_rowgroup_size()?;

        tracing::info!(
            "Updated RAPTOR smart sizing: {}",
            self.optimal_size.rationale
        );

        Ok(())
    }
}

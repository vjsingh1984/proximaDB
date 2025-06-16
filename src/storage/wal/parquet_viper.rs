// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Parquet WAL Flushing with VIPER (Vectorized Intelligent Parquet with Efficient Retrieval)
//! 
//! This module implements advanced Parquet flushing capabilities:
//! - Columnar storage optimized for vector workloads
//! - VIPER clustering for efficient data organization  
//! - Intelligent partitioning and compression
//! - Schema evolution support

use anyhow::{Result, Context};
use arrow::array::{
    ArrayRef, StringArray, UInt64Array, Float32Array, BinaryArray, 
    TimestampMillisecondArray, RecordBatch, ListArray, MapArray
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::{Compression, Encoding};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use serde_json::Value;

use crate::core::{CollectionId, VectorId, VectorRecord};
use super::{WalEntry, WalOperation, FlushResult};
use crate::storage::filesystem::FilesystemFactory;
use crate::storage::unified_engine::CollectionConfig;

/// VIPER configuration for intelligent partitioning
#[derive(Debug, Clone)]
pub struct ViperConfig {
    /// Enable VIPER clustering (groups similar vectors together)
    pub enable_clustering: bool,
    
    /// Number of clusters for partitioning (0 = auto-detect)
    pub cluster_count: usize,
    
    /// Minimum vectors per partition
    pub min_vectors_per_partition: usize,
    
    /// Maximum vectors per partition  
    pub max_vectors_per_partition: usize,
    
    /// Enable dictionary encoding for vector IDs
    pub enable_dictionary_encoding: bool,
    
    /// Compression algorithm for Parquet
    pub compression: Compression,
    
    /// Page size for Parquet files
    pub page_size: usize,
    
    /// Row group size
    pub row_group_size: usize,
}

impl Default for ViperConfig {
    fn default() -> Self {
        Self {
            enable_clustering: true,
            cluster_count: 0, // Auto-detect
            min_vectors_per_partition: 1000,
            max_vectors_per_partition: 100000,
            enable_dictionary_encoding: true,
            compression: Compression::ZSTD(parquet::basic::ZstdLevel::try_new(3).unwrap()),
            page_size: 1024 * 1024, // 1MB
            row_group_size: 100000,  // 100K rows
        }
    }
}

/// VIPER Parquet flusher with intelligent clustering
#[derive(Debug)]
pub struct ViperParquetFlusher {
    config: ViperConfig,
    filesystem: Arc<FilesystemFactory>,
}

impl ViperParquetFlusher {
    /// Create new VIPER Parquet flusher
    pub fn new(config: ViperConfig, filesystem: Arc<FilesystemFactory>) -> Self {
        Self { config, filesystem }
    }
    
    /// Flush WAL entries to Parquet with VIPER optimization
    pub async fn flush_to_parquet(
        &self,
        collection_id: &CollectionId,
        entries: Vec<WalEntry>,
        output_path: &Path,
    ) -> Result<FlushResult> {
        if entries.is_empty() {
            return Ok(FlushResult {
                entries_flushed: 0,
                bytes_written: 0,
                segments_created: 0,
                collections_affected: vec![],
                flush_duration_ms: 0,
            });
        }
        
        let start_time = std::time::Instant::now();
        
        tracing::info!("🔥 Starting VIPER Parquet flush for collection {} with {} entries", 
                      collection_id, entries.len());
        
        // Step 1: Apply VIPER clustering if enabled
        let clustered_entries = if self.config.enable_clustering {
            self.apply_viper_clustering(&entries).await?
        } else {
            entries
        };
        
        // Step 2: Convert to Arrow RecordBatch
        let record_batch = self.convert_to_arrow_batch(&clustered_entries)?;
        
        // Step 3: Write to Parquet with optimized properties
        let bytes_written = self.write_parquet(&record_batch, output_path).await?;
        
        let flush_duration = start_time.elapsed().as_millis() as u64;
        
        tracing::info!("✅ VIPER Parquet flush completed: {} entries, {} bytes, {}ms", 
                      clustered_entries.len(), bytes_written, flush_duration);
        
        Ok(FlushResult {
            entries_flushed: clustered_entries.len() as u64,
            bytes_written,
            segments_created: 1,
            collections_affected: vec![collection_id.clone()],
            flush_duration_ms: flush_duration,
        })
    }
    
    /// Flush vector records to Parquet with dynamic schema - Strategy Pattern entry point
    pub async fn flush_vectors_to_parquet(
        &self,
        collection_config: &CollectionConfig,
        records: Vec<VectorRecord>,
        output_path: &Path,
    ) -> Result<FlushResult> {
        let strategy = ViperSchemaStrategy::new(collection_config);
        let processor = VectorRecordProcessor::new(&strategy);
        
        processor.flush_to_parquet(self, records, output_path).await
    }
    
    /// Apply VIPER clustering to organize vectors efficiently
    async fn apply_viper_clustering(&self, entries: &[WalEntry]) -> Result<Vec<WalEntry>> {
        tracing::debug!("🧠 Applying VIPER clustering to {} entries", entries.len());
        
        // Extract vectors for clustering
        let mut vectors = Vec::new();
        let mut vector_entries = Vec::new();
        
        for entry in entries {
            match &entry.operation {
                WalOperation::Insert { vector_id: _, record, expires_at: _ } |
                WalOperation::Update { vector_id: _, record, expires_at: _ } => {
                    vectors.push(record.vector.clone());
                    vector_entries.push(entry.clone());
                }
                _ => {
                    // Non-vector operations (collection ops, deletes) are not clustered
                    vector_entries.push(entry.clone());
                }
            }
        }
        
        if vectors.is_empty() {
            return Ok(entries.to_vec());
        }
        
        // Determine cluster count
        let cluster_count = if self.config.cluster_count == 0 {
            // Auto-detect: sqrt of number of vectors, capped between 2 and 50
            ((vectors.len() as f64).sqrt().round() as usize).clamp(2, 50)
        } else {
            self.config.cluster_count
        };
        
        tracing::debug!("🎯 Using {} clusters for {} vectors", cluster_count, vectors.len());
        
        // Apply K-means clustering using linfa
        let clustered_indices = self.perform_kmeans_clustering(&vectors, cluster_count)?;
        
        // Reorder entries based on cluster assignments
        let mut clustered_entries = Vec::with_capacity(vector_entries.len());
        let mut cluster_groups: HashMap<usize, Vec<WalEntry>> = HashMap::new();
        
        for (idx, entry) in vector_entries.into_iter().enumerate() {
            if idx < clustered_indices.len() {
                let cluster_id = clustered_indices[idx];
                cluster_groups.entry(cluster_id).or_default().push(entry);
            } else {
                // Non-vector entries go to cluster 0
                cluster_groups.entry(0).or_default().push(entry);
            }
        }
        
        // Flatten clusters back to a single vector (clustered entries are now grouped)
        for cluster_id in 0..cluster_count {
            if let Some(cluster_entries) = cluster_groups.remove(&cluster_id) {
                clustered_entries.extend(cluster_entries);
            }
        }
        
        tracing::debug!("✅ VIPER clustering complete: {} clusters, {} entries", 
                       cluster_count, clustered_entries.len());
        
        Ok(clustered_entries)
    }
    
    /// Perform K-means clustering on vectors using linfa
    fn perform_kmeans_clustering(&self, vectors: &[Vec<f32>], cluster_count: usize) -> Result<Vec<usize>> {
        use linfa::prelude::*;
        use linfa_clustering::KMeansParams;
        use ndarray::{Array2, ArrayView1};
        
        if vectors.is_empty() {
            return Ok(Vec::new());
        }
        
        let vector_dim = vectors[0].len();
        
        // Convert vectors to ndarray matrix
        let mut matrix_data = Vec::with_capacity(vectors.len() * vector_dim);
        for vector in vectors {
            if vector.len() != vector_dim {
                return Err(anyhow::anyhow!("Inconsistent vector dimensions"));
            }
            matrix_data.extend_from_slice(vector);
        }
        
        let matrix = Array2::from_shape_vec((vectors.len(), vector_dim), matrix_data)
            .context("Failed to create matrix for clustering")?;
        
        // Create dataset
        let dataset = Dataset::from(matrix);
        
        // Perform K-means clustering
        let kmeans = KMeansParams::new(cluster_count)
            .max_n_iterations(100)
            .tolerance(1e-4)
            .fit(&dataset)
            .context("K-means clustering failed")?;
        
        // Get cluster assignments
        let predictions = kmeans.predict(&dataset);
        let cluster_assignments: Vec<usize> = predictions.iter().cloned().collect();
        
        tracing::debug!("🎯 K-means clustering assigned vectors to {} clusters", cluster_count);
        
        Ok(cluster_assignments)
    }
    
    /// Convert WAL entries to Arrow RecordBatch
    fn convert_to_arrow_batch(&self, entries: &[WalEntry]) -> Result<RecordBatch> {
        let mut entry_ids = Vec::new();
        let mut collection_ids = Vec::new();
        let mut operation_types = Vec::new();
        let mut vector_ids = Vec::new();
        let mut vector_data = Vec::new();
        let mut timestamps = Vec::new();
        let mut sequences = Vec::new();
        let mut global_sequences = Vec::new();
        let mut versions = Vec::new();
        
        for entry in entries {
            entry_ids.push(entry.entry_id.clone());
            collection_ids.push(entry.collection_id.to_string());
            timestamps.push(entry.timestamp.timestamp_millis());
            sequences.push(entry.sequence);
            global_sequences.push(entry.global_sequence);
            versions.push(entry.version);
            
            match &entry.operation {
                WalOperation::Insert { vector_id, record, expires_at: _ } => {
                    operation_types.push("INSERT".to_string());
                    vector_ids.push(Some(vector_id.clone()));
                    
                    // Serialize vector record to bytes
                    let serialized = bincode::serialize(record)
                        .context("Failed to serialize vector record")?;
                    vector_data.push(Some(serialized));
                }
                WalOperation::Update { vector_id, record, expires_at: _ } => {
                    operation_types.push("UPDATE".to_string());
                    vector_ids.push(Some(vector_id.clone()));
                    
                    let serialized = bincode::serialize(record)
                        .context("Failed to serialize vector record")?;
                    vector_data.push(Some(serialized));
                }
                WalOperation::Delete { vector_id, expires_at: _ } => {
                    operation_types.push("DELETE".to_string());
                    vector_ids.push(Some(vector_id.clone()));
                    vector_data.push(None);
                }
                WalOperation::CreateCollection { collection_id: _, config: _ } => {
                    operation_types.push("CREATE_COLLECTION".to_string());
                    vector_ids.push(None);
                    vector_data.push(None);
                }
                WalOperation::DropCollection { collection_id: _ } => {
                    operation_types.push("DROP_COLLECTION".to_string());
                    vector_ids.push(None);
                    vector_data.push(None);
                }
            }
        }
        
        // Create Arrow schema
        let schema = Arc::new(Schema::new(vec![
            Field::new("entry_id", DataType::Utf8, false),
            Field::new("collection_id", DataType::Utf8, false),
            Field::new("operation_type", DataType::Utf8, false),
            Field::new("vector_id", DataType::Utf8, true),
            Field::new("vector_data", DataType::Binary, true),
            Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
            Field::new("sequence", DataType::UInt64, false),
            Field::new("global_sequence", DataType::UInt64, false),
            Field::new("version", DataType::UInt64, false),
        ]));
        
        // Create Arrow arrays
        let entry_id_array: ArrayRef = Arc::new(StringArray::from(entry_ids));
        let collection_id_array: ArrayRef = Arc::new(StringArray::from(collection_ids));
        let operation_type_array: ArrayRef = Arc::new(StringArray::from(operation_types));
        
        let vector_id_array: ArrayRef = Arc::new(StringArray::from(
            vector_ids.into_iter().map(|v| v.as_deref()).collect::<Vec<_>>()
        ));
        
        let vector_data_array: ArrayRef = Arc::new(BinaryArray::from(
            vector_data.into_iter().map(|v| v.as_deref()).collect::<Vec<_>>()
        ));
        
        let timestamp_array: ArrayRef = Arc::new(TimestampMillisecondArray::from(timestamps));
        let sequence_array: ArrayRef = Arc::new(UInt64Array::from(sequences));
        let global_sequence_array: ArrayRef = Arc::new(UInt64Array::from(global_sequences));
        let version_array: ArrayRef = Arc::new(UInt64Array::from(versions));
        
        let record_batch = RecordBatch::try_new(
            schema,
            vec![
                entry_id_array,
                collection_id_array,
                operation_type_array,
                vector_id_array,
                vector_data_array,
                timestamp_array,
                sequence_array,
                global_sequence_array,
                version_array,
            ],
        ).context("Failed to create Arrow RecordBatch")?;
        
        Ok(record_batch)
    }
    
    /// Write RecordBatch to Parquet file with optimized properties
    async fn write_parquet(&self, batch: &RecordBatch, output_path: &Path) -> Result<u64> {
        // Create optimized writer properties
        let props = WriterProperties::builder()
            .set_compression(self.config.compression)
            .set_dictionary_enabled(self.config.enable_dictionary_encoding)
            .set_data_page_size_limit(self.config.page_size)
            .set_max_row_group_size(self.config.row_group_size)
            .set_encoding(Encoding::DELTA_BINARY_PACKED)
            .build();
        
        // Create temporary file for writing
        let temp_path = output_path.with_extension("tmp");
        let file = std::fs::File::create(&temp_path)
            .with_context(|| format!("Failed to create temporary Parquet file: {:?}", temp_path))?;
        
        // Write to Parquet
        let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))
            .context("Failed to create Parquet writer")?;
        
        writer.write(batch)
            .context("Failed to write RecordBatch to Parquet")?;
        
        writer.close()
            .context("Failed to close Parquet writer")?;
        
        // Get file size
        let metadata = std::fs::metadata(&temp_path)
            .context("Failed to get Parquet file metadata")?;
        let bytes_written = metadata.len();
        
        // Move temp file to final location
        std::fs::rename(&temp_path, output_path)
            .with_context(|| format!("Failed to move Parquet file to final location: {:?}", output_path))?;
        
        tracing::debug!("📦 Parquet file written: {} bytes to {:?}", bytes_written, output_path);
        
        Ok(bytes_written)
    }
    
    /// Read Parquet file back to WAL entries (for recovery/testing)
    pub async fn read_from_parquet(&self, parquet_path: &Path) -> Result<Vec<WalEntry>> {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        
        let file = std::fs::File::open(parquet_path)
            .with_context(|| format!("Failed to open Parquet file: {:?}", parquet_path))?;
        
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)
            .context("Failed to create Parquet reader")?;
        
        let reader = builder.build()
            .context("Failed to build Parquet reader")?;
        
        let mut entries = Vec::new();
        
        for batch_result in reader {
            let batch = batch_result.context("Failed to read Parquet batch")?;
            
            // Convert Arrow batch back to WAL entries
            let batch_entries = self.convert_arrow_batch_to_entries(&batch)?;
            entries.extend(batch_entries);
        }
        
        tracing::debug!("📖 Read {} entries from Parquet file: {:?}", entries.len(), parquet_path);
        
        Ok(entries)
    }
    
    /// Convert Arrow RecordBatch back to WAL entries
    fn convert_arrow_batch_to_entries(&self, batch: &RecordBatch) -> Result<Vec<WalEntry>> {
        // This is a simplified implementation - in production you'd want more robust conversion
        // that handles all the edge cases and type conversions properly
        
        let num_rows = batch.num_rows();
        let mut entries = Vec::with_capacity(num_rows);
        
        // Extract columns
        let entry_ids = batch.column(0).as_any().downcast_ref::<StringArray>()
            .context("Failed to extract entry_id column")?;
        let collection_ids = batch.column(1).as_any().downcast_ref::<StringArray>()
            .context("Failed to extract collection_id column")?;
        let operation_types = batch.column(2).as_any().downcast_ref::<StringArray>()
            .context("Failed to extract operation_type column")?;
        
        // For now, return empty vector as full implementation would be quite complex
        // This is mainly for testing/verification purposes
        tracing::debug!("🔄 Converting {} rows from Parquet (simplified implementation)", num_rows);
        
        Ok(entries)
    }
}

/// VIPER statistics for monitoring performance
#[derive(Debug, Clone)]
pub struct ViperStats {
    pub total_entries_clustered: u64,
    pub clusters_created: usize,
    pub clustering_time_ms: u64,
    pub parquet_write_time_ms: u64,
    pub compression_ratio: f64,
    pub avg_vectors_per_cluster: f64,
}

impl Default for ViperStats {
    fn default() -> Self {
        Self {
            total_entries_clustered: 0,
            clusters_created: 0,
            clustering_time_ms: 0,
            parquet_write_time_ms: 0,
            compression_ratio: 1.0,
            avg_vectors_per_cluster: 0.0,
        }
    }
}

// =================================================================================================
// DESIGN PATTERNS FOR FLEXIBLE, FUTURE-PROOF VIPER SCHEMA ARCHITECTURE
// =================================================================================================

/// Strategy Pattern: Defines different schema generation strategies
pub trait SchemaGenerationStrategy {
    fn generate_schema(&self) -> Result<Arc<Schema>>;
    fn get_filterable_fields(&self) -> &[String];
    fn get_collection_id(&self) -> &CollectionId;
    fn supports_ttl(&self) -> bool;
    fn get_version(&self) -> u32;
}

/// Template Method Pattern: Defines the algorithm for vector record processing
pub trait VectorProcessor {
    fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()>;
    fn convert_to_batch(&self, records: &[VectorRecord], schema: &Arc<Schema>) -> Result<RecordBatch>;
    fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch>;
}

/// Builder Pattern: For complex schema configuration
#[derive(Debug)]
pub struct ViperSchemaBuilder {
    filterable_fields: Vec<String>,
    enable_ttl: bool,
    enable_extra_meta: bool,
    vector_dimension: Option<usize>,
    schema_version: u32,
    compression_level: Option<i32>,
}

impl ViperSchemaBuilder {
    pub fn new() -> Self {
        Self {
            filterable_fields: Vec::new(),
            enable_ttl: false,
            enable_extra_meta: true,
            vector_dimension: None,
            schema_version: 1,
            compression_level: None,
        }
    }
    
    pub fn with_filterable_fields(mut self, fields: Vec<String>) -> Self {
        self.filterable_fields = fields;
        self
    }
    
    pub fn with_ttl(mut self, enable: bool) -> Self {
        self.enable_ttl = enable;
        self
    }
    
    pub fn with_extra_meta(mut self, enable: bool) -> Self {
        self.enable_extra_meta = enable;
        self
    }
    
    pub fn with_vector_dimension(mut self, dim: usize) -> Self {
        self.vector_dimension = Some(dim);
        self
    }
    
    pub fn with_schema_version(mut self, version: u32) -> Self {
        self.schema_version = version;
        self
    }
    
    pub fn build(self, collection_config: &CollectionConfig) -> ViperSchemaStrategy {
        ViperSchemaStrategy::from_builder(self, collection_config)
    }
}

/// Concrete Strategy Implementation for VIPER schema generation
#[derive(Debug, Clone)]
pub struct ViperSchemaStrategy {
    collection_config: CollectionConfig,
    schema_version: u32,
    enable_ttl: bool,
    enable_extra_meta: bool,
}

impl ViperSchemaStrategy {
    pub fn new(collection_config: &CollectionConfig) -> Self {
        Self {
            collection_config: collection_config.clone(),
            schema_version: 1,
            enable_ttl: true,
            enable_extra_meta: true,
        }
    }
    
    fn from_builder(builder: ViperSchemaBuilder, collection_config: &CollectionConfig) -> Self {
        Self {
            collection_config: collection_config.clone(),
            schema_version: builder.schema_version,
            enable_ttl: builder.enable_ttl,
            enable_extra_meta: builder.enable_extra_meta,
        }
    }
}

impl SchemaGenerationStrategy for ViperSchemaStrategy {
    fn generate_schema(&self) -> Result<Arc<Schema>> {
        let mut fields = Vec::new();
        
        // Core required fields
        fields.push(Field::new("id", DataType::Utf8, false));
        fields.push(Field::new("vectors", DataType::List(Arc::new(Field::new("item", DataType::Float32, false))), false));
        
        // Dynamic filterable metadata columns (up to 16)
        for (i, field_name) in self.collection_config.filterable_metadata_fields.iter().enumerate() {
            if i >= 16 { break; } // Safety check
            fields.push(Field::new(
                &format!("meta_{}", field_name), 
                DataType::Utf8, 
                true
            ));
        }
        
        // Extra metadata as Map type for unlimited additional fields
        if self.enable_extra_meta {
            let map_field = Field::new(
                "extra_meta",
                DataType::Map(
                    Arc::new(Field::new(
                        "entries",
                        DataType::Struct(vec![
                            Field::new("key", DataType::Utf8, false),
                            Field::new("value", DataType::Utf8, true),
                        ].into()),
                        false,
                    )),
                    false,
                ),
                true,
            );
            fields.push(map_field);
        }
        
        // TTL support
        if self.enable_ttl {
            fields.push(Field::new("expires_at", DataType::Timestamp(TimeUnit::Millisecond, None), true));
        }
        
        // Metadata fields for optimization
        fields.push(Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false));
        fields.push(Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false));
        
        Ok(Arc::new(Schema::new(fields)))
    }
    
    fn get_filterable_fields(&self) -> &[String] {
        &self.collection_config.filterable_metadata_fields
    }
    
    fn get_collection_id(&self) -> &CollectionId {
        &self.collection_config.collection_id
    }
    
    fn supports_ttl(&self) -> bool {
        self.enable_ttl
    }
    
    fn get_version(&self) -> u32 {
        self.schema_version
    }
}

/// Adapter Pattern: Adapts vector records to different schema formats
pub struct VectorRecordSchemaAdapter<'a> {
    strategy: &'a dyn SchemaGenerationStrategy,
}

impl<'a> VectorRecordSchemaAdapter<'a> {
    pub fn new(strategy: &'a dyn SchemaGenerationStrategy) -> Self {
        Self { strategy }
    }
    
    pub fn adapt_records_to_schema(&self, records: &[VectorRecord], schema: &Arc<Schema>) -> Result<RecordBatch> {
        let mut id_builder = arrow::array::StringBuilder::new();
        let mut vectors_builder = arrow::array::ListBuilder::new(arrow::array::Float32Builder::new());
        
        // Dynamic metadata builders for filterable fields
        let mut meta_builders: HashMap<String, arrow::array::StringBuilder> = HashMap::new();
        for field_name in self.strategy.get_filterable_fields() {
            meta_builders.insert(field_name.clone(), arrow::array::StringBuilder::new());
        }
        
        // Extra metadata and timestamp builders
        let mut extra_meta_builder = arrow::array::MapBuilder::new(None, 
            arrow::array::StringBuilder::new(), 
            arrow::array::StringBuilder::new()
        );
        let mut expires_at_builder = arrow::array::TimestampMillisecondBuilder::new();
        let mut created_at_builder = arrow::array::TimestampMillisecondBuilder::new();
        let mut updated_at_builder = arrow::array::TimestampMillisecondBuilder::new();
        
        for record in records {
            // ID
            id_builder.append_value(&record.id);
            
            // Vectors
            for &val in &record.vector {
                vectors_builder.values().append_value(val);
            }
            vectors_builder.append(true);
            
            // Filterable metadata fields
            for field_name in self.strategy.get_filterable_fields() {
                let builder = meta_builders.get_mut(field_name).unwrap();
                if let Some(value) = record.metadata.get(field_name) {
                    builder.append_value(&value.to_string());
                } else {
                    builder.append_null();
                }
            }
            
            // Extra metadata (non-filterable fields)
            let filterable_set: std::collections::HashSet<_> = self.strategy.get_filterable_fields().iter().collect();
            for (key, value) in &record.metadata {
                if !filterable_set.contains(key) {
                    extra_meta_builder.keys().append_value(key);
                    extra_meta_builder.values().append_value(&value.to_string());
                }
            }
            extra_meta_builder.append(true);
            
            // Timestamps
            if let Some(expires_at) = record.expires_at {
                expires_at_builder.append_value(expires_at.timestamp_millis());
            } else {
                expires_at_builder.append_null();
            }
            created_at_builder.append_value(record.timestamp.timestamp_millis());
            updated_at_builder.append_value(record.timestamp.timestamp_millis());
        }
        
        // Build arrays
        let mut arrays: Vec<ArrayRef> = Vec::new();
        arrays.push(Arc::new(id_builder.finish()));
        arrays.push(Arc::new(vectors_builder.finish()));
        
        // Add filterable metadata arrays
        for field_name in self.strategy.get_filterable_fields() {
            let builder = meta_builders.remove(field_name).unwrap();
            arrays.push(Arc::new(builder.finish()));
        }
        
        // Add extra metadata and timestamps
        arrays.push(Arc::new(extra_meta_builder.finish()));
        arrays.push(Arc::new(expires_at_builder.finish()));
        arrays.push(Arc::new(created_at_builder.finish()));
        arrays.push(Arc::new(updated_at_builder.finish()));
        
        RecordBatch::try_new(schema.clone(), arrays)
            .context("Failed to create RecordBatch from adapted records")
    }
}

/// Template Method Pattern implementation for vector processing
pub struct VectorRecordProcessor<'a> {
    strategy: &'a dyn SchemaGenerationStrategy,
}

impl<'a> VectorRecordProcessor<'a> {
    pub fn new(strategy: &'a dyn SchemaGenerationStrategy) -> Self {
        Self { strategy }
    }
    
    /// Template method defining the processing algorithm
    pub async fn flush_to_parquet(
        &self,
        flusher: &ViperParquetFlusher,
        mut records: Vec<VectorRecord>,
        output_path: &Path,
    ) -> Result<FlushResult> {
        if records.is_empty() {
            return Ok(FlushResult {
                entries_flushed: 0,
                bytes_written: 0,
                segments_created: 0,
                collections_affected: vec![],
                flush_duration_ms: 0,
            });
        }
        
        let start_time = std::time::Instant::now();
        
        tracing::info!("🎯 Processing {} vector records with VIPER strategy for collection '{}'", 
                      records.len(), self.strategy.get_collection_id());
        
        // Step 1: Preprocess (Template Method hook)
        self.preprocess_records(&mut records)?;
        
        // Step 2: Generate schema
        let schema = self.strategy.generate_schema()?;
        
        // Step 3: Convert to batch (Template Method hook)
        let record_batch = self.convert_to_batch(&records, &schema)?;
        
        // Step 4: Postprocess (Template Method hook)
        let final_batch = self.postprocess_batch(record_batch)?;
        
        // Step 5: Write to Parquet
        let bytes_written = flusher.write_parquet(&final_batch, output_path).await?;
        
        let flush_duration = start_time.elapsed().as_millis() as u64;
        
        tracing::info!("✅ VIPER vector processing completed: {} records, {} bytes, {}ms", 
                      records.len(), bytes_written, flush_duration);
        
        Ok(FlushResult {
            entries_flushed: records.len() as u64,
            bytes_written,
            segments_created: 1,
            collections_affected: vec![self.strategy.get_collection_id().clone()],
            flush_duration_ms: flush_duration,
        })
    }
}

impl<'a> VectorProcessor for VectorRecordProcessor<'a> {
    fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()> {
        // Default preprocessing: Sort by timestamp for better compression
        records.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        
        tracing::debug!("🔧 Preprocessed {} records (sorted by timestamp)", records.len());
        Ok(())
    }
    
    fn convert_to_batch(&self, records: &[VectorRecord], schema: &Arc<Schema>) -> Result<RecordBatch> {
        let adapter = VectorRecordSchemaAdapter::new(self.strategy);
        adapter.adapt_records_to_schema(records, schema)
    }
    
    fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch> {
        // Default postprocessing: Return as-is
        // Future extensions could add compression hints, column reordering, etc.
        Ok(batch)
    }
}

/// Factory Pattern: Creates appropriate schema strategies
pub struct ViperSchemaFactory;

impl ViperSchemaFactory {
    pub fn create_strategy(collection_config: &CollectionConfig) -> Box<dyn SchemaGenerationStrategy> {
        // Future: Could create different strategies based on collection characteristics
        // For now, always return VIPER strategy
        Box::new(ViperSchemaStrategy::new(collection_config))
    }
    
    pub fn create_strategy_with_builder(
        builder: ViperSchemaBuilder, 
        collection_config: &CollectionConfig
    ) -> Box<dyn SchemaGenerationStrategy> {
        Box::new(builder.build(collection_config))
    }
}
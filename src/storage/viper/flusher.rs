//! Core VIPER Parquet Flushing Implementation
//! 
//! This module contains the main ViperParquetFlusher implementation
//! with core Parquet writing functionality and clustering capabilities.

use anyhow::{Result, Context};
use arrow::array::RecordBatch;
use arrow::datatypes::Schema;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::{Compression, Encoding};
use std::sync::Arc;

use crate::core::{CollectionId, VectorRecord};
use crate::storage::filesystem::FilesystemFactory;
use crate::storage::unified_engine::CollectionConfig;
use crate::storage::{WalEntry, WalOperation};
use super::config::ViperConfig;
use super::schema::ViperSchemaStrategy;
use super::processor::VectorRecordProcessor;

/// Core flush result information
#[derive(Debug, Clone)]
pub struct FlushResult {
    pub entries_flushed: u64,
    pub bytes_written: u64,
    pub segments_created: u64,
    pub collections_affected: Vec<CollectionId>,
    pub flush_duration_ms: u64,
}

/// Core VIPER Parquet flusher implementation
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
    
    /// Flush vector records to Parquet with dynamic schema - Strategy Pattern entry point
    pub async fn flush_vectors_to_parquet(
        &self,
        collection_config: &CollectionConfig,
        records: Vec<VectorRecord>,
        output_url: &str,
    ) -> Result<FlushResult> {
        let strategy = ViperSchemaStrategy::new(collection_config);
        let processor = VectorRecordProcessor::new(&strategy);
        
        processor.flush_to_parquet(self, records, output_url).await
    }
    
    /// Write RecordBatch to Parquet file using filesystem API
    pub async fn write_parquet(&self, batch: &RecordBatch, output_url: &str) -> Result<u64> {
        // Create optimized writer properties
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(parquet::basic::ZstdLevel::try_new(3)?))
            .set_encoding(Encoding::DELTA_BINARY_PACKED)
            .set_dictionary_enabled(self.config.enable_dictionary_encoding)
            .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Page)
            .set_max_row_group_size(self.config.row_group_size)
            .set_write_batch_size(8192)
            .build();
        
        // Serialize RecordBatch to bytes
        let mut buffer = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), Some(props))
                .context("Failed to create Parquet writer")?;
            
            writer.write(batch)
                .context("Failed to write RecordBatch to Parquet")?;
            
            writer.close()
                .context("Failed to close Parquet writer")?;
        }
        
        let bytes_written = buffer.len() as u64;
        
        // Write using filesystem API
        self.filesystem.write(output_url, &buffer, None).await
            .with_context(|| format!("Failed to write Parquet file to: {}", output_url))?;
        
        tracing::debug!("📦 Parquet file written: {} bytes to {}", bytes_written, output_url);
        
        Ok(bytes_written)
    }
    
    /// Legacy method for WAL entries (for backward compatibility)
    pub async fn flush_to_parquet(
        &self,
        collection_id: &CollectionId,
        entries: Vec<WalEntry>,
        output_url: &str,
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
        let bytes_written = self.write_parquet(&record_batch, output_url).await?;
        
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
                    // Non-vector operations go to cluster 0
                    vector_entries.push(entry.clone());
                }
            }
        }
        
        if vectors.is_empty() {
            tracing::debug!("No vectors to cluster");
            return Ok(entries.to_vec());
        }
        
        // Determine cluster count
        let cluster_count = if self.config.cluster_count > 0 {
            self.config.cluster_count.min(vectors.len())
        } else {
            // Auto-detect: one cluster per 10k vectors, minimum 2, maximum 16
            (vectors.len() / 10_000).clamp(2, 16)
        };
        
        if cluster_count <= 1 {
            tracing::debug!("Not enough vectors for clustering");
            return Ok(entries.to_vec());
        }
        
        // Perform K-means clustering
        let cluster_assignments = self.assign_to_collection(&vectors)?;
        
        // Group entries by cluster
        let mut cluster_groups: std::collections::HashMap<usize, Vec<WalEntry>> = std::collections::HashMap::new();
        for (i, entry) in vector_entries.into_iter().enumerate() {
            let cluster_id = cluster_assignments.get(i).copied().unwrap_or(0);
            cluster_groups.entry(cluster_id).or_insert_with(Vec::new).push(entry);
        }
        
        let mut clustered_entries = Vec::new();
        
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
    
    /// Simple assignment to single collection directory (no clustering for MVP)
    fn assign_to_collection(&self, vectors: &[Vec<f32>]) -> Result<Vec<usize>> {
        // For MVP: all vectors go to cluster 0 (single collection directory)
        Ok(vec![0; vectors.len()])
    }
    
    /// Convert WAL entries to Arrow RecordBatch (legacy support)
    fn convert_to_arrow_batch(&self, entries: &[WalEntry]) -> Result<RecordBatch> {
        use arrow::array::{ArrayRef, StringArray, UInt64Array, BinaryArray, TimestampMillisecondArray};
        use arrow::datatypes::{DataType, Field, TimeUnit};
        
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
        
        let vector_id_array: ArrayRef = Arc::new(StringArray::from(vector_ids));
        
        // Convert Vec<Option<Vec<u8>>> to Vec<Option<&[u8]>> for BinaryArray
        let vector_data_refs: Vec<Option<&[u8]>> = vector_data.iter()
            .map(|v| v.as_deref())
            .collect();
        let vector_data_array: ArrayRef = Arc::new(BinaryArray::from(vector_data_refs));
        
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
}
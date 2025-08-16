//! Unified Parquet Writer for NOVA and VIPER engines
//! 
//! Provides optimized Parquet writing with:
//! - Built-in bloom filters for efficient lookups
//! - ID-less storage using row group offsets as implicit IDs
//! - Streaming write support for large datasets
//! - Quantization-aware schema generation

use anyhow::{anyhow, Context, Result};
use arrow_array::{
    Array, ArrayRef, BinaryArray, FixedSizeBinaryArray, Float32Array, Int64Array, 
    RecordBatch, StringArray, UInt32Array
};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::{ArrowWriter, ProjectionMask};
use parquet::basic::{Compression, Encoding};
use parquet::bloom_filter::BloomFilter as ParquetBloomFilter;
use parquet::file::properties::{WriterProperties, WriterPropertiesBuilder};
use parquet::schema::types::Type;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, trace};

use crate::core::VectorRecord;
use crate::core::compression::CompressionAlgorithm;
use crate::storage::engines::columnar::{ColumnarConfig, QuantizationConfig};

/// Configuration for Parquet writing
#[derive(Debug, Clone)]
pub struct ParquetWriterConfig {
    /// Row group size (number of records)
    pub row_group_size: usize,
    
    /// Enable bloom filters for ID columns
    pub enable_bloom_filters: bool,
    
    /// Bloom filter FPP (false positive probability)
    pub bloom_filter_fpp: f64,
    
    /// Compression algorithm
    pub compression: CompressionAlgorithm,
    
    /// Enable dictionary encoding for string columns
    pub enable_dictionary: bool,
    
    /// Enable delta encoding for integer columns
    pub enable_delta_encoding: bool,
    
    /// Quantization configuration
    pub quantization: QuantizationConfig,
    
    /// Store vectors without explicit IDs (use row group offset + row index)
    /// WARNING: Disabling this removes customer ID column and breaks ID-based APIs
    pub id_less_storage: bool,
    
    /// Write batch size for streaming
    pub write_batch_size: usize,
}

impl Default for ParquetWriterConfig {
    fn default() -> Self {
        Self {
            row_group_size: 10000,
            enable_bloom_filters: true,
            bloom_filter_fpp: 0.01,
            compression: CompressionAlgorithm::Mixed, // Use Mixed as the recommended default
            enable_dictionary: true,
            enable_delta_encoding: true,
            quantization: QuantizationConfig::default(),
            id_less_storage: false, // KEEP ID COLUMN FOR CUSTOMER APIs
            write_batch_size: 1000,
        }
    }
}

/// Streaming Parquet writer optimized for columnar engines
pub struct StreamingParquetWriter {
    writer: ArrowWriter<File>,
    config: ParquetWriterConfig,
    schema: Arc<Schema>,
    current_batch: Vec<VectorRecord>,
    current_row_group: usize,
    total_records_written: u64,
    /// ID bloom filters per row group for fast ID lookups
    id_bloom_filters: Vec<crate::storage::engines::columnar::id_index::BloomFilter>,
    
    /// Metadata bloom filters for other columns
    metadata_bloom_filters: HashMap<String, crate::storage::engines::columnar::id_index::BloomFilter>,
    file_path: String,
}

impl StreamingParquetWriter {
    /// Create new streaming writer
    pub fn new<P: AsRef<Path>>(
        file_path: P,
        dimension: usize,
        config: ParquetWriterConfig,
    ) -> Result<Self> {
        let file_path_str = file_path.as_ref().to_string_lossy().to_string();
        info!("Creating streaming Parquet writer: {}", file_path_str);
        
        // Create optimized schema
        let schema = Self::create_optimized_schema(dimension, &config)?;
        
        // Create writer properties with optimizations
        let props = Self::create_writer_properties(&config)?;
        
        // Open file and create writer
        let file = File::create(&file_path)?;
        let writer = ArrowWriter::try_new(file, schema.clone(), Some(props))?;
        
        Ok(Self {
            writer,
            config,
            schema,
            current_batch: Vec::new(),
            current_row_group: 0,
            total_records_written: 0,
            id_bloom_filters: Vec::new(),
            metadata_bloom_filters: HashMap::new(),
            file_path: file_path_str,
        })
    }
    
    /// Create optimized Parquet schema
    fn create_optimized_schema(
        dimension: usize,
        config: &ParquetWriterConfig,
    ) -> Result<Arc<Schema>> {
        let mut fields = Vec::new();
        
        // Core fields - ID column is REQUIRED for customer APIs
        fields.push(Field::new("id", DataType::Utf8, false)); // NOT NULL
        
        // Optional: Row group offset for internal optimizations (when id_less_storage is enabled)
        if config.id_less_storage {
            debug!("ID-less storage enabled - adding offset columns for internal optimization");
        }
        
        // Row group offset (implicit ID for ID-less storage)
        fields.push(Field::new("row_group_offset", DataType::UInt32, false));
        fields.push(Field::new("row_index", DataType::UInt32, false));
        
        // Vector data (FP32)
        fields.push(Field::new(
            "vector_fp32",
            DataType::FixedSizeBinary(dimension as i32 * 4),
            false,
        ));
        
        // Quantized vectors
        if config.quantization.enable_binary {
            let binary_size = (dimension + 7) / 8;
            fields.push(Field::new(
                "vector_binary",
                DataType::FixedSizeBinary(binary_size as i32),
                true,
            ));
        }
        
        if config.quantization.enable_int8 {
            fields.push(Field::new(
                "vector_int8",
                DataType::FixedSizeBinary(dimension as i32),
                true,
            ));
            fields.push(Field::new("int8_scale", DataType::Float32, true));
            fields.push(Field::new("int8_zero_point", DataType::Int8, true));
        }
        
        if config.quantization.enable_pq {
            fields.push(Field::new(
                "vector_pq",
                DataType::FixedSizeBinary(config.quantization.pq_segments as i32),
                true,
            ));
            fields.push(Field::new(
                "pq_codebook",
                DataType::Binary,
                true,
            ));
        }
        
        // Metadata fields
        fields.push(Field::new("timestamp", DataType::Int64, false));
        fields.push(Field::new("version", DataType::Int64, true));
        fields.push(Field::new("metadata_json", DataType::Utf8, true));
        
        Ok(Arc::new(Schema::new(fields)))
    }
    
    /// Create writer properties with optimizations
    fn create_writer_properties(config: &ParquetWriterConfig) -> Result<WriterProperties> {
        let mut builder = WriterPropertiesBuilder::new()
            .set_max_row_group_size(config.row_group_size)
            .set_write_batch_size(config.write_batch_size);
        
        // Set compression
        let compression = match config.compression {
            CompressionAlgorithm::Zstd => Compression::ZSTD(parquet::basic::ZstdLevel::default()),
            CompressionAlgorithm::Lz4 => Compression::LZ4,
            CompressionAlgorithm::Snappy => Compression::SNAPPY,
            CompressionAlgorithm::Gzip => Compression::GZIP(parquet::basic::GzipLevel::default()),
            CompressionAlgorithm::Brotli => Compression::BROTLI(parquet::basic::BrotliLevel::default()),
            CompressionAlgorithm::Mixed => {
                // Mixed compression strategy: Use ZSTD level 3 as default
                // Per-column optimization will be applied at writer level
                info!("🎯 Columnar Parquet Writer: Using Mixed compression strategy");
                Compression::ZSTD(parquet::basic::ZstdLevel::try_new(3).unwrap_or_default())
            },
            _ => Compression::ZSTD(parquet::basic::ZstdLevel::default()),
        };
        
        builder = builder.set_compression(compression);
        
        // Apply Mixed compression per-column optimization
        if matches!(config.compression, CompressionAlgorithm::Mixed) {
            builder = Self::apply_mixed_compression_optimization(builder, schema)?;
        }
        
        // Enable dictionary encoding for string columns
        if config.enable_dictionary {
            builder = builder.set_dictionary_enabled(true);
        }
        
        // Enable bloom filters
        if config.enable_bloom_filters {
            // Enable bloom filters for ID and metadata columns
            builder = builder.set_bloom_filter_enabled(true);
        }
        
        Ok(builder.build())
    }
    
    /// Write a batch of records (streaming interface)
    pub async fn write_batch(&mut self, records: &[VectorRecord]) -> Result<()> {
        for record in records {
            self.current_batch.push(record.clone());
            
            // Flush when batch is full
            if self.current_batch.len() >= self.config.write_batch_size {
                self.flush_current_batch().await?;
            }
        }
        
        Ok(())
    }
    
    /// Write a single record
    pub async fn write_record(&mut self, record: VectorRecord) -> Result<()> {
        self.current_batch.push(record);
        
        if self.current_batch.len() >= self.config.write_batch_size {
            self.flush_current_batch().await?;
        }
        
        Ok(())
    }
    
    /// Flush current batch to Parquet
    async fn flush_current_batch(&mut self) -> Result<()> {
        if self.current_batch.is_empty() {
            return Ok(());
        }
        
        trace!("Flushing batch of {} records", self.current_batch.len());
        
        // Convert records to Arrow RecordBatch
        let batch = self.create_record_batch(&self.current_batch)?;
        
        // Update bloom filters
        if self.config.enable_bloom_filters {
            self.update_bloom_filters(&self.current_batch)?;
        }
        
        // Write to Parquet
        self.writer.write(&batch)?;
        
        self.total_records_written += self.current_batch.len() as u64;
        
        // Clear batch
        self.current_batch.clear();
        
        debug!("Flushed batch, total records: {}", self.total_records_written);
        Ok(())
    }
    
    /// Convert VectorRecords to Arrow RecordBatch
    fn create_record_batch(&self, records: &[VectorRecord]) -> Result<RecordBatch> {
        let num_records = records.len();
        let mut arrays: Vec<ArrayRef> = Vec::new();
        
        // ID column (ALWAYS REQUIRED for customer APIs)
        let ids: Vec<Option<String>> = records.iter()
            .map(|r| r.id.clone())
            .collect();
        arrays.push(Arc::new(StringArray::from(ids)));
        
        // Row group offset and row index (for ID-less storage)
        let row_group_offsets: Vec<u32> = (0..num_records)
            .map(|_| self.current_row_group as u32)
            .collect();
        let row_indices: Vec<u32> = (0..num_records as u32).collect();
        
        arrays.push(Arc::new(UInt32Array::from(row_group_offsets)));
        arrays.push(Arc::new(UInt32Array::from(row_indices)));
        
        // Vector data (FP32)
        let vectors = self.create_vector_array(records)?;
        arrays.push(vectors);
        
        // Quantized vectors
        if self.config.quantization.enable_binary {
            let binary_vectors = self.create_binary_vector_array(records)?;
            arrays.push(binary_vectors);
        }
        
        if self.config.quantization.enable_int8 {
            let (int8_vectors, scales, zero_points) = self.create_int8_vector_arrays(records)?;
            arrays.push(int8_vectors);
            arrays.push(scales);
            arrays.push(zero_points);
        }
        
        if self.config.quantization.enable_pq {
            let (pq_vectors, codebooks) = self.create_pq_vector_arrays(records)?;
            arrays.push(pq_vectors);
            arrays.push(codebooks);
        }
        
        // Metadata fields
        let timestamps: Vec<i64> = records.iter()
            .map(|r| r.timestamp as i64)
            .collect();
        arrays.push(Arc::new(Int64Array::from(timestamps)));
        
        let versions: Vec<Option<i64>> = records.iter()
            .map(|r| r.version.map(|v| v as i64))
            .collect();
        arrays.push(Arc::new(Int64Array::from(versions)));
        
        let metadata: Vec<Option<String>> = records.iter()
            .map(|r| r.metadata.as_ref().map(|m| serde_json::to_string(m).unwrap_or_default()))
            .collect();
        arrays.push(Arc::new(StringArray::from(metadata)));
        
        RecordBatch::try_new(self.schema.clone(), arrays)
            .context("Failed to create RecordBatch")
    }
    
    /// Create FP32 vector array
    fn create_vector_array(&self, records: &[VectorRecord]) -> Result<ArrayRef> {
        let mut values = Vec::new();
        
        for record in records {
            // Convert f32 vector to bytes
            let bytes = record.vector.iter()
                .flat_map(|f| f.to_le_bytes())
                .collect::<Vec<u8>>();
            values.push(Some(bytes));
        }
        
        Ok(Arc::new(FixedSizeBinaryArray::try_from_iter(values.into_iter())?))
    }
    
    /// Create binary quantized vector array
    fn create_binary_vector_array(&self, records: &[VectorRecord]) -> Result<ArrayRef> {
        let mut values = Vec::new();
        
        for record in records {
            // Binary quantization: threshold at 0.0
            let mut bits = Vec::new();
            let mut current_byte = 0u8;
            let mut bit_pos = 0;
            
            for &value in &record.vector {
                if value > 0.0 {
                    current_byte |= 1 << bit_pos;
                }
                bit_pos += 1;
                
                if bit_pos == 8 {
                    bits.push(current_byte);
                    current_byte = 0;
                    bit_pos = 0;
                }
            }
            
            // Handle remaining bits
            if bit_pos > 0 {
                bits.push(current_byte);
            }
            
            values.push(Some(bits));
        }
        
        Ok(Arc::new(FixedSizeBinaryArray::try_from_iter(values.into_iter())?))
    }
    
    /// Create INT8 quantized vector arrays
    fn create_int8_vector_arrays(&self, records: &[VectorRecord]) -> Result<(ArrayRef, ArrayRef, ArrayRef)> {
        let mut vectors = Vec::new();
        let mut scales = Vec::new();
        let mut zero_points = Vec::new();
        
        for record in records {
            // Find min/max for quantization
            let min_val = record.vector.iter().cloned().fold(f32::INFINITY, f32::min);
            let max_val = record.vector.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            
            let scale = (max_val - min_val) / 255.0;
            let zero_point = (-min_val / scale).round() as i8;
            
            // Quantize to INT8
            let quantized: Vec<u8> = record.vector.iter()
                .map(|&v| ((v / scale) + zero_point as f32).round().clamp(0.0, 255.0) as u8)
                .collect();
            
            vectors.push(Some(quantized));
            scales.push(Some(scale));
            zero_points.push(Some(zero_point));
        }
        
        let vector_array = Arc::new(FixedSizeBinaryArray::try_from_iter(vectors.into_iter())?);
        let scale_array = Arc::new(Float32Array::from(scales));
        let zero_point_array = Arc::new(arrow_array::Int8Array::from(zero_points));
        
        Ok((vector_array, scale_array, zero_point_array))
    }
    
    /// Create Product Quantization vector arrays
    fn create_pq_vector_arrays(&self, records: &[VectorRecord]) -> Result<(ArrayRef, ArrayRef)> {
        let mut pq_codes = Vec::new();
        let mut codebooks = Vec::new();
        
        for record in records {
            // Simplified PQ - divide into segments and quantize each
            let segments = self.config.quantization.pq_segments as usize;
            let segment_size = record.vector.len() / segments;
            let mut codes = Vec::new();
            let mut codebook = Vec::new();
            
            for segment_idx in 0..segments {
                let start = segment_idx * segment_size;
                let end = ((segment_idx + 1) * segment_size).min(record.vector.len());
                let segment = &record.vector[start..end];
                
                // Simple quantization: find centroid
                let centroid: f32 = segment.iter().sum::<f32>() / segment.len() as f32;
                
                // Quantize segment to single code (simplified)
                let code = (centroid * 255.0).clamp(0.0, 255.0) as u8;
                codes.push(code);
                
                // Store centroid in codebook
                codebook.extend_from_slice(&centroid.to_le_bytes());
            }
            
            pq_codes.push(Some(codes));
            codebooks.push(Some(codebook));
        }
        
        let pq_array = Arc::new(FixedSizeBinaryArray::try_from_iter(pq_codes.into_iter())?);
        let codebook_array = Arc::new(BinaryArray::from_opt_vec(codebooks));
        
        Ok((pq_array, codebook_array))
    }
    
    /// Update bloom filters for efficient lookups
    fn update_bloom_filters(&mut self, records: &[VectorRecord]) -> Result<()> {
        for record in records {
            // ALWAYS add ID to bloom filter (critical for customer APIs)
            if let Some(id) = &record.id {
                self.add_to_bloom_filter("id", id)?;
            }
            
            // Add other indexable fields
            self.add_to_bloom_filter("timestamp", &record.timestamp.to_string())?;
            
            if let Some(version) = record.version {
                self.add_to_bloom_filter("version", &version.to_string())?;
            }
        }
        
        Ok(())
    }
    
    /// Add value to bloom filter
    fn add_to_bloom_filter(&mut self, column: &str, value: &str) -> Result<()> {
        if column == "id" {
            // Ensure we have a bloom filter for current row group
            if self.id_bloom_filters.len() <= self.current_row_group {
                // Estimate records per row group for bloom filter sizing
                let records_per_group = self.config.row_group_size;
                let bloom = crate::storage::engines::columnar::id_index::BloomFilter::new(
                    records_per_group, 
                    self.config.bloom_filter_fpp
                );
                self.id_bloom_filters.push(bloom);
            }
            
            // Add ID to current row group bloom filter
            if let Some(bloom) = self.id_bloom_filters.get_mut(self.current_row_group) {
                bloom.insert(value);
            }
        } else {
            // Handle metadata bloom filters
            let bloom = self.metadata_bloom_filters.entry(column.to_string())
                .or_insert_with(|| {
                    crate::storage::engines::columnar::id_index::BloomFilter::new(
                        self.config.row_group_size,
                        self.config.bloom_filter_fpp
                    )
                });
            bloom.insert(value);
        }
        
        debug!("Added to {} bloom filter: {}", column, value);
        Ok(())
    }
    
    /// Finalize and close the writer
    pub async fn finalize(mut self) -> Result<StreamingParquetWriterStats> {
        info!("Finalizing Parquet writer: {}", self.file_path);
        
        // Flush remaining records
        if !self.current_batch.is_empty() {
            self.flush_current_batch().await?;
        }
        
        // Close writer
        let metadata = self.writer.finish()?;
        
        let stats = StreamingParquetWriterStats {
            file_path: self.file_path,
            total_records: self.total_records_written,
            total_row_groups: metadata.num_row_groups(),
            file_size: metadata.file_metadata().serialized_size() as u64,
            compression_ratio: self.calculate_compression_ratio(&metadata),
            bloom_filter_count: self.id_bloom_filters.len() + self.metadata_bloom_filters.len(),
        };
        
        info!("Parquet write complete: {:?}", stats);
        Ok(stats)
    }
    
    /// Apply mixed compression strategy with per-column optimization
    fn apply_mixed_compression_optimization(
        mut builder: WriterPropertiesBuilder,
        schema: &Schema,
    ) -> Result<WriterPropertiesBuilder> {
        use crate::core::compression::{detect_column_type, get_optimal_compression_for_column, 
                                      map_to_parquet_compression, CompressionContext};
        
        info!("🎯 Columnar Parquet Writer: Applying Mixed compression to {} columns", schema.fields().len());
        
        // Apply per-column compression optimization
        for field in schema.fields() {
            let column_name = field.name();
            
            // Detect column type based on name and context
            let column_type = detect_column_type(column_name, &CompressionContext::ParquetColumn);
            
            // Get optimal compression algorithm for this column type
            let optimal_algorithm = get_optimal_compression_for_column(&column_type);
            
            // Convert to Parquet compression and apply
            if let Some(parquet_compression) = map_to_parquet_compression(&optimal_algorithm) {
                let column_path = parquet::schema::types::ColumnPath::from(column_name.as_str());
                
                debug!("🔧 Mixed compression: {} -> {:?} (type: {:?})", 
                       column_name, optimal_algorithm, column_type);
                
                // Apply per-column compression
                builder = builder.set_column_compression(column_path.clone(), parquet_compression);
                
                // Apply optimal encoding based on column type
                let encoding = match column_type {
                    crate::core::compression::ColumnDataType::BinaryQuantized => {
                        // Binary data - use bit packing for maximum density
                        parquet::basic::Encoding::BIT_PACKED
                    },
                    crate::core::compression::ColumnDataType::Int8Quantized => {
                        // Integer quantized - use delta encoding
                        parquet::basic::Encoding::DELTA_BINARY_PACKED
                    },
                    crate::core::compression::ColumnDataType::ProductQuantized |
                    crate::core::compression::ColumnDataType::FullPrecision => {
                        // Vector data - use byte stream split for floating point efficiency
                        parquet::basic::Encoding::BYTE_STREAM_SPLIT
                    },
                    crate::core::compression::ColumnDataType::Identifier |
                    crate::core::compression::ColumnDataType::Metadata => {
                        // String/metadata columns - use dictionary encoding for deduplication
                        parquet::basic::Encoding::RLE_DICTIONARY
                    },
                    crate::core::compression::ColumnDataType::Timestamp => {
                        // Timestamps - use delta encoding for monotonic values
                        parquet::basic::Encoding::DELTA_BINARY_PACKED
                    },
                    crate::core::compression::ColumnDataType::Generic => {
                        // Generic data - use plain encoding
                        parquet::basic::Encoding::PLAIN
                    },
                };
                
                builder = builder.set_column_encoding(column_path, encoding);
                
                debug!("🔧 Mixed encoding: {} -> {:?}", column_name, encoding);
            }
        }
        
        info!("✅ Mixed compression applied to all columns");
        Ok(builder)
    }

    /// Calculate compression ratio
    fn calculate_compression_ratio(&self, metadata: &parquet::file::metadata::ParquetMetaData) -> f32 {
        let mut total_uncompressed = 0u64;
        let mut total_compressed = 0u64;
        
        for row_group in metadata.row_groups() {
            total_uncompressed += row_group.total_byte_size() as u64;
            total_compressed += row_group.compressed_size() as u64;
        }
        
        if total_compressed > 0 {
            total_uncompressed as f32 / total_compressed as f32
        } else {
            1.0
        }
    }
}

/// Statistics from Parquet writing
#[derive(Debug, Clone)]
pub struct StreamingParquetWriterStats {
    pub file_path: String,
    pub total_records: u64,
    pub total_row_groups: i32,
    pub file_size: u64,
    pub compression_ratio: f32,
    pub bloom_filter_count: usize,
}

/// Batch Parquet writer for bulk operations
pub struct BatchParquetWriter {
    config: ParquetWriterConfig,
    file_path: String,
    dimension: usize,
}

impl BatchParquetWriter {
    /// Create new batch writer
    pub fn new<P: AsRef<Path>>(
        file_path: P,
        dimension: usize,
        config: ParquetWriterConfig,
    ) -> Self {
        Self {
            config,
            file_path: file_path.as_ref().to_string_lossy().to_string(),
            dimension,
        }
    }
    
    /// Write all records at once (optimized for bulk inserts)
    pub async fn write_all(&self, records: &[VectorRecord]) -> Result<StreamingParquetWriterStats> {
        info!("Batch writing {} records to {}", records.len(), self.file_path);
        
        let mut writer = StreamingParquetWriter::new(
            &self.file_path,
            self.dimension,
            self.config.clone(),
        )?;
        
        // Write in optimized batches
        let batch_size = self.config.write_batch_size;
        for chunk in records.chunks(batch_size) {
            writer.write_batch(chunk).await?;
        }
        
        writer.finalize().await
    }
}

/// ID-less vector lookup utilities
pub struct IdLessLookup;

impl IdLessLookup {
    /// Generate implicit ID from row group and row index
    pub fn generate_implicit_id(row_group: u32, row_index: u32) -> String {
        format!("rg{:06}_row{:08}", row_group, row_index)
    }
    
    /// Parse implicit ID to get row group and row index
    pub fn parse_implicit_id(implicit_id: &str) -> Result<(u32, u32)> {
        let parts: Vec<&str> = implicit_id.split('_').collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid implicit ID format: {}", implicit_id));
        }
        
        let row_group = parts[0].strip_prefix("rg")
            .ok_or_else(|| anyhow!("Invalid row group format"))?
            .parse::<u32>()?;
            
        let row_index = parts[1].strip_prefix("row")
            .ok_or_else(|| anyhow!("Invalid row index format"))?
            .parse::<u32>()?;
            
        Ok((row_group, row_index))
    }
    
    /// Create lookup index from Parquet metadata
    pub fn create_lookup_index(
        metadata: &parquet::file::metadata::ParquetMetaData,
    ) -> HashMap<String, (u32, u32)> {
        let mut index = HashMap::new();
        
        for (rg_idx, row_group) in metadata.row_groups().iter().enumerate() {
            for row_idx in 0..row_group.num_rows() {
                let implicit_id = Self::generate_implicit_id(rg_idx as u32, row_idx as u32);
                index.insert(implicit_id, (rg_idx as u32, row_idx as u32));
            }
        }
        
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    
    #[tokio::test]
    async fn test_streaming_parquet_writer() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_streaming.parquet");
        
        let config = ParquetWriterConfig {
            write_batch_size: 10,
            id_less_storage: false, // Keep ID column for API compatibility
            ..Default::default()
        };
        
        let mut writer = StreamingParquetWriter::new(&file_path, 128, config).unwrap();
        
        // Write some test records
        for i in 0..25 {
            let record = VectorRecord {
                id: Some(format!("vec_{}", i)),
                vector: (0..128).map(|j| (i + j) as f32 * 0.01).collect(),
                metadata: None,
                timestamp: i as u32,
                updated_at: None,
                expires_at: None,
                version: Some(1),
            };
            
            writer.write_record(record).await.unwrap();
        }
        
        let stats = writer.finalize().await.unwrap();
        
        assert_eq!(stats.total_records, 25);
        assert!(stats.file_size > 0);
        assert!(stats.compression_ratio > 0.0);
    }
    
    #[tokio::test]
    async fn test_batch_parquet_writer() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_batch.parquet");
        
        let config = ParquetWriterConfig::default();
        let writer = BatchParquetWriter::new(&file_path, 256, config);
        
        let records: Vec<VectorRecord> = (0..100).map(|i| VectorRecord {
            id: Some(format!("batch_vec_{}", i)),
            vector: (0..256).map(|j| (i + j) as f32 * 0.001).collect(),
            metadata: None,
            timestamp: i as u32,
            updated_at: None,
            expires_at: None,
            version: Some(1),
        }).collect();
        
        let stats = writer.write_all(&records).await.unwrap();
        
        assert_eq!(stats.total_records, 100);
        assert!(stats.file_size > 0);
    }
    
    #[test]
    fn test_id_less_lookup() {
        let implicit_id = IdLessLookup::generate_implicit_id(5, 1234);
        assert_eq!(implicit_id, "rg000005_row00001234");
        
        let (rg, row) = IdLessLookup::parse_implicit_id(&implicit_id).unwrap();
        assert_eq!(rg, 5);
        assert_eq!(row, 1234);
    }
}
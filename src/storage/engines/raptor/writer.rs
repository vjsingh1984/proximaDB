use arrow_array::RecordBatch;
use arrow_schema::Schema;
use std::sync::Arc;
use anyhow::Result;
use tokio::sync::Mutex;
use std::path::PathBuf;

use crate::core::compression::{UnifiedCompressionEngine, CompressionConfig, CompressionAlgorithm};
use crate::compute::quantization::storage_engine::{StorageQuantizationEngine, QuantizationConfig};
use crate::storage::engines::columnar::{
    parquet_writer::ParquetWriter as ColumnarWriter,
    schema_manager::SchemaManager,
    utilities::ColumnarUtilities,
};
use super::{RaptorConfig, RowGroup, RowGroupManager};

pub struct RaptorWriter {
    base_path: String,
    config: RaptorConfig,
    schema: Arc<Schema>,
    
    // Reuse unified components
    compression_engine: Arc<UnifiedCompressionEngine>,
    quantization_engine: Arc<StorageQuantizationEngine>,
    columnar_writer: Arc<Mutex<ColumnarWriter>>,
    schema_manager: Arc<SchemaManager>,
    
    // RAPTOR-specific
    current_rowgroup: Option<RowGroupBuffer>,
    rowgroup_manager: Arc<Mutex<RowGroupManager>>,
    file_offset: u64,
}

struct RowGroupBuffer {
    batch: RecordBatch,
    row_count: usize,
    start_offset: u64,
}

impl RaptorWriter {
    pub async fn new(
        base_path: String,
        config: RaptorConfig,
        schema: Arc<Schema>,
    ) -> Result<Self> {
        // Initialize unified compression engine
        let compression_config = CompressionConfig {
            algorithm: match &config.compression {
                super::config::CompressionCodec::None => CompressionAlgorithm::None,
                super::config::CompressionCodec::Lz4 => CompressionAlgorithm::Lz4,
                super::config::CompressionCodec::Zstd(level) => CompressionAlgorithm::Zstd(*level),
                super::config::CompressionCodec::Snappy => CompressionAlgorithm::Snappy,
                super::config::CompressionCodec::Gzip(level) => CompressionAlgorithm::Gzip(*level),
            },
            block_size: 64 * 1024, // 64KB blocks
            enable_dictionary: true,
        };
        let compression_engine = Arc::new(UnifiedCompressionEngine::new(compression_config)?);
        
        // Initialize unified quantization engine
        let quantization_config = QuantizationConfig::default_for_dimension(
            schema.fields()
                .iter()
                .find(|f| f.name() == "vector")
                .map(|f| {
                    // Extract dimension from vector field
                    // This is simplified - actual implementation would parse field metadata
                    768
                })
                .unwrap_or(768)
        );
        let quantization_engine = Arc::new(StorageQuantizationEngine::new(quantization_config)?);
        
        // Initialize columnar writer using shared columnar infrastructure
        let columnar_config = crate::storage::engines::columnar::config::ColumnarConfig {
            enable_statistics: config.enable_statistics,
            enable_bloom_filter: config.enable_bloom_filters,
            compression: compression_config.algorithm.clone(),
            row_group_size: config.rowgroup_size,
            enable_dictionary: true,
            dictionary_page_size: 1024 * 1024,
            data_page_size: 64 * 1024,
        };
        
        let schema_manager = Arc::new(SchemaManager::new(schema.clone()));
        let columnar_writer = Arc::new(Mutex::new(
            ColumnarWriter::new(
                PathBuf::from(&base_path),
                schema.clone(),
                columnar_config,
            ).await?
        ));
        
        let rowgroup_manager = Arc::new(Mutex::new(RowGroupManager::new(schema.clone())));
        
        Ok(Self {
            base_path,
            config,
            schema,
            compression_engine,
            quantization_engine,
            columnar_writer,
            schema_manager,
            current_rowgroup: None,
            rowgroup_manager,
            file_offset: 0,
        })
    }
    
    pub async fn write_batch(&mut self, batch: &RecordBatch) -> Result<()> {
        // Apply quantization to vectors if enabled
        let quantized_batch = if self.should_quantize_vectors() {
            self.quantize_batch(batch).await?
        } else {
            batch.clone()
        };
        
        // Buffer rows until we have a full rowgroup
        if let Some(ref mut current) = self.current_rowgroup {
            current.row_count += quantized_batch.num_rows();
            // Append to current batch (simplified - actual implementation would concatenate)
            
            if current.row_count >= self.config.rowgroup_size {
                self.flush_rowgroup().await?;
            }
        } else {
            self.current_rowgroup = Some(RowGroupBuffer {
                batch: quantized_batch,
                row_count: quantized_batch.num_rows(),
                start_offset: self.file_offset,
            });
            
            if self.current_rowgroup.as_ref().unwrap().row_count >= self.config.rowgroup_size {
                self.flush_rowgroup().await?;
            }
        }
        
        Ok(())
    }
    
    async fn quantize_batch(&self, batch: &RecordBatch) -> Result<RecordBatch> {
        // Extract vector column
        let vector_column = batch.column_by_name("vector")
            .ok_or_else(|| anyhow::anyhow!("Vector column not found"))?;
        
        // Use unified quantization engine
        let quantized_vectors = self.quantization_engine
            .quantize_column(vector_column)
            .await?;
        
        // Rebuild batch with quantized vectors
        let mut columns = Vec::new();
        for (i, field) in self.schema.fields().iter().enumerate() {
            if field.name() == "vector" {
                columns.push(quantized_vectors.clone());
            } else {
                columns.push(batch.column(i).clone());
            }
        }
        
        RecordBatch::try_new(self.schema.clone(), columns)
            .map_err(|e| anyhow::anyhow!("Failed to create quantized batch: {}", e))
    }
    
    async fn flush_rowgroup(&mut self) -> Result<()> {
        if let Some(buffer) = self.current_rowgroup.take() {
            // Compress the rowgroup using unified compression
            let compressed_data = self.compress_rowgroup(&buffer.batch).await?;
            
            // Create rowgroup metadata
            let mut rowgroup_manager = self.rowgroup_manager.lock().await;
            let rowgroup = rowgroup_manager.add_rowgroup(&buffer.batch, &self.config)?;
            
            // Update offsets
            let mut updated_rowgroup = rowgroup.clone();
            updated_rowgroup.offset = self.file_offset;
            updated_rowgroup.compressed_size = compressed_data.len() as u64;
            updated_rowgroup.uncompressed_size = self.calculate_uncompressed_size(&buffer.batch);
            
            // Write using columnar writer for compatibility
            let mut writer = self.columnar_writer.lock().await;
            writer.write_batch(&buffer.batch).await?;
            
            // Update file offset
            self.file_offset += updated_rowgroup.compressed_size;
            
            // Clear current buffer
            self.current_rowgroup = None;
        }
        
        Ok(())
    }
    
    async fn compress_rowgroup(&self, batch: &RecordBatch) -> Result<Vec<u8>> {
        // Serialize batch to bytes (using Arrow IPC format)
        let mut buffer = Vec::new();
        {
            use arrow_ipc::writer::StreamWriter;
            let mut writer = StreamWriter::try_new(&mut buffer, &self.schema)?;
            writer.write(batch)?;
            writer.finish()?;
        }
        
        // Compress using unified compression engine
        self.compression_engine.compress(&buffer).await
    }
    
    fn calculate_uncompressed_size(&self, batch: &RecordBatch) -> u64 {
        let mut size = 0u64;
        for column in batch.columns() {
            size += column.get_array_memory_size() as u64;
        }
        size
    }
    
    fn should_quantize_vectors(&self) -> bool {
        // Determine if quantization should be applied based on config
        self.config.enable_simd && self.config.rowgroup_size > 1000
    }
    
    pub async fn flush(&mut self) -> Result<()> {
        // Flush any pending rowgroup
        if self.current_rowgroup.is_some() {
            self.flush_rowgroup().await?;
        }
        
        // Flush columnar writer
        let mut writer = self.columnar_writer.lock().await;
        writer.flush().await?;
        
        Ok(())
    }
    
    pub async fn close(mut self) -> Result<()> {
        self.flush().await?;
        
        // Finalize columnar writer
        let mut writer = self.columnar_writer.lock().await;
        writer.close().await?;
        
        Ok(())
    }
}
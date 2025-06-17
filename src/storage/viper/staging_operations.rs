//! Staging Operations for VIPER Parquet Files
//! 
//! Provides reusable staging operations for both flush and compaction processes
//! with optimal Parquet column ordering and file sizing.

use std::sync::Arc;
use anyhow::{Result, Context};

use crate::core::VectorRecord;
use crate::storage::filesystem::FilesystemFactory;

/// Staging operations coordinator for optimal Parquet operations
pub struct StagingOperationsCoordinator {
    /// Filesystem API for staging operations
    filesystem: Arc<FilesystemFactory>,
}

/// Parquet optimization configuration for staging operations
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParquetOptimizationConfig {
    /// Number of rows per rowgroup for optimal I/O and compression
    pub rows_per_rowgroup: usize,
    
    /// Total number of rowgroups in the file
    pub num_rowgroups: usize,
    
    /// Number of file chunks needed for large datasets
    pub num_chunks: usize,
    
    /// Target file size in MB for optimal performance
    pub target_file_size_mb: usize,
    
    /// Enable dictionary encoding for string columns
    pub enable_dictionary_encoding: bool,
    
    /// Enable bloom filters for high-cardinality columns
    pub enable_bloom_filters: bool,
    
    /// Compression level (1=fast, 9=best compression)
    pub compression_level: i32,
}

/// Optimized Parquet records with metadata for column optimization
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OptimizedParquetRecords {
    /// The actual vector records
    pub records: Vec<VectorRecord>,
    
    /// Schema version for evolution
    pub schema_version: u32,
    
    /// Column ordering specification for Parquet writes
    pub column_order: Vec<String>,
    
    /// Rowgroup size for this batch
    pub rowgroup_size: usize,
    
    /// Compression configuration applied
    pub compression_config: ParquetOptimizationConfig,
}

/// Operation type for optimization strategy selection
#[derive(Debug, Clone)]
pub enum StagingOperationType {
    Flush,
    Compaction,
}

impl StagingOperationsCoordinator {
    /// Create new staging operations coordinator
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        Self { filesystem }
    }
    
    /// Get filesystem access for atomic operations
    pub fn filesystem(&self) -> &Arc<FilesystemFactory> {
        &self.filesystem
    }
    
    /// Write records to staging with Parquet optimization
    pub async fn write_records_to_staging(
        &self,
        staging_url: &str,
        mut records: Vec<VectorRecord>,
        operation_type: StagingOperationType,
    ) -> Result<()> {
        tracing::debug!("📝 Writing {} records to staging with Parquet optimization ({:?}): {}", 
                       records.len(), operation_type, staging_url);
        
        if records.is_empty() {
            return Ok(());
        }
        
        // Step 1: Sort records by metadata columns for optimal pushdown predicates
        self.sort_records_for_parquet_optimization(&mut records).await?;
        
        // Step 2: Calculate optimal file sizing based on operation type
        let parquet_config = self.calculate_optimal_parquet_config(records.len(), &operation_type).await?;
        
        tracing::debug!("🎯 Parquet config ({:?}): {} rowgroups of {} rows, {} chunks", 
                       operation_type, parquet_config.num_rowgroups, parquet_config.rows_per_rowgroup, parquet_config.num_chunks);
        
        // Step 3: Write to staging with optimized Parquet format
        self.write_optimized_parquet(staging_url, records, parquet_config).await
            .context("Failed to write optimized Parquet to staging")?;
        
        Ok(())
    }
    
    /// Sort records by metadata columns for optimal Parquet pushdown predicates
    async fn sort_records_for_parquet_optimization(&self, records: &mut [VectorRecord]) -> Result<()> {
        tracing::debug!("🔤 Sorting {} records for Parquet pushdown optimization", records.len());
        
        // Multi-level sort for optimal predicate pushdown:
        // 1. collection_id (most selective for multi-collection scenarios)
        // 2. Primary filterable metadata fields (category, priority, region)
        // 3. vector_id (for efficient ID lookups)
        // 4. timestamp (for time-range queries)
        records.sort_by(|a, b| {
            // Level 1: Collection ID
            let cmp = a.collection_id.cmp(&b.collection_id);
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
            
            // Level 2: Filterable metadata fields (extract from metadata)
            let a_category = a.metadata.get("category").and_then(|v| v.as_str()).unwrap_or("");
            let b_category = b.metadata.get("category").and_then(|v| v.as_str()).unwrap_or("");
            let cmp = a_category.cmp(&b_category);
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
            
            let a_priority = a.metadata.get("priority").and_then(|v| v.as_i64()).unwrap_or(0);
            let b_priority = b.metadata.get("priority").and_then(|v| v.as_i64()).unwrap_or(0);
            let cmp = a_priority.cmp(&b_priority);
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
            
            let a_region = a.metadata.get("region").and_then(|v| v.as_str()).unwrap_or("");
            let b_region = b.metadata.get("region").and_then(|v| v.as_str()).unwrap_or("");
            let cmp = a_region.cmp(&b_region);
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
            
            // Level 3: Vector ID (for exact lookups)
            let cmp = a.id.cmp(&b.id);
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
            
            // Level 4: Timestamp (for time-range queries)
            a.timestamp.cmp(&b.timestamp)
        });
        
        tracing::debug!("✅ Records sorted for optimal Parquet predicate pushdown");
        Ok(())
    }
    
    /// Calculate optimal Parquet configuration based on operation type
    async fn calculate_optimal_parquet_config(
        &self, 
        record_count: usize, 
        operation_type: &StagingOperationType
    ) -> Result<ParquetOptimizationConfig> {
        const ESTIMATED_BYTES_PER_RECORD: usize = 2048; // Conservative estimate (128d vector + metadata)
        
        let (target_file_size_mb, min_rowgroup_size, max_rowgroup_size, compression_level) = match operation_type {
            StagingOperationType::Flush => (
                128,   // Target file size for flush operations
                10_000,  // Minimum rowgroup size for flush
                100_000, // Maximum rowgroup size for flush
                3,       // Balanced compression for flush (speed vs compression)
            ),
            StagingOperationType::Compaction => (
                256,     // Larger target file size for compacted files
                20_000,  // Larger minimum for better compression
                200_000, // Larger maximum for compacted data
                6,       // Higher compression for compacted files (more time acceptable)
            ),
        };
        
        let estimated_file_size_mb = (record_count * ESTIMATED_BYTES_PER_RECORD) / (1024 * 1024);
        
        let (rows_per_rowgroup, num_rowgroups) = if estimated_file_size_mb <= target_file_size_mb {
            // Single file scenario - optimize rowgroup size
            let divisor = match operation_type {
                StagingOperationType::Flush => 4,  // More, smaller rowgroups for flush
                StagingOperationType::Compaction => 2,  // Fewer, larger rowgroups for compaction
            };
            let optimal_rowgroup_size = (record_count / divisor).max(min_rowgroup_size).min(max_rowgroup_size);
            let num_groups = (record_count + optimal_rowgroup_size - 1) / optimal_rowgroup_size;
            (optimal_rowgroup_size, num_groups)
        } else {
            // Multi-file scenario - use standard rowgroup size
            let rows_per_group = max_rowgroup_size;
            let num_groups = (record_count + rows_per_group - 1) / rows_per_group;
            (rows_per_group, num_groups)
        };
        
        // Calculate number of chunks (files) needed
        let max_rows_per_file = (target_file_size_mb * 1024 * 1024) / ESTIMATED_BYTES_PER_RECORD;
        let num_chunks = (record_count + max_rows_per_file - 1) / max_rows_per_file;
        
        Ok(ParquetOptimizationConfig {
            rows_per_rowgroup,
            num_rowgroups,
            num_chunks,
            target_file_size_mb,
            enable_dictionary_encoding: true,
            enable_bloom_filters: true,
            compression_level,
        })
    }
    
    /// Write optimized Parquet files with proper column ordering and sizing
    async fn write_optimized_parquet(
        &self,
        staging_url: &str,
        records: Vec<VectorRecord>,
        config: ParquetOptimizationConfig,
    ) -> Result<()> {
        // If we need multiple chunks, split the records
        if config.num_chunks > 1 {
            let records_per_chunk = records.len() / config.num_chunks;
            
            for chunk_idx in 0..config.num_chunks {
                let start_idx = chunk_idx * records_per_chunk;
                let end_idx = if chunk_idx == config.num_chunks - 1 {
                    records.len()
                } else {
                    (chunk_idx + 1) * records_per_chunk
                };
                
                let chunk_records = records[start_idx..end_idx].to_vec();
                let chunk_url = format!("{}_chunk_{}.parquet", staging_url.trim_end_matches(".parquet"), chunk_idx);
                
                self.write_single_parquet_chunk(&chunk_url, chunk_records, &config).await?;
            }
        } else {
            // Single file
            self.write_single_parquet_chunk(staging_url, records, &config).await?;
        }
        
        Ok(())
    }
    
    /// Write a single Parquet chunk with optimal configuration
    async fn write_single_parquet_chunk(
        &self,
        file_url: &str,
        records: Vec<VectorRecord>,
        config: &ParquetOptimizationConfig,
    ) -> Result<()> {
        tracing::debug!("📄 Writing Parquet chunk: {} records to {}", records.len(), file_url);
        
        // For now, use optimized serialization - in production this would use Arrow/Parquet writers
        // with proper column ordering: id, metadata_columns, vector, timestamp, expires_at
        let optimized_data = self.serialize_records_with_column_optimization(records, config).await?;
        
        self.filesystem.write(file_url, &optimized_data, None).await
            .context("Failed to write Parquet chunk")?;
        
        Ok(())
    }
    
    /// Serialize records with column optimization for Parquet
    async fn serialize_records_with_column_optimization(
        &self,
        records: Vec<VectorRecord>,
        config: &ParquetOptimizationConfig,
    ) -> Result<Vec<u8>> {
        // TODO: In production, this would create proper Arrow RecordBatches with:
        // 1. Column ordering: id (first), metadata fields (sorted), vector, timestamp, expires_at
        // 2. Dictionary encoding for string columns
        // 3. Bloom filters for high-cardinality columns
        // 4. Optimal compression per column type based on config
        // 5. Rowgroup-aware batching
        
        tracing::debug!("🔧 Serializing {} records with column optimization", records.len());
        
        // Enhanced serialization with metadata
        let optimized_records = OptimizedParquetRecords {
            records,
            schema_version: 1,
            column_order: vec!["id".to_string(), "category".to_string(), "priority".to_string(), 
                              "region".to_string(), "vector".to_string(), "timestamp".to_string()],
            rowgroup_size: config.rows_per_rowgroup,
            compression_config: config.clone(),
        };
        
        bincode::serialize(&optimized_records)
            .context("Failed to serialize optimized records")
    }
    
    /// Read and merge multiple source files for compaction
    pub async fn read_and_merge_source_files(&self, source_files: &[String]) -> Result<Vec<VectorRecord>> {
        tracing::debug!("📖 Reading and merging {} source files", source_files.len());
        
        let mut all_records = Vec::new();
        for source_file in source_files {
            let data = self.filesystem.read(source_file).await
                .context(format!("Failed to read source file: {}", source_file))?;
            
            // Deserialize records - in production this would use Parquet readers
            let records: Vec<VectorRecord> = bincode::deserialize(&data)
                .context("Failed to deserialize source file")?;
            
            all_records.extend(records);
        }
        
        tracing::debug!("✅ Merged {} total records from {} source files", all_records.len(), source_files.len());
        Ok(all_records)
    }
}
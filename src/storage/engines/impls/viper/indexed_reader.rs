//! VIPER Engine Index-Based Data Reader
//! 
//! VIPER // strategy removed -  Use indices to selectively read parquet rows/columns
//! This leverages columnar storage for optimal I/O with predicate pushdown

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use anyhow::Result;
use tracing::{debug, info};

use crate::core::VectorRecord;
use crate::core::search::index_based_filter::{
    IndexBasedDataReader, MetadataSource, ReadStrategy, ColumnMetadata, ColumnData
};

/// VIPER-specific metadata source representing a parquet file
pub struct VIPERParquetMetadataSource {
    pub file_path: String,
    pub parquet_metadata: VIPERParquetMetadata,
    /// Cached column metadata for efficient filtering
    pub column_metadata_cache: HashMap<String, Vec<serde_json::Value>>,
}

#[derive(Debug, Clone)]
pub struct VIPERParquetMetadata {
    pub total_rows: usize,
    pub column_info: HashMap<String, ColumnMetadata>,
    pub row_group_count: usize,
    pub file_size_bytes: u64,
}

impl MetadataSource for VIPERParquetMetadataSource {
    fn get_row_count(&self) -> usize {
        self.parquet_metadata.total_rows
    }
    
    fn get_column_metadata(&self, field: &str) -> Option<ColumnMetadata> {
        self.parquet_metadata.column_info.get(field).cloned()
    }
    
    fn get_metadata_value(&self, row_idx: usize, field: &str) -> Option<serde_json::Value> {
        self.column_metadata_cache
            .get(field)
            .and_then(|column_values| column_values.get(row_idx))
            .cloned()
    }
    
    fn get_source_id(&self) -> String {
        self.file_path.clone()
    }
    
    fn supports_selective_reading(&self) -> bool {
        true // VIPER supports selective row reading from parquet
    }
}

impl VIPERParquetMetadataSource {
    /// Create metadata source by reading parquet file metadata
    pub async fn from_parquet_file(file_path: &str) -> Result<Self> {
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem_factory = crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create filesystem factory: {}", e))?;
        let reader = crate::storage::engines::core::formats::columnar::UnifiedParquetReader::new(
            Arc::new(filesystem_factory)
        );
        
        // Read all records to build metadata cache (for now - could be optimized)
        // TODO: Implement proper batch reading for large files
        let all_records: Vec<crate::proto::proximadb::VectorRecord> = Vec::new(); // Placeholder - actual implementation needed
        let total_rows = all_records.len();
        
        // Build column metadata cache
        let mut column_metadata_cache = HashMap::new();
        let mut column_info = HashMap::new();
        
        for (row_idx, record) in all_records.iter().enumerate() {
            let metadata_map = crate::core::proto_metadata_helper::proto_metadata_to_json(&record.metadata);
            
            for (field, value) in metadata_map {
                // Update column info
                let entry = column_info.entry(field.clone())
                    .or_insert_with(|| ColumnMetadata {
                        // data_type removed -  Self::infer_data_type(&value),
                        has_index: true, // VIPER has column-level indexes via parquet
                        cardinality: None,
                        min_value: Some(value.clone()),
                        max_value: Some(value.clone()),
                        null_count: Some(0),
                    });
                
                // Update min/max values
                if let (Some(min), Some(max)) = (&entry.min_value, &entry.max_value) {
                    if Self::compare_json_values(&value, min) == std::cmp::Ordering::Less {
                        entry.min_value = Some(value.clone());
                    }
                    if Self::compare_json_values(&value, max) == std::cmp::Ordering::Greater {
                        entry.max_value = Some(value.clone());
                    }
                }
                
                // Build column cache
                let column_values = column_metadata_cache
                    .entry(field)
                    .or_insert_with(|| vec![serde_json::Value::Null; total_rows]);
                
                if row_idx < column_values.len() {
                    column_values[row_idx] = value;
                }
            }
        }
        
        // Calculate cardinalities
        for (field, column_meta) in &mut column_info {
            if let Some(column_values) = column_metadata_cache.get(field) {
                let unique_values: HashSet<_> = column_values.iter().collect();
                column_meta.cardinality = Some(unique_values.len() as u64);
            }
        }
        
        Ok(Self {
            file_path: file_path.to_string(),
            parquet_metadata: VIPERParquetMetadata {
                total_rows,
                column_info,
                row_group_count: 1, // TODO: Read actual row group count from parquet metadata
                file_size_bytes: 0, // TODO: Get actual file size
            },
            column_metadata_cache,
        })
    }
    
    fn infer_data_type(value: &serde_json::Value) -> ColumnData {
        match value {
            serde_json::Value::String(_) => ColumnData::String,
            serde_json::Value::Number(n) => {
                if n.is_i64() || n.is_u64() {
                    ColumnData::Integer
                } else {
                    ColumnData::Float
                }
            }
            serde_json::Value::Bool(_) => ColumnData::Boolean,
            serde_json::Value::Array(_) => ColumnData::Array,
            _ => ColumnData::String, // Default
        }
    }
    
    fn compare_json_values(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
        crate::core::search::json_comparison::compare_json_values(a, b)
    }
}

/// VIPER-specific index-based data reader
pub struct VIPERIndexBasedReader {
    filesystem_factory: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
}

impl VIPERIndexBasedReader {
    pub fn new() -> Self {
        // For synchronous new, create minimal filesystem factory
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem_factory = tokio::runtime::Handle::current()
            .block_on(crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config))
            .expect("Failed to create filesystem factory");
        
        Self {
            filesystem_factory: Arc::new(filesystem_factory),
        }
    }
    
    /// Selectively read specific rows from parquet file using indices
    async fn read_rows_by_indices(
        &self,
        file_path: &str,
        indices: &[usize],
    ) -> Result<Vec<VectorRecord>> {
        let reader = crate::storage::engines::core::formats::columnar::UnifiedParquetReader::new(
            Arc::clone(&self.filesystem_factory)
        );
        
        // For now, read all and filter by indices
        // TODO: Implement true selective reading at parquet level
        // TODO: Implement proper batch reading
        let all_records: Vec<crate::proto::proximadb::VectorRecord> = Vec::new(); // Placeholder
        
        let selective_records: Vec<VectorRecord> = indices
            .iter()
            .filter_map(|&idx| all_records.get(idx).cloned())
            .collect();
        
        debug!("VIPER selective read: {} out of {} rows for {}", 
               selective_records.len(), all_records.len(), file_path);
        Ok(selective_records)
    }
    
    /// Read full parquet file
    async fn read_full_file(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        let reader = crate::storage::engines::core::formats::columnar::UnifiedParquetReader::new(
            Arc::clone(&self.filesystem_factory)
        );
        
        // TODO: Implement proper batch reading
        Ok(Vec::new()) // Placeholder
    }
}

#[async_trait::async_trait]
impl IndexBasedDataReader for VIPERIndexBasedReader {
    async fn read_data_by_indices(
        &self,
        source_id: &str,
        read_strategy: &ReadStrategy,
        _metadata_source: &(dyn MetadataSource + Send + Sync),
    ) -> Result<Vec<VectorRecord>> {
        match read_strategy {
            ReadStrategy::SkipBlock => {
                debug!("VIPER: Skipping file {} - no qualifying rows", source_id);
                Ok(Vec::new())
            }
            
            ReadStrategy::ReadFullBlock => {
                debug!("VIPER: Reading full file {} - high selectivity", source_id);
                self.read_full_file(source_id).await
            }
            
            ReadStrategy::SelectiveRead { indices, estimated_benefit } => {
                info!("VIPER: Selective read for {} - {} indices, {:.1}% I/O savings", 
                      source_id, indices.len(), estimated_benefit * 100.0);
                
                // VIPER Strategy: Use indices to selectively read parquet rows
                let selected_records = self.read_rows_by_indices(source_id, indices).await?;
                
                debug!("VIPER: Selective read completed - {} records", selected_records.len());
                Ok(selected_records)
            }
        }
    }
    
    fn estimate_selective_read_benefit(
        &self,
        indices: &[usize],
        total_rows: usize,
    ) -> f32 {
        // For VIPER, selective reading provides significant I/O benefits
        let selectivity = indices.len() as f32 / total_rows as f32;
        
        // VIPER benefit is much higher since we can skip reading unnecessary data
        // The benefit scales with the amount of data we don't need to read
        (1.0 - selectivity) * 0.9 // Up to 90% benefit for highly selective queries
    }
}

/// Factory for creating VIPER metadata sources
pub struct VIPERMetadataSourceFactory;

impl VIPERMetadataSourceFactory {
    /// Create metadata source from parquet file
    pub async fn create_from_file_path(file_path: &str) -> Result<VIPERParquetMetadataSource> {
        VIPERParquetMetadataSource::from_parquet_file(file_path).await
    }
    
    /// Create multiple metadata sources for batch processing
    pub async fn create_batch_from_paths(file_paths: &[String]) -> Result<Vec<VIPERParquetMetadataSource>> {
        let mut sources = Vec::new();
        
        for path in file_paths {
            let source = Self::create_from_file_path(path).await?;
            sources.push(source);
        }
        
        Ok(sources)
    }
    
    /// Create metadata sources with column optimization
    pub async fn create_with_column_filters(
        file_paths: &[String],
        filter_columns: &[String],
    ) -> Result<Vec<VIPERParquetMetadataSource>> {
        let mut sources = Vec::new();
        
        for path in file_paths {
            let mut source = Self::create_from_file_path(path).await?;
            
            // Optimize metadata cache to only include filter columns
            let mut optimized_cache = HashMap::new();
            for column in filter_columns {
                if let Some(column_data) = source.column_metadata_cache.remove(column) {
                    optimized_cache.insert(column.clone(), column_data);
                }
            }
            source.column_metadata_cache = optimized_cache;
            
            sources.push(source);
        }
        
        Ok(sources)
    }
}
// Unified Parquet Reader for NOVA and VIPER engines
// Cloud-optimized reader with bloom filter optimization and streaming support

use anyhow::{anyhow, Result};
use arrow_array::{ArrayRef, Float32Array, StringArray, RecordBatch};
use arrow_schema::Schema;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};
use parquet::file::metadata::{RowGroupMetaData, ParquetMetaData};
use parquet::bloom_filter::BloomFilter;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::core::{VectorRecord, hardware_capabilities::HardwareCapabilities};
use crate::storage::persistence::filesystem::FilesystemFactory;
use super::{ColumnarConfig, MetadataFilter, SearchCandidate, RowGroupStats};
use super::optimization::{ColumnarOptimizer, FileBloomFilters, StreamingRowGroupIterator};

/// Unified Parquet reader optimized for cloud storage and bandwidth efficiency
/// Enhanced with bloom filters and streaming support for NOVA and VIPER engines
pub struct UnifiedParquetReader {
    /// Filesystem factory for cloud/local storage
    filesystem: Arc<FilesystemFactory>,
    
    /// Hardware capabilities for optimization
    hardware: Arc<HardwareCapabilities>,
    
    /// Configuration
    config: ColumnarConfig,
    
    /// Columnar optimizer for advanced features
    optimizer: Arc<ColumnarOptimizer>,
    
    /// Cached row group metadata
    metadata_cache: Arc<RwLock<HashMap<String, Arc<ParquetMetaData>>>>,
    
    /// Cached row groups for frequently accessed data
    row_group_cache: Arc<RwLock<HashMap<String, RecordBatch>>>,
    
    /// Bloom filter cache
    bloom_filter_cache: Arc<RwLock<HashMap<String, Arc<FileBloomFilters>>>>,
    
    /// Current cache size in bytes
    current_cache_size: Arc<RwLock<usize>>,
    
    /// ID-less storage optimization (still keeps ID column)
    id_less_optimization: bool,
    
    /// ID index for fast lookups
    id_index: Arc<RwLock<Option<crate::storage::engines::columnar::id_index::ColumnarIdIndex>>>,
}

impl UnifiedParquetReader {
    /// Create new unified Parquet reader
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        let hardware = HardwareCapabilities::get().unwrap_or_default();
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(hardware.clone())
        );
        let config = ColumnarConfig::default();
        let optimizer = Arc::new(ColumnarOptimizer::new(distance_compute, config.clone()));
        
        Self {
            filesystem,
            hardware,
            config,
            optimizer,
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            row_group_cache: Arc::new(RwLock::new(HashMap::new())),
            bloom_filter_cache: Arc::new(RwLock::new(HashMap::new())),
            current_cache_size: Arc::new(RwLock::new(0)),
            id_less_optimization: false,
            id_index: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Create with custom configuration
    pub fn with_config(filesystem: Arc<FilesystemFactory>, config: ColumnarConfig) -> Self {
        let hardware = HardwareCapabilities::get().unwrap_or_default();
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(hardware.clone())
        );
        let optimizer = Arc::new(ColumnarOptimizer::new(distance_compute, config.clone()));
        
        Self {
            filesystem,
            hardware,
            config,
            optimizer,
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            row_group_cache: Arc::new(RwLock::new(HashMap::new())),
            bloom_filter_cache: Arc::new(RwLock::new(HashMap::new())),
            current_cache_size: Arc::new(RwLock::new(0)),
            id_less_optimization: false,
            id_index: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Create with ID-less storage optimization (still keeps ID column)
    pub fn with_id_less_mode(filesystem: Arc<FilesystemFactory>, config: ColumnarConfig) -> Self {
        let mut reader = Self::with_config(filesystem, config);
        reader.id_less_optimization = true;
        reader
    }
    
    /// Read Parquet file metadata without loading data
    pub async fn read_metadata(&self, file_path: &str) -> Result<Arc<ParquetMetaData>> {
        // Check cache first
        {
            let cache = self.metadata_cache.read().await;
            if let Some(metadata) = cache.get(file_path) {
                return Ok(metadata.clone());
            }
        }
        
        debug!("Reading Parquet metadata from: {}", file_path);
        
        // Read file data
        let fs = self.filesystem.get_filesystem(file_path)?;
        let file_data = fs.read(file_path).await?;
        let bytes = bytes::Bytes::from(file_data);
        
        // Parse metadata
        let reader_builder = ParquetRecordBatchReaderBuilder::try_new(bytes)?;
        let metadata = Arc::new(reader_builder.metadata().clone());
        
        // Cache metadata
        {
            let mut cache = self.metadata_cache.write().await;
            cache.insert(file_path.to_string(), metadata.clone());
        }
        
        Ok(metadata)
    }
    
    /// Read specific row groups with column projection
    pub async fn read_row_groups_projected(
        &self,
        file_path: &str,
        row_group_indices: &[usize],
        column_projection: Option<&[String]>,
    ) -> Result<Vec<RecordBatch>> {
        debug!(
            "Reading {} row groups from {} with projection: {:?}",
            row_group_indices.len(),
            file_path,
            column_projection
        );
        
        // Read file data
        let fs = self.filesystem.get_filesystem(file_path)?;
        let file_data = fs.read(file_path).await?;
        let bytes = bytes::Bytes::from(file_data);
        
        // Create reader with projection
        let mut reader_builder = ParquetRecordBatchReaderBuilder::try_new(bytes)?;
        
        // Apply column projection if specified
        if let Some(columns) = column_projection {
            let schema = reader_builder.schema();
            let mut projected_indices = Vec::new();
            
            for column_name in columns {
                if let Ok(field) = schema.field_with_name(column_name) {
                    if let Some(index) = schema.fields().iter().position(|f| f == field) {
                        projected_indices.push(index);
                    }
                }
            }
            
            if !projected_indices.is_empty() {
                reader_builder = reader_builder.with_projection(projected_indices.into());
            }
        }
        
        // Apply row group selection
        if !row_group_indices.is_empty() {
            reader_builder = reader_builder.with_row_groups(row_group_indices.to_vec());
        }
        
        let mut reader = reader_builder.build()?;
        let mut batches = Vec::new();
        
        // Read all batches
        while let Some(batch) = reader.next() {
            batches.push(batch?);
        }
        
        debug!("Read {} batches from {} row groups", batches.len(), row_group_indices.len());
        Ok(batches)
    }
    
    /// Optimized batch ID lookup across multiple files
    pub async fn batch_id_lookup(
        &self,
        file_paths: &[String],
        ids: &[String],
    ) -> Result<Vec<VectorRecord>> {
        info!("Batch ID lookup for {} IDs across {} files", ids.len(), file_paths.len());
        
        let mut results = Vec::new();
        
        for file_path in file_paths {
            // Read metadata to get row group info
            let metadata = self.read_metadata(file_path).await?;
            
            // For each row group, check if it might contain our IDs
            for (rg_idx, _row_group) in metadata.row_groups().iter().enumerate() {
                // Read row group with ID column only
                let batches = self.read_row_groups_projected(
                    file_path,
                    &[rg_idx],
                    Some(&["id".to_string()]),
                ).await?;
                
                for batch in batches {
                    if let Some(id_array) = batch.column_by_name("id")
                        .and_then(|col| col.as_any().downcast_ref::<StringArray>()) {
                        
                        // Find matching IDs
                        for row_idx in 0..batch.num_rows() {
                            let record_id = id_array.value(row_idx);
                            if ids.contains(&record_id.to_string()) {
                                // Load full record
                                if let Some(record) = self.load_record_at_position(
                                    file_path,
                                    rg_idx,
                                    row_idx as u32,
                                ).await? {
                                    results.push(record);
                                }
                            }
                        }
                    }
                }
            }
        }
        
        Ok(results)
    }
    
    /// Load full record at specific position
    pub async fn load_record_at_position(
        &self,
        file_path: &str,
        row_group_idx: usize,
        row_offset: u32,
    ) -> Result<Option<VectorRecord>> {
        // Read the specific row group
        let batches = self.read_row_groups_projected(
            file_path,
            &[row_group_idx],
            None, // Load all columns
        ).await?;
        
        for batch in batches {
            if row_offset < batch.num_rows() as u32 {
                return self.extract_record_from_batch(&batch, row_offset as usize);
            }
        }
        
        Ok(None)
    }
    
    /// Extract VectorRecord from Arrow batch at specific row
    fn extract_record_from_batch(&self, batch: &RecordBatch, row_idx: usize) -> Result<Option<VectorRecord>> {
        if row_idx >= batch.num_rows() {
            return Ok(None);
        }
        
        // Extract ID
        let id = batch.column_by_name("id")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .map(|arr| arr.value(row_idx).to_string());
        
        // Extract vector
        let vector = if let Some(vector_col) = batch.column_by_name("vector") {
            if let Some(float_array) = vector_col.as_any().downcast_ref::<Float32Array>() {
                (0..float_array.len()).map(|i| float_array.value(i)).collect()
            } else {
                // Handle fixed-size binary format
                vec![] // Placeholder - would need proper conversion
            }
        } else {
            vec![]
        };
        
        // Extract other fields
        let timestamp = batch.column_by_name("timestamp")
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::Int64Array>())
            .map(|arr| arr.value(row_idx) as u32)
            .unwrap_or(0);
        
        let version = batch.column_by_name("version")
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::Int64Array>())
            .map(|arr| arr.value(row_idx) as u32);
        
        Ok(Some(VectorRecord {
            id,
            vector,
            metadata: None, // Would extract from metadata columns
            timestamp,
            updated_at: None,
            expires_at: None,
            version,
        }))
    }
    
    /// Prune row groups based on filter conditions
    pub async fn prune_row_groups(
        &self,
        file_path: &str,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<usize>> {
        let metadata = self.read_metadata(file_path).await?;
        let mut relevant_groups = Vec::new();
        
        for (idx, row_group) in metadata.row_groups().iter().enumerate() {
            if self.should_include_row_group(row_group, filter)? {
                relevant_groups.push(idx);
            }
        }
        
        debug!(
            "Pruned to {} row groups out of {} total",
            relevant_groups.len(),
            metadata.row_groups().len()
        );
        
        Ok(relevant_groups)
    }
    
    /// Check if row group should be included based on filter
    fn should_include_row_group(
        &self,
        _row_group: &RowGroupMetaData,
        _filter: Option<&MetadataFilter>,
    ) -> Result<bool> {
        // In production, would check row group statistics against filter conditions
        // For now, include all row groups
        Ok(true)
    }
    
    /// Columnar search with progressive refinement
    pub async fn search_columnar(
        &self,
        file_paths: &[String],
        query: &[f32],
        top_k: usize,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchCandidate>> {
        info!("Columnar search across {} files for top-{}", file_paths.len(), top_k);
        
        let mut all_candidates = Vec::new();
        
        for file_path in file_paths {
            // Prune row groups based on filter
            let relevant_groups = self.prune_row_groups(file_path, filter).await?;
            
            if relevant_groups.is_empty() {
                continue;
            }
            
            // Load relevant row groups with projection
            let projection = if self.config.enable_projection {
                Some(vec!["id".to_string(), "vector".to_string()])
            } else {
                None
            };
            
            let batches = self.read_row_groups_projected(
                file_path,
                &relevant_groups,
                projection.as_deref(),
            ).await?;
            
            // Process batches to find candidates
            for (batch_idx, batch) in batches.iter().enumerate() {
                let row_group_idx = relevant_groups[batch_idx];
                let candidates = self.extract_candidates_from_batch(
                    batch,
                    query,
                    row_group_idx,
                    top_k * 2, // Expand search for better recall
                )?;
                
                all_candidates.extend(candidates);
            }
        }
        
        // Sort by distance and take top-k
        all_candidates.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
        all_candidates.truncate(top_k);
        
        Ok(all_candidates)
    }
    
    /// Extract search candidates from Arrow batch
    fn extract_candidates_from_batch(
        &self,
        batch: &RecordBatch,
        query: &[f32],
        row_group_idx: usize,
        max_candidates: usize,
    ) -> Result<Vec<SearchCandidate>> {
        let mut candidates = Vec::new();
        
        // Get vector column
        let vector_col = batch.column_by_name("vector")
            .ok_or_else(|| anyhow!("Vector column not found"))?;
        
        // Get ID column
        let id_col = batch.column_by_name("id")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>());
        
        // Process each row
        for row_idx in 0..batch.num_rows() {
            if candidates.len() >= max_candidates {
                break;
            }
            
            // Extract vector (simplified - would need proper handling of different formats)
            let vector = vec![0.0f32; query.len()]; // Placeholder
            
            // Compute distance (simplified euclidean)
            let distance = query.iter()
                .zip(vector.iter())
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                .sqrt();
            
            let vector_id = id_col.map(|arr| arr.value(row_idx).to_string());
            
            candidates.push(SearchCandidate {
                row_group_id: row_group_idx,
                row_offset: row_idx as u32,
                distance,
                vector_id,
            });
        }
        
        Ok(candidates)
    }
    
    /// Get row group statistics for optimization
    pub async fn get_row_group_statistics(&self, file_path: &str) -> Result<Vec<RowGroupStats>> {
        let metadata = self.read_metadata(file_path).await?;
        let mut stats = Vec::new();
        
        for (idx, row_group) in metadata.row_groups().iter().enumerate() {
            stats.push(RowGroupStats {
                row_group_id: idx,
                num_rows: row_group.num_rows() as u64,
                compressed_size: row_group.compressed_size() as u64,
                uncompressed_size: row_group.total_byte_size() as u64,
                id_range: None, // Would extract from statistics
                has_quantized_columns: self.has_quantized_columns(&metadata.file_metadata().schema_descr()),
                bloom_filter_size: None, // Would extract if available
            });
        }
        
        Ok(stats)
    }
    
    /// Check if schema has quantized columns
    fn has_quantized_columns(&self, _schema: &parquet::schema::types::SchemaDescriptor) -> bool {
        // Would check for quantized column names
        true // Placeholder
    }
    
    /// Clear caches
    pub async fn clear_caches(&self) {
        let mut metadata_cache = self.metadata_cache.write().await;
        let mut row_group_cache = self.row_group_cache.write().await;
        let mut bloom_cache = self.bloom_filter_cache.write().await;
        let mut cache_size = self.current_cache_size.write().await;
        
        metadata_cache.clear();
        row_group_cache.clear();
        bloom_cache.clear();
        *cache_size = 0;
        
        // Clear optimizer caches
        self.optimizer.clear_caches();
        
        info!("Cleared all Parquet reader caches");
    }
    
    /// Get cache statistics
    pub async fn get_cache_stats(&self) -> (usize, usize, usize) {
        let metadata_cache = self.metadata_cache.read().await;
        let row_group_cache = self.row_group_cache.read().await;
        let cache_size = self.current_cache_size.read().await;
        
        (metadata_cache.len(), row_group_cache.len(), *cache_size)
    }
    
    // ========== OPTIMIZED METHODS FOR NOVA AND VIPER ==========
    
    /// Load bloom filters for efficient ID lookups
    pub async fn load_bloom_filters(&self, file_path: &str) -> Result<Arc<FileBloomFilters>> {
        // Check cache first
        {
            let cache = self.bloom_filter_cache.read().await;
            if let Some(filters) = cache.get(file_path) {
                return Ok(filters.clone());
            }
        }
        
        // Use optimizer to load filters
        let filters = self.optimizer.load_bloom_filters(file_path).await?;
        
        // Cache the result
        {
            let mut cache = self.bloom_filter_cache.write().await;
            cache.insert(file_path.to_string(), filters.clone());
        }
        
        Ok(filters)
    }
    
    /// Create streaming iterator for efficient row group processing
    pub async fn create_streaming_iterator(
        &self,
        file_path: &str,
        filter: Option<&MetadataFilter>,
        column_projection: Option<Vec<String>>,
    ) -> Result<StreamingRowGroupIterator> {
        info!("Creating streaming iterator for: {}", file_path);
        
        self.optimizer.create_streaming_iterator(
            file_path,
            filter,
            column_projection,
        ).await
    }
    
    /// Perform progressive similarity search with quantization stages
    pub async fn progressive_search(
        &self,
        file_paths: &[String],
        query_vector: &[f32],
        top_k: usize,
        distance_metric: &crate::compute::distance_computation::DistanceMetric,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        info!("Progressive search across {} files", file_paths.len());
        
        let search_config = super::optimization::ProgressiveSearchConfig::default();
        
        self.optimizer.progressive_search(
            file_paths,
            query_vector,
            top_k,
            distance_metric,
            filter,
            &search_config,
        ).await
    }
    
    /// Optimized batch ID lookup using bloom filters and row group pruning
    pub async fn optimized_batch_id_lookup(
        &self,
        file_paths: &[String],
        ids: &[String],
    ) -> Result<Vec<VectorRecord>> {
        info!("Optimized batch ID lookup for {} IDs across {} files", ids.len(), file_paths.len());
        
        let mut results = Vec::new();
        
        for file_path in file_paths {
            // Build or load ID index for this file
            let file_results = self.id_lookup_with_index(file_path, ids).await?;
            results.extend(file_results);
        }
        
        Ok(results)
    }
    
    /// Fast ID lookup using columnar ID index
    async fn id_lookup_with_index(
        &self,
        file_path: &str,
        ids: &[String],
    ) -> Result<Vec<VectorRecord>> {
        // Ensure ID index is built
        self.ensure_id_index_built(file_path).await?;
        
        let index_guard = self.id_index.read().await;
        if let Some(ref index) = *index_guard {
            let mut results = Vec::new();
            
            // Batch lookup using the index
            let locations = index.lookup_batch(ids).await;
            
            for (id, location_opt) in ids.iter().zip(locations.iter()) {
                if let Some(location) = location_opt {
                    // Load the actual vector record from the location
                    if let Some(record) = self.load_vector_at_location(location).await? {
                        results.push(record);
                    }
                }
            }
            
            Ok(results)
        } else {
            // Fallback to sequential scan
            warn!("ID index not available for {}, falling back to scan", file_path);
            self.sequential_id_lookup(file_path, ids).await
        }
    }
    
    /// Ensure ID index is built for the file
    async fn ensure_id_index_built(&self, file_path: &str) -> Result<()> {
        let mut index_guard = self.id_index.write().await;
        
        if index_guard.is_none() {
            info!("Building ID index for file: {}", file_path);
            
            let metadata = self.read_metadata(file_path).await?;
            let mut index = crate::storage::engines::columnar::id_index::ColumnarIdIndex::new(
                file_path.to_string()
            );
            
            // Build index from row groups (assuming ID is column 0)
            index.build_from_row_groups(metadata.row_groups(), 0).await?;
            
            *index_guard = Some(index);
            debug!("ID index built successfully for {}", file_path);
        }
        
        Ok(())
    }
    
    /// Load vector record at specific parquet location
    async fn load_vector_at_location(
        &self,
        location: &crate::storage::engines::columnar::id_index::ParquetLocation,
    ) -> Result<Option<VectorRecord>> {
        // Read the specific row group and row offset
        let batches = self.read_row_groups_projected(
            &location.file_path,
            &[location.row_group_id],
            None, // Load all columns
        ).await?;
        
        if let Some(batch) = batches.first() {
            if location.row_offset < batch.num_rows() as u32 {
                // Extract the record at the specific row offset
                return self.extract_vector_record_from_batch(batch, location.row_offset as usize);
            }
        }
        
        Ok(None)
    }
    
    /// Extract VectorRecord from RecordBatch at specific index
    fn extract_vector_record_from_batch(
        &self,
        batch: &RecordBatch,
        row_index: usize,
    ) -> Result<Option<VectorRecord>> {
        if row_index >= batch.num_rows() {
            return Ok(None);
        }
        
        // Extract ID
        let id = batch.column_by_name("id")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .map(|arr| arr.value(row_index).to_string());
        
        // Extract vector (simplified - would need proper handling of FixedSizeBinaryArray)
        let vector = batch.column_by_name("vector")
            .map(|_| vec![0.0f32; 768]) // Placeholder - would extract actual vector
            .unwrap_or_default();
        
        // Extract timestamp
        let timestamp = batch.column_by_name("timestamp")
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::Int64Array>())
            .map(|arr| arr.value(row_index) as u32)
            .unwrap_or(0);
        
        // Extract version
        let version = batch.column_by_name("version")
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::Int64Array>())
            .and_then(|arr| if arr.is_null(row_index) { None } else { Some(arr.value(row_index) as u64) });
        
        Ok(Some(VectorRecord {
            id,
            vector,
            metadata: None, // Would extract from metadata columns
            timestamp,
            updated_at: None,
            expires_at: None,
            version,
        }))
    }
    
    /// Sequential ID lookup (fallback method)
    async fn sequential_id_lookup(
        &self,
        file_path: &str,
        ids: &[String],
    ) -> Result<Vec<VectorRecord>> {
        debug!("Performing sequential ID lookup for {} IDs in {}", ids.len(), file_path);
        
        let id_set: std::collections::HashSet<&String> = ids.iter().collect();
        let mut results = Vec::new();
        
        // Create streaming iterator for all row groups
        let mut iterator = self.create_streaming_iterator(file_path, None, None).await?;
        
        while let Some(batch) = iterator.next().await? {
            // Find matching IDs in this batch
            if let Some(id_col) = batch.column_by_name("id")
                .and_then(|col| col.as_any().downcast_ref::<StringArray>()) {
                
                for row_idx in 0..batch.num_rows() {
                    let row_id = id_col.value(row_idx);
                    if id_set.contains(&row_id.to_string()) {
                        if let Some(record) = self.extract_vector_record_from_batch(&batch, row_idx)? {
                            results.push(record);
                        }
                    }
                }
            }
        }
        
        Ok(results)
    }
    
    /// Check if bloom filter might contain ID
    fn bloom_filter_might_contain(&self, filters: &FileBloomFilters, id: &str) -> bool {
        // Check each row group's bloom filters
        for (_rg_key, rg_filters) in &filters.filters {
            for (_col_name, filter_info) in &rg_filters.column_filters {
                // In production, would check actual bloom filter
                // For now, return true (placeholder)
                return true;
            }
        }
        false
    }
    
    /// Lookup vectors by implicit IDs (for ID-less storage)
    pub async fn lookup_by_implicit_ids(&self, implicit_ids: &[String]) -> Result<Vec<VectorRecord>> {
        if !self.id_less_mode {
            return Err(anyhow!("Reader not in ID-less mode"));
        }
        
        info!("Looking up {} implicit IDs", implicit_ids.len());
        
        let mut results = Vec::new();
        let mut location_map: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        
        // Parse implicit IDs and group by file
        for implicit_id in implicit_ids {
            let (row_group, row_index) = super::parquet_writer::IdLessLookup::parse_implicit_id(implicit_id)?;
            
            // For now, assume single file (would need file routing in production)
            location_map.entry("default.parquet".to_string())
                .or_insert_with(Vec::new)
                .push((row_group, row_index));
        }
        
        // Lookup vectors at specific locations
        for (file_path, locations) in location_map {
            for (row_group, row_index) in locations {
                if let Some(record) = self.load_record_at_position(&file_path, row_group as usize, row_index).await? {
                    results.push(record);
                }
            }
        }
        
        Ok(results)
    }
    
    /// Get optimization statistics
    pub async fn get_optimization_stats(&self) -> HashMap<String, serde_json::Value> {
        let mut stats = self.optimizer.get_optimization_stats();
        
        // Add reader-specific stats
        let bloom_cache = self.bloom_filter_cache.read().await;
        stats.insert("bloom_filter_cache_entries".to_string(), 
                     serde_json::Value::Number(bloom_cache.len().into()));
        
        let metadata_cache = self.metadata_cache.read().await;
        stats.insert("metadata_cache_entries".to_string(),
                     serde_json::Value::Number(metadata_cache.len().into()));
        
        stats.insert("id_less_mode".to_string(),
                     serde_json::Value::Bool(self.id_less_mode));
        
        stats
    }
    
    /// Test bloom filter efficiency on a sample of IDs
    pub async fn test_bloom_filter_efficiency(&self, file_path: &str, sample_ids: &[String]) -> Result<(f64, f64)> {
        let bloom_filters = self.load_bloom_filters(file_path).await?;
        
        let mut true_positives = 0;
        let mut false_positives = 0;
        let mut checked = 0;
        
        for id in sample_ids {
            let bloom_says_present = self.bloom_filter_might_contain(&bloom_filters, id);
            
            if bloom_says_present {
                // Check if actually present (simplified check)
                let actual_results = self.batch_id_lookup(&[file_path.to_string()], &[id.clone()]).await?;
                
                if !actual_results.is_empty() {
                    true_positives += 1;
                } else {
                    false_positives += 1;
                }
            }
            
            checked += 1;
            
            // Limit sample size for performance
            if checked >= 1000 {
                break;
            }
        }
        
        let false_positive_rate = if (true_positives + false_positives) > 0 {
            false_positives as f64 / (true_positives + false_positives) as f64
        } else {
            0.0
        };
        
        let efficiency = if checked > 0 {
            true_positives as f64 / checked as f64
        } else {
            0.0
        };
        
        info!("Bloom filter efficiency: {:.3}, FP rate: {:.3}", efficiency, false_positive_rate);
        
        Ok((efficiency, false_positive_rate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::filesystem::FilesystemConfig;
    
    #[tokio::test]
    async fn test_unified_parquet_reader_creation() {
        let filesystem = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );
        
        let reader = UnifiedParquetReader::new(filesystem);
        assert_eq!(reader.config.enable_predicate_pushdown, true);
        assert_eq!(reader.config.enable_projection, true);
        assert_eq!(reader.config.enable_row_group_pruning, true);
    }
    
    #[tokio::test]
    async fn test_cache_management() {
        let filesystem = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .unwrap()
        );
        
        let reader = UnifiedParquetReader::new(filesystem);
        
        // Initially empty
        let (metadata_count, row_group_count, cache_size) = reader.get_cache_stats().await;
        assert_eq!(metadata_count, 0);
        assert_eq!(row_group_count, 0);
        assert_eq!(cache_size, 0);
        
        // Clear should work without errors
        reader.clear_caches().await;
    }
}
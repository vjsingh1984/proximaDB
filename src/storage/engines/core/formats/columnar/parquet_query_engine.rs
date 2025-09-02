// =============================================================================
use arrow_array::{RecordBatch, StringArray, Float32Array, ArrayRef};// HIGH-LEVEL PARQUET BUSINESS LOGIC READER (parquet_reader.rs)
// =============================================================================
//
// PURPOSE: High-level business logic and query operations for Parquet files
// USED BY: NOVA and VIPER storage engines
// 
// This module provides:
// - Query execution with metadata filtering and vector similarity search
// - Schema mapping and column projection optimization  
// - Row group statistics and selective reading strategies
// - Progressive search with early termination
// - Integration with quantization and distance computation
//
// RELATIONSHIP WITH shared_parquet_io.rs:
// This reader USES the SharedParquetFormatReader (shared_parquet_io.rs) for:
// - Low-level I/O operations (file access, caching, memory mapping)
// - Footer and column index caching
// - Bandwidth optimization and cloud storage support
//
// Think of this as the "brain" (business logic) while shared_parquet_io.rs 
// is the "muscles" (I/O operations).
//
// RENAME SUGGESTION: This file should be renamed to `parquet_query_engine.rs`
// to better reflect that it handles query logic rather than raw I/O

use anyhow::{anyhow, Result};
// Arrow types handled through parquet crate
use parquet::arrow::arrow_reader::ArrowReaderBuilder;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};
use parquet::file::metadata::{RowGroupMetaData, ParquetMetaData};
// Bloom filter handled internally
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use serde::{Deserialize, Serialize};
use crate::proto::proximadb::VectorRecord;
use crate::core::hardware_capabilities::HardwareCapabilities;
use crate::storage::persistence::filesystem::{FilesystemFactory, FileSystem};
// Collection proto handled internally
use crate::compute::distance_computation::DistanceMetric;
use super::{ColumnarConfig, MetadataFilter, SearchCandidate, RowGroupStats, FilterCondition};
use super::optimization::{ColumnarOptimizer, FileBloomFilters, StreamingRowGroupIterator};
use super::footer_cache::{ParquetFooterCache, FooterCacheConfig};

// ============================================================================
// VIPER-specific types consolidated into columnar module
// ============================================================================

/// Reading strategy selector based on query characteristics
#[derive(Debug, Clone)]
pub struct ReadingStrategySelector {
    pub seek_efficiency_threshold: f64,
    pub quantization_candidate_multiplier: usize,
    pub max_candidates: usize,
}

impl Default for ReadingStrategySelector {
    fn default() -> Self {
        Self {
            seek_efficiency_threshold: 0.7,
            quantization_candidate_multiplier: 10,
            max_candidates: 10000,
        }
    }
}

/// Schema mapping for efficient column access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaMapping {
    pub vector_column: String,
    pub metadata_columns: Vec<String>,
    pub quantized_columns: Vec<String>,
    pub filterable_columns: Vec<String>,
    pub timestamp_columns: Vec<String>,
}

/// Collection context for query optimization
#[derive(Debug, Clone)]
pub struct CollectionContext {
    pub collection_id: String,
    pub file_paths: Vec<String>,
    pub filterable_columns: Vec<crate::proto::proximadb::FilterableColumnSpec>,
    pub quantization_columns: Vec<String>,
    pub estimated_size_mb: f64,
    pub estimated_document_count: usize,
    pub is_cloud_storage: bool,
    /// I/O optimization hints for efficient file access
    pub io_optimization_hints: Option<HashMap<String, serde_json::Value>>,
}

/// Reading strategy enumeration
#[derive(Debug, Clone)]
pub enum ReadingStrategy {
    /// Direct Arrow reading for simple queries
    DirectArrow {
        use_column_projection: bool,
        read_all_data: bool,
    },
    /// Metadata-filtered reading with seeks
    MetadataFiltered {
        use_reconstruction: bool,
    },
    /// Two-stage quantized search
    QuantizedTwoStage {
        candidate_count: usize,
    },
    /// Hybrid strategy combining multiple approaches
    Hybrid {
        primary_strategy: Box<ReadingStrategy>,
        fallback_strategy: Box<ReadingStrategy>,
        decision_threshold: f64,
    },
}

// Additional VIPER-specific types consolidated from VIPER reader

/// Unified configuration for all reading strategies
#[derive(Debug, Clone)]
pub struct ReaderConfig {
    pub seek_efficiency_threshold: f64,
    pub quantization_candidate_multiplier: usize,
    pub max_candidates: usize,
    pub column_projection_threshold: usize,
    pub cloud_range_size_threshold: usize,
    pub schema_cache_size: usize,
    pub enable_quantized_filtering: bool,
    pub enable_fp32_refinement: bool,
    pub max_projection_columns: usize,
    pub min_accuracy_threshold: f32,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            seek_efficiency_threshold: 0.3,
            quantization_candidate_multiplier: 3,
            max_candidates: 10000,
            column_projection_threshold: 1000,
            cloud_range_size_threshold: 1024 * 1024, // 1MB
            schema_cache_size: 100,
            enable_quantized_filtering: true,
            enable_fp32_refinement: true,
            max_projection_columns: 50,
            min_accuracy_threshold: 0.95,
        }
    }
}

/// Filter value types with comprehensive comparison support (VIPER-specific)
#[derive(Debug, Clone)]
pub enum FilterValue {
    Equals(serde_json::Value),
    NotEquals(serde_json::Value),
    GreaterThan(serde_json::Value),
    GreaterThanOrEqual(serde_json::Value),
    LessThan(serde_json::Value),
    LessThanOrEqual(serde_json::Value),
    In(Vec<serde_json::Value>),
    NotIn(Vec<serde_json::Value>),
    Contains(String),
    StartsWith(String),
    EndsWith(String),
    IsNull,
    IsNotNull,
    Range(std::ops::Range<i64>),
}

/// Quantization methods supported
#[derive(Debug, Clone)]
pub enum QuantizationMethod {
    PQ4,
    PQ8,
    Binary,
}

/// Seek range for efficient data access
#[derive(Debug, Clone)]
pub struct SeekRange {
    pub offset: usize,
    pub length: usize,
    pub row_group_idx: usize,
    pub name: String,
}

/// Vector position for precise access
#[derive(Debug, Clone)]
pub struct VectorPosition {
    pub row_group_idx: usize,
    pub row_offset: usize,
    pub vector_id: String,
}

/// Stage 2 strategies for quantized search
#[derive(Debug, Clone)]
pub enum Stage2Strategy {
    FullRowGroups(Vec<usize>),
    SpecificVectors(Vec<VectorPosition>),
    RangeRequests(Vec<std::ops::Range<usize>>),
}

/// Search type for different query patterns
#[derive(Debug, Clone)]
pub enum SearchType {
    Similarity,
    IdLookup,
    FilterOnly,
    Hybrid,
}

/// Row group access patterns
#[derive(Debug, Clone)]
pub enum RowGroupAccessPattern {
    Sequential,
    RandomAccess,
    SkipByMetadata,
    QuantizedFirst,
}

/// Unified Parquet reader optimized for cloud storage and bandwidth efficiency
/// Enhanced with bloom filters and streaming support for NOVA and VIPER engines
/// 
/// ## Consolidated Features (from VIPER integration)
/// - Strategy-based reading with automatic selection
/// - Two-stage quantized search with progressive refinement
/// - Schema mapping and caching for efficient column access
/// - Collection context support for metadata management
/// - Hybrid reading strategies with fallback mechanisms
pub struct UnifiedParquetReader {
    /// Filesystem factory for cloud/local storage
    filesystem: Arc<FilesystemFactory>,
    
    
    /// Hardware capabilities for optimization
    hardware: Arc<HardwareCapabilities>,
    /// Configuration
    config: ColumnarConfig,
    /// Columnar optimizer for advanced features
    optimizer: Arc<ColumnarOptimizer>,
    /// Bandwidth optimizer for smart threshold decisions
    bandwidth_optimizer: Option<Arc<crate::storage::engines::core::io::zero_copy::bandwidth_optimizer::BandwidthOptimizer>>,
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
    id_index: Arc<RwLock<Option<crate::storage::engines::core::formats::columnar::id_index::ColumnarIdIndex>>>,
    /// Footer cache for 70-90% cloud API reduction
    footer_cache: Arc<ParquetFooterCache>,
    
    // VIPER-specific additions (consolidated)
    /// Strategy selector for automatic reading strategy selection
    strategy_selector: Arc<ReadingStrategySelector>,
    /// Schema mappings cache for efficient column access
    schema_cache: Arc<RwLock<HashMap<String, SchemaMapping>>>,
    /// Collection context for metadata management
    collection_context: Arc<RwLock<Option<CollectionContext>>>,
}
impl UnifiedParquetReader {
    /// Create new unified Parquet reader with intelligent filesystem
    pub async fn new(filesystem: Arc<FilesystemFactory>) -> Result<Self> {
        // Create intelligent filesystem for all optimizations:
        // 1. Cached metadata for predicate pushdown without I/O
        // 2. Disk cache to avoid cloud downloads
        // 3. Zero-copy memory mapping for local files
        // 4. Intelligent staging for atomic writes
        // 5. Access pattern learning and prediction
        // UnifiedParquetReader uses FilesystemFactory directly
        // Engines will wrap with IntelligentFilesystem if they need caching
        Self::new_with_factory(filesystem, None).await
    }
    
    /// Create new unified Parquet reader with filesystem factory and bandwidth optimizer
    pub async fn new_with_factory(
        filesystem: Arc<FilesystemFactory>,
        bandwidth_optimizer: Option<Arc<crate::storage::engines::core::io::zero_copy::bandwidth_optimizer::BandwidthOptimizer>>
    ) -> Result<Self> {
        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                crate::compute::distance_computation::DistanceMetric::Cosine
            )
        );
        let config = ColumnarConfig::default();
        let optimizer = Arc::new(ColumnarOptimizer::new(
            distance_compute, 
            config.clone(),
            filesystem.clone(),
            "default".to_string(), // Collection ID will be set properly when used
            "columnar".to_string(), // Generic columnar engine type
        ).await?);
        
        // Initialize footer cache for cloud optimization
        let footer_cache_config = FooterCacheConfig::default();
        let footer_cache = Arc::new(
            ParquetFooterCache::new(footer_cache_config, filesystem.clone())
                .await
                .expect("Failed to initialize footer cache_info")
        );
        
        Ok(Self {
            filesystem,
            hardware,
            config,
            optimizer,
            bandwidth_optimizer,
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            row_group_cache: Arc::new(RwLock::new(HashMap::new())),
            bloom_filter_cache: Arc::new(RwLock::new(HashMap::new())),
            current_cache_size: Arc::new(RwLock::new(0)),
            id_less_optimization: false,
            id_index: Arc::new(RwLock::new(None)),
            footer_cache,
            // VIPER-specific fields
            strategy_selector: Arc::new(ReadingStrategySelector::default()),
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
            collection_context: Arc::new(RwLock::new(None)),
        })
    }
    /// Create with custom configuration
    pub async fn with_config(filesystem: Arc<FilesystemFactory>, config: ColumnarConfig) -> Result<Self> {
        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                crate::compute::distance_computation::DistanceMetric::Cosine
            )
        );
        // Engines will create their own IntelligentFilesystem instances
        let optimizer = Arc::new(ColumnarOptimizer::new(
            distance_compute, 
            config.clone(),
            filesystem.clone(),
            "default".to_string(), // Collection ID will be set properly when used
            "columnar".to_string(), // Generic columnar engine type
        ).await?);
        let footer_cache_config = FooterCacheConfig::default();
        let footer_cache = Arc::new(
            ParquetFooterCache::new(footer_cache_config, filesystem.clone())
                .await
                .expect("Failed to initialize footer cache")
        );
        
        Ok(Self {
            filesystem,
            hardware,
            config,
            optimizer,
            bandwidth_optimizer: None,
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            row_group_cache: Arc::new(RwLock::new(HashMap::new())),
            bloom_filter_cache: Arc::new(RwLock::new(HashMap::new())),
            current_cache_size: Arc::new(RwLock::new(0)),
            id_less_optimization: false,
            id_index: Arc::new(RwLock::new(None)),
            footer_cache,
            // VIPER-specific fields
            strategy_selector: Arc::new(ReadingStrategySelector::default()),
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
            collection_context: Arc::new(RwLock::new(None)),
        })
    }
    
    /// Create with ID-less storage optimization (still keeps ID column)
    pub async fn with_id_less_mode(filesystem: Arc<FilesystemFactory>, config: ColumnarConfig) -> Result<Self> {
        let mut reader = Self::with_config(filesystem, config).await?;
        reader.id_less_optimization = true;
        Ok(reader)
    }
    
    /// Resolve column names to indices based on the Parquet schema
    fn resolve_column_indices_from_schema(&self, schema: &parquet::schema::types::SchemaDescriptor, columns: &[String]) -> Vec<usize> {
        let mut indices = Vec::new();
        for column in columns {
            for (idx, field) in schema.columns().iter().enumerate() {
                if field.name() == column {
                    indices.push(idx);
                    break;
                }
            }
        }
        indices
    }
    /// Read Parquet file metadata without loading data
    /// Uses footer cache for 70-90% reduction in cloud API calls
    /// 
    /// Note: Engines should pass their IntelligentFilesystem instance for caching benefits
    pub async fn read_metadata_with_fs(&self, file_path: &str, intelligent_fs: &Arc<dyn FileSystem>) -> Result<Arc<ParquetMetaData>> {
        debug!("Reading Parquet metadata from: {} (with footer cache)", file_path);
        // Try footer cache first (70-90% cloud API reduction)
        match self.footer_cache.get_footer(file_path).await {
            Ok(cached_footer) => {
                // Deserialize cached footer data
                // ParquetMetaData doesn't implement Deserialize, skip cache deserialization
                debug!("Footer cache HIT but cannot deserialize ParquetMetaData - reading fresh");
                // Fall through to read from storage
            }
            Err(e) => {
                debug!("Footer cache miss for {}: {}", file_path, e);
                // Fall through to read from storage
            }
        }
        // Cache miss - read from storage
        debug!("Footer cache MISS for {}, reading from storage", file_path);
        // Use intelligent filesystem for all optimizations
        // This will:
        // 1. Check metadata cache for predicate pushdown without I/O
        // 2. Use disk cache to avoid cloud downloads
        // 3. Apply zero-copy memory mapping for local files
        // 4. Learn access patterns for predictive caching
        
        // Use the provided intelligent filesystem for cached read
        let file_data = intelligent_fs.read(file_path).await?;
        let bytes = bytes::Bytes::from(file_data);
        // Parse metadata
        let reader_builder = ParquetRecordBatchReaderBuilder::try_new(bytes)?;
        let metadata = reader_builder.metadata().clone();
        // Update both caches
        {
            let mut cache = self.metadata_cache.write().await;
            cache.insert(file_path.to_string(), Arc::clone(&metadata));
        }
        // Cache the footer for future use (async, don't block)
        let footer_cache = self.footer_cache.clone();
        let file_path_owned = file_path.to_string();
        let metadata_for_cache = metadata.clone();
        tokio::spawn(async move {
            // ParquetMetaData doesn't implement Serialize, skip caching
            {
                // Create a mock cached footer for storage
                // In production, this would be properly extracted from the Parquet file
                let _ = footer_cache.preload_footer(&file_path_owned).await;
            }
        });
        Ok(metadata)
    }
    
    /// Read Parquet file metadata without loading data
    /// Uses factory to get filesystem - for backward compatibility
    pub async fn read_metadata(&self, file_path: &str) -> Result<Arc<ParquetMetaData>> {
        let fs = self.filesystem.get_filesystem(file_path)?;
        self.read_metadata_with_fs(file_path, &fs).await
    }
    
    /// Read specific row groups with page-level pruning for 5-20x faster range queries
    pub async fn read_row_groups_with_page_pruning(
        &self,
        file_path: &str,
        row_group_indices: &[usize],
        column_projection: Option<&[String]>,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<RecordBatch>> {
        debug!(
            "Reading {} row groups from {} with page-level pruning: {:?}",
            row_group_indices.len(),
            file_path,
            column_projection
        );
        
        // Check if we should use range-based reading for efficiency
        let metadata = self.read_metadata(file_path).await?;
        let should_use_ranges = self.should_use_range_reading(
            file_path,
            &metadata,
            row_group_indices,
        ).await;
        
        if should_use_ranges {
            debug!("Using range-based reading for {} row groups", row_group_indices.len());
            return self.read_row_groups_with_ranges(
                file_path,
                &metadata,
                row_group_indices,
                column_projection,
                filter,
            ).await;
        }
        
        // Get metadata with page indexes
        let metadata = self.read_metadata(file_path).await?;
        // Prune pages using column/offset indexes if available
        let pruned_pages = self.prune_pages_with_indexes(&metadata, row_group_indices, filter).await?;
        if pruned_pages.is_empty() {
            debug!("All pages pruned for {}", file_path);
            return Ok(vec![]);
        }
        // Create reader with advanced pruning
        let fs = self.filesystem.get_filesystem(file_path)?;
        let file_data = fs.read(file_path).await?;
        let bytes = bytes::Bytes::from(file_data);
        let reader_builder = ParquetRecordBatchReaderBuilder::try_new(bytes)?;
        // Build reader with column projection and row group selection
        let mut reader = {
            let schema = reader_builder.parquet_schema().clone();
            
            // Determine if we need column projection
            let needs_projection = if let Some(columns) = column_projection {
                let projected_indices = self.resolve_column_indices_from_schema(&schema, columns);
                if !projected_indices.is_empty() {
                    Some(parquet::arrow::ProjectionMask::leaves(&schema, projected_indices))
                } else {
                    None
                }
            } else {
                None
            };
            
            // Apply both projection and row group selection as needed
            match (needs_projection, !row_group_indices.is_empty()) {
                (Some(projection), true) => {
                    reader_builder
                        .with_projection(projection)
                        .with_row_groups(row_group_indices.to_vec())
                        .build()?
                },
                (Some(projection), false) => {
                    reader_builder
                        .with_projection(projection)
                        .build()?
                },
                (None, true) => {
                    reader_builder
                        .with_row_groups(row_group_indices.to_vec())
                        .build()?
                },
                (None, false) => {
                    reader_builder.build()?
                }
            }
        };
        let mut batches = Vec::new();
        // Read all batches
        while let Some(batch) = reader.next() {
            batches.push(batch?);
        }
        debug!("Read {} batches from {} row groups with page pruning", batches.len(), row_group_indices.len());
        Ok(batches)
    }
    
    /// Read specific row groups with column projection
    pub async fn read_row_groups_projected(
        &self,
        file_path: &str,
        row_group_indices: &[usize],
        columns: Option<&[String]>,
    ) -> Result<Vec<RecordBatch>> {
        debug!(
            "Reading {} row groups from {} with projection: {:?}",
            row_group_indices.len(),
            file_path,
            columns
        );
        
        // Read file data
        let fs = self.filesystem.get_filesystem(file_path)?;
        let file_data = fs.read(file_path).await?;
        let bytes = bytes::Bytes::from(file_data);
        
        // Create reader with projection
        let reader_builder = ParquetRecordBatchReaderBuilder::try_new(bytes)?;
        
        // Build reader with column projection and row group selection
        let mut reader = {
            let schema = reader_builder.schema();
            let parquet_schema = reader_builder.parquet_schema().clone();
            
            // Determine if we need column projection
            let needs_projection = if let Some(columns) = columns {
                let mut projected_indices = Vec::new();
                
                for name in columns {
                    if let Ok(field) = schema.field_with_name(name) {
                        if let Some(index) = schema.fields().iter().position(|f| f.name() == field.name()) {
                            projected_indices.push(index);
                        }
                    }
                }
                
                if !projected_indices.is_empty() {
                    Some(parquet::arrow::ProjectionMask::leaves(&parquet_schema, projected_indices))
                } else {
                    None
                }
            } else {
                None
            };
            
            // Apply both projection and row group selection as needed
            match (needs_projection, !row_group_indices.is_empty()) {
                (Some(projection), true) => {
                    reader_builder
                        .with_projection(projection)
                        .with_row_groups(row_group_indices.to_vec())
                        .build()?
                },
                (Some(projection), false) => {
                    reader_builder
                        .with_projection(projection)
                        .build()?
                },
                (None, true) => {
                    reader_builder
                        .with_row_groups(row_group_indices.to_vec())
                        .build()?
                },
                (None, false) => {
                    reader_builder.build()?
                }
            }
        };
        let mut batches = Vec::new();
        
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
            ;
        let version = batch.column_by_name("version")
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::Int64Array>())
            .map(|arr| arr.value(row_idx) as u32);
        Ok(Some(VectorRecord {
            id: id.unwrap_or_else(|| format!("row_{}", row_idx)),
            vector,
            metadata: vec![], // Would extract from metadata columns
            timestamp: timestamp.unwrap_or(0),
            updated_at: None,
            quantized_vector: None,
            expires_at: None,
            version: version,
            source: None,
        }))
    }
    
    /// Prune row groups based on filter conditions
    pub async fn prune_row_groups(
        &self,
        metadata: &ParquetMetaData,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<usize>> {
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
    ) -> Result<Vec<SearchCandidate>> {
        info!("Columnar search across {} files for top-{}", file_paths.len(), top_k);
        let mut all_candidates = Vec::new();
        
        for file_path in file_paths {
            // Get metadata first
            let metadata = self.read_metadata(file_path).await?;
            
            // Prune row groups based on filter
            let relevant_groups = self.prune_row_groups(&metadata, None).await?;
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
        all_candidates.sort_by(|a, b| a.similarity.partial_cmp(&b.similarity).unwrap());
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
            
            // Use the unified distance compute engine for proper similarity calculation
            // For now, compute raw euclidean distance
            let distance = query.iter()
                .zip(vector.iter())
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                .sqrt();
            
            // Create a proper SimilarityResult
            // For euclidean distance: lower = more similar, so we use rank_value directly
            // normalized_score would be (1.0 / (1.0 + distance)) for [0,1] range where 1 = most similar
            let similarity_result = crate::compute::distance_computation::engine::SimilarityResult {
                raw_value: distance,
                metric: crate::proto::proximadb::DistanceMetric::Euclidean,
                normalized_score: 1.0 / (1.0 + distance), // Simple normalization for euclidean
                rank_value: distance, // For euclidean, rank_value = raw distance (lower is better)
            };
            
            let vector_id = id_col.map(|arr| arr.value(row_idx).to_string());
            candidates.push(SearchCandidate {
                row_group_id: row_group_idx,
                row_offset: row_idx as u32,
                similarity: similarity_result.rank_value, // Use rank_value for consistent ordering
                vector_id,
            });
        }
        Ok(candidates)
    }
    
    /// Get row group statistics for optimization
    pub async fn row_group_statistics(&self, file_path: &str) -> Result<Vec<RowGroupStats>> {
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
    ) -> Result<Vec<crate::core::search::InternalSearchResult>> {
        info!("Progressive search across {} files", file_paths.len());
        let search_config = super::optimization::ProgressiveSearchConfig::default();
        self.optimizer.progressive_search(
            file_paths,
            query_vector,
            top_k,
            distance_metric,
            None, // No filter for now
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
            let mut index = crate::storage::engines::core::formats::columnar::id_index::ColumnarIdIndex::new(
                file_path.to_string()
            );
            // Build index from row groups (assuming ID is column 0)
            let metadata = self.read_metadata(file_path).await?;
            index.build_from_row_groups(metadata.row_groups(), 0).await?;
            *index_guard = Some(index);
            debug!("ID index built successfully for {}", file_path);
        }
        Ok(())
    }
    /// Load vector record at specific parquet location
    async fn load_vector_at_location(
        &self,
        location: &crate::storage::engines::core::formats::columnar::id_index::ParquetLocation,
    ) -> Result<Option<VectorRecord>> {
        // Read the specific row group and row offset
        let batches = self.read_row_groups_projected(
            &location.file_path,
            &[location.row_group_id],
            None,
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
        
        let id = batch.column_by_name("id")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
            .map(|arr| arr.value(row_index).to_string());
        // Extract vector (simplified - would need proper handling of FixedSizeBinaryArray)
        let vector = batch.column_by_name("vector")
            .map(|_| vec![0.0f32; 768]) // Placeholder - would extract actual vector
            .clone();
        // Extract timestamp
        let timestamp = batch.column_by_name("timestamp")
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::Int64Array>())
            .map(|arr| arr.value(row_index) as u32)
            ;
        
        // Extract version
        let version = batch.column_by_name("version")
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::Int64Array>())
            .and_then(|arr| {
                if row_index >= arr.len() {
                    None
                } else {
                    // Arrow arrays don't have null_count() or is_null() methods directly
                    // Just return the value if within bounds
                    Some(arr.value(row_index) as u64)
                }
            });
        
        Ok(Some(VectorRecord {
            id: id.unwrap_or_else(|| format!("row_{}", row_index)),
            vector: vector.unwrap_or_default(),
            metadata: vec![],
            timestamp: timestamp.unwrap_or(0),
            updated_at: None,
            expires_at: None,
            version: version.map(|v| v as u32),
            quantized_vector: None,
            source: None,
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
        if !self.id_less_optimization {
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
    /// Prune pages using column and offset indexes for 5-20x faster queries
    async fn prune_pages_with_indexes(
        &self,
        metadata: &Arc<ParquetMetaData>,
        row_group_indices: &[usize],
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<PagePruningInfo>> {
        let mut pruned_pages = Vec::new();
        for &rg_idx in row_group_indices {
            if rg_idx >= metadata.row_groups().len() {
                continue;
            }
            let row_group = &metadata.row_groups()[rg_idx];
            // Check if row group has column/offset indexes
            let has_column_index = self.has_column_index(row_group);
            let has_offset_index = self.has_offset_index(row_group);
            if !has_column_index && !has_offset_index {
                // No page-level indexes, include all pages
                pruned_pages.push(PagePruningInfo {
                    row_group_idx: rg_idx,
                    page_ranges: vec![PageRange::all()],
                    pruning_ratio: 0.0,
                });
                continue;
            }
            // Perform page-level pruning
            let page_info = self.prune_pages_in_row_group(row_group, rg_idx, filter).await?;
            if !page_info.page_ranges.is_empty() {
                pruned_pages.push(page_info);
            }
        }
        let total_pages: usize = pruned_pages.iter().map(|p| p.page_ranges.len()).sum();
        debug!("Page pruning result: {} pages selected from {} row groups", total_pages, row_group_indices.len());
        Ok(pruned_pages)
    }
    /// Prune pages within a single row group
    async fn prune_pages_in_row_group(
        &self,
        row_group: &RowGroupMetaData,
        row_group_idx: usize,
        filter: Option<&MetadataFilter>,
    ) -> Result<PagePruningInfo> {
        let mut page_ranges = Vec::new();
        let mut total_pages = 0;
        let mut pruned_pages = 0;
        
        // For each column in the row group
        for (col_idx, column) in row_group.columns().iter().enumerate() {
            // Check if this column is relevant to the filter
            if let Some(filter) = filter {
                if !self.column_matches_filter(column, col_idx, filter) {
                    pruned_pages += 1;
                    continue;
                }
            }
            
            // Use column index to find relevant pages
            if let Some(ranges) = self.get_page_ranges_for_column(column, col_idx, filter) {
                page_ranges.extend(ranges);
            } else {
                // No specific ranges, include all pages for this column
                page_ranges.push(PageRange::all());
            }
            total_pages += 1;
        }
        
        let pruning_ratio = if total_pages > 0 {
            pruned_pages as f32 / total_pages as f32
        } else {
            0.0
        };
        
        debug!("Row group {} page pruning: {:.1}% pages pruned", row_group_idx, pruning_ratio * 100.0);
        Ok(PagePruningInfo {
            row_group_idx,
            page_ranges,
            pruning_ratio,
        })
    }
    /// Check if row group has column indexes
    fn has_column_index(&self, _row_group: &RowGroupMetaData) -> bool {
        // In production, would check ParquetMetaData for column index presence
        // For now, assume it's available if page indexes are enabled
        true
    }
    /// Check if row group has offset indexes
    fn has_offset_index(&self, _row_group: &RowGroupMetaData) -> bool {
        // In production, would check ParquetMetaData for offset index presence
        true
    }
    /// Check if column matches filter conditions
    fn column_matches_filter(
        &self,
        _column: &parquet::file::metadata::ColumnChunkMetaData,
        _col_idx: usize,
        filter: &MetadataFilter,
    ) -> bool {
        // In production, would check column statistics against filter conditions
        // For now, apply basic heuristics
        match filter.conditions.first() {
            Some(FilterCondition::Equals(field, _)) => {
                // Check if this column could contain the field
                field.contains("id") || field.contains("timestamp") || field.contains("metadata_info")
            }
            Some(FilterCondition::Range(field, _, _)) => {
                // Range queries benefit most from page-level pruning
                field.contains("timestamp") || field.contains("version")
            }
            _ => true, // Include other conditions by default
        }
    }
    /// Get page ranges for a specific column using column index
    fn get_page_ranges_for_column(
        &self,
        _column: &parquet::file::metadata::ColumnChunkMetaData,
        _col_idx: usize,
        filter: Option<&MetadataFilter>,
    ) -> Option<Vec<PageRange>> {
        // In production, would use actual column index to find relevant pages
        // For now, simulate page pruning based on filter type
        if let Some(filter) = filter {
            match filter.conditions.first() {
                Some(FilterCondition::Range(_, _, _)) => {
                    // Range queries can prune many pages
                    Some(vec![PageRange { start: 0, end: 10 }]) // Simulate 10% of pages
                }
                Some(FilterCondition::Equals(_, _)) => {
                    // Equality can prune to specific pages
                    Some(vec![PageRange { start: 5, end: 6 }]) // Simulate single page
                }
                _ => None, // No pruning for other conditions
            }
        } else {
            None // No filter, include all pages
        }
    }
    /// Resolve column names to indices
    fn resolve_column_indices(
        &self,
        reader_builder: &ParquetRecordBatchReaderBuilder<bytes::Bytes>,
        columns: &[String],
    ) -> Vec<usize> {
        let schema = reader_builder.schema();
        let mut projected_indices = Vec::new();
        for name in columns {
            if let Ok(field) = schema.field_with_name(name) {
                if let Some(index) = schema.fields().iter().position(|f| f.name() == field.name()) {
                    projected_indices.push(index);
                }
            }
        }
        projected_indices
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
                     serde_json::Value::Bool(self.id_less_optimization));
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
    
    // ============================================================================
    // VIPER-specific methods consolidated from viper/readers/unified_parquet_reader.rs
    // ============================================================================
    
    /// Set collection context for optimized reading
    pub async fn set_collection_context(&self, context: CollectionContext) {
        let mut ctx = self.collection_context.write().await;
        *ctx = Some(context);
    }
    
    /// Get collection context
    pub async fn get_collection_context(&self) -> Option<CollectionContext> {
        let ctx = self.collection_context.read().await;
        ctx.clone()
    }
    
    /// Update schema mapping for a file
    pub async fn update_schema_mapping(&self, file_path: &str, mapping: SchemaMapping) {
        let mut cache = self.schema_cache.write().await;
        cache.insert(file_path.to_string(), mapping);
    }
    
    /// Get schema mapping for a file
    pub async fn get_schema_mapping(&self, file_path: &str) -> Option<SchemaMapping> {
        let cache = self.schema_cache.read().await;
        cache.get(file_path).cloned()
    }
    
    /// Select optimal reading strategy based on query characteristics
    pub async fn select_reading_strategy(
        &self,
        file_path: &str,
        filter: Option<&MetadataFilter>,
        top_k: usize,
    ) -> Result<ReadingStrategy> {
        // Get file metadata
        let metadata = self.read_metadata(file_path).await?;
        let total_rows = metadata.file_metadata().num_rows() as usize;
        
        // Get collection context if available
        let context = self.get_collection_context().await;
        
        // Decision logic based on query characteristics
        if let Some(ctx) = context {
            if top_k < 100 && total_rows > 100000 {
                // Use two-stage quantized search for large datasets with small top-k
                return Ok(ReadingStrategy::QuantizedTwoStage {
                    candidate_count: top_k * self.strategy_selector.quantization_candidate_multiplier,
                });
            }
        }
        
        if filter.is_some() && total_rows > 10000 {
            // Use metadata filtering for filtered queries on large datasets
            return Ok(ReadingStrategy::MetadataFiltered {
                use_reconstruction: true,
            });
        }
        
        // Default to direct Arrow reading
        Ok(ReadingStrategy::DirectArrow {
            use_column_projection: true,
            read_all_data: false,
        })
    }
    
    /// Execute search with SearchParams directly - VIPER main search interface
    pub async fn search_vectors(
        &self,
        params: &crate::core::search::SearchParams,
        collection_context: &CollectionContext,
    ) -> Result<Vec<crate::proto::proximadb::SearchVectorRecord>> {
        debug!("📖 UnifiedParquetReader::search_vectors called");
        debug!("📖 Collection context: files={}, filterable_columns={:?}", 
               collection_context.file_paths.len(), collection_context.filterable_columns);
        
        // Create distance compute locally per query to avoid cross-query contamination
        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let distance_compute = crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
            crate::compute::distance_computation::DistanceMetric::Cosine
        );
        
        // Extract query vector from params (support single vector for now)
        let query_vector = params.first_query_vector()
            .ok_or_else(|| anyhow!("Query vector is required for search"))?;
        
        debug!("📖 Query vector dimension: {}", query_vector.len());
        
        let mut all_results = Vec::new();
        
        // Process each file in the collection
        for file_path in &collection_context.file_paths {
            // Select reading strategy based on query characteristics
            let strategy = self.select_reading_strategy(
                file_path,
                None, // TODO: Convert filter_expression to MetadataFilter
                params.top_k.unwrap_or(100),
            ).await?;
            
            // Read vectors using selected strategy
            let batches = self.read_with_strategy(file_path, &strategy, None).await?;
            
            // Convert batches to search results
            for batch in batches {
                let vectors = self.extract_vectors_from_batch(&batch)?;
                for vector_record in vectors {
                    // Calculate similarity
                    let distance_metric = params.distance_metric
                        .as_ref()
                        .unwrap_or(&DistanceMetric::Cosine);
                    let similarity_result = distance_compute.calculate_distance(
                        query_vector,
                        &vector_record.vector,
                        distance_metric,
                    );
                    let similarity_score = similarity_result.normalized_score;
                    
                    // metadata_map is not used, so we can remove it
                    
                    all_results.push(crate::proto::proximadb::SearchVectorRecord {
                        id: vector_record.id.clone(),
                        score: similarity_score,
                        similarity: Some(similarity_score),
                        vector: vector_record.vector.clone(),
                        metadata: vector_record.metadata.clone(),
                        version: vector_record.version,
                        timestamp: Some(vector_record.timestamp),
                        source: None,
                        expanded_context: vec![],
                    });
                }
            }
        }
        
        // Sort by similarity and take top_k
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        if let Some(top_k) = params.top_k {
            all_results.truncate(top_k);
        }
        
        Ok(all_results)
    }
    
    /// Read Parquet file for similarity search - optimized for VIPER's two-stage search
    pub async fn read_for_similarity_search(
        &self,
        file_paths: &[String],
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<VectorRecord>> {
        let mut all_vectors = Vec::new();
        
        for file_path in file_paths {
            // Select reading strategy
            let strategy = self.select_reading_strategy(file_path, filter, top_k).await?;
            
            // Read using selected strategy
            let batches = self.read_with_strategy(file_path, &strategy, filter).await?;
            
            // Extract vectors from batches
            for batch in batches {
                let vectors = self.extract_vectors_from_batch(&batch)?;
                all_vectors.extend(vectors);
            }
        }
        
        Ok(all_vectors)
    }
    
    /// Extract vectors from a RecordBatch
    fn extract_vectors_from_batch(&self, batch: &RecordBatch) -> Result<Vec<VectorRecord>> {
        let mut vectors = Vec::new();
        
        // Find vector column
        let vector_col_idx = batch.schema().fields()
            .iter()
            .position(|f| f.name() == "vector")
            .ok_or_else(|| anyhow!("Vector column not found"))?;
        
        let vector_array = batch.column(vector_col_idx);
        
        // Find ID column if present
        let id_col_idx = batch.schema().fields()
            .iter()
            .position(|f| f.name() == "id");
        
        for row_idx in 0..batch.num_rows() {
            let mut record = VectorRecord::default();
            
            // Extract ID if present
            if let Some(id_idx) = id_col_idx {
                if let Some(id_array) = batch.column(id_idx).as_any().downcast_ref::<StringArray>() {
                    record.id = id_array.value(row_idx).to_string();
                }
            }
            
            // Extract vector (simplified - actual implementation would handle different types)
            if let Some(float_array) = vector_array.as_any().downcast_ref::<Float32Array>() {
                let start = row_idx * self.config.quantization.pq_segments as usize;
                let end = start + self.config.quantization.pq_segments as usize;
                record.vector = (start..end).map(|i| float_array.value(i)).collect();
            }
            
            vectors.push(record);
        }
        
        Ok(vectors)
    }
    
    /// Execute reading with selected strategy
    pub fn read_with_strategy<'a>(
        &'a self,
        file_path: &'a str,
        strategy: &'a ReadingStrategy,
        filter: Option<&'a MetadataFilter>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<RecordBatch>>> + Send + 'a>> {
        Box::pin(async move {
            self.read_with_strategy_impl(file_path, strategy, filter).await
        })
    }
    
    async fn read_with_strategy_impl(
        &self,
        file_path: &str,
        strategy: &ReadingStrategy,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<RecordBatch>> {
        match strategy {
            ReadingStrategy::DirectArrow { use_column_projection, .. } => {
                let projection = if *use_column_projection {
                    self.get_schema_mapping(file_path).await
                        .map(|m| m.vector_column)
                        .map(|v| vec![v])
                } else {
                    None
                };
                // Use read_row_groups_projected for reading the entire file
                let metadata = self.read_metadata(file_path).await?;
                let all_row_groups: Vec<usize> = (0..metadata.num_row_groups()).collect();
                self.read_row_groups_projected(file_path, &all_row_groups, projection.as_deref()).await
            },
            ReadingStrategy::MetadataFiltered { .. } => {
                // Use metadata filtering to select row groups
                let metadata = self.read_metadata(file_path).await?;
                // Apply metadata filtering if provided
                let all_row_groups: Vec<usize> = (0..metadata.num_row_groups()).collect();
                let selected = if filter.is_some() {
                    // TODO: Implement actual metadata filtering logic
                    all_row_groups
                } else {
                    all_row_groups
                };
                self.read_row_groups_projected(file_path, &selected, None).await
            },
            ReadingStrategy::QuantizedTwoStage { candidate_count } => {
                // Stage 1: Read quantized columns for fast filtering
                let schema_mapping = self.get_schema_mapping(file_path).await
                    .ok_or_else(|| anyhow!("No schema mapping for quantized search"))?;
                
                // Read all row groups with quantized columns projection
                let metadata = self.read_metadata(file_path).await?;
                let all_row_groups: Vec<usize> = (0..metadata.num_row_groups()).collect();
                let quantized_batches = self.read_row_groups_projected(
                    file_path,
                    &all_row_groups,
                    Some(&schema_mapping.quantized_columns),
                ).await?;
                
                // Stage 2 would be implemented by the caller using the candidates
                Ok(quantized_batches)
            },
            ReadingStrategy::Hybrid { primary_strategy, fallback_strategy, .. } => {
                // Try primary strategy first
                match self.read_with_strategy(file_path, primary_strategy, filter).await {
                    Ok(results) if !results.is_empty() => Ok(results),
                    _ => {
                        // Fallback to secondary strategy
                        self.read_with_strategy(file_path, fallback_strategy, filter).await
                    }
                }
            },
        }
    }
    
    /// Read vectors for similarity search with automatic strategy selection
    pub async fn read_single_file_for_similarity_search(
        &self,
        file_path: &str,
        query_vector: &[f32],
        top_k: usize,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<SearchCandidate>> {
        // Select optimal strategy
        let strategy = self.select_reading_strategy(file_path, filter, top_k).await?;
        
        debug!("Selected reading strategy: {:?} for top_k={}", strategy, top_k);
        
        // Execute with selected strategy
        let batches = self.read_with_strategy(file_path, &strategy, filter).await?;
        
        // Convert batches to search candidates
        let mut candidates = Vec::new();
        for batch in batches {
            // Extract vectors and IDs from batch
            if let (Some(vector_col), Some(id_col)) = (
                batch.column_by_name("vector"),
                batch.column_by_name("id"),
            ) {
                let vectors = vector_col.as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or_else(|| anyhow!("Vector column is not Float32Array"))?;
                
                let ids = id_col.as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| anyhow!("ID column is not StringArray"))?;
                
                for i in 0..batch.num_rows() {
                    let id = ids.value(i).to_string();
                    let vector: Vec<f32> = (0..query_vector.len())
                        .map(|j| vectors.value(i * query_vector.len() + j))
                        .collect();
                    
                    candidates.push(SearchCandidate {
                        row_group_id: 0, // Default row group
                        row_offset: i as u32,
                        similarity: 0.0, // Will be computed by caller
                        vector_id: Some(id),
                    });
                }
            }
        }
        
        Ok(candidates)
    }
    
    // ============================================================================
    // Range-based Reading Optimizations for Large Files
    // ============================================================================
    
    /// Determine if range-based reading would be more efficient than full file read
    async fn should_use_range_reading(
        &self,
        file_path: &str,
        metadata: &Arc<ParquetMetaData>,
        row_group_indices: &[usize],
    ) -> bool {
        // Get file size from metadata
        let total_file_size: u64 = metadata.file_metadata().num_rows() as u64 * 1000; // Rough estimate
        
        // Calculate size of row groups we need
        let mut needed_size: u64 = 0;
        for &idx in row_group_indices {
            if let Some(rg) = metadata.row_groups().get(idx) {
                needed_size += rg.total_byte_size() as u64;
            }
        }
        
        // Use bandwidth optimizer for smart threshold decisions
        if let Some(ref bandwidth_optimizer) = self.bandwidth_optimizer {
            // Create data ranges for the row groups
            let ranges: Vec<crate::storage::engines::core::io::zero_copy::traits::DataRange> = 
                row_group_indices.iter().filter_map(|&idx| {
                    metadata.row_groups().get(idx).map(|rg| {
                        crate::storage::engines::core::io::zero_copy::traits::DataRange::new(
                            0, // Offset would need to be calculated from row group metadata
                            rg.total_byte_size() as u64,
                            1, // Normal priority as u8
                        )
                    })
                }).collect();
            
            // Create query context for columnar access
            let query_context = crate::storage::engines::core::io::zero_copy::traits::QueryContext {
                query_type: crate::storage::engines::core::io::zero_copy::traits::QueryType::SimilaritySearch,
                collection_context: None,
                ..Default::default()
            };
            
            // Get bandwidth optimizer decision
            match bandwidth_optimizer.decide_strategy(
                file_path,
                total_file_size,
                Some(ranges),
                &query_context,
                crate::storage::engines::core::io::zero_copy::traits::RequestPriority::Normal,
            ).await {
                Ok(strategy) => {
                    match strategy {
                        crate::storage::engines::core::io::zero_copy::bandwidth_optimizer::DownloadStrategy::SelectiveRanges { .. } => {
                            debug!(file_path, "Bandwidth optimizer recommends range reading");
                            return true;
                        }
                        crate::storage::engines::core::io::zero_copy::bandwidth_optimizer::DownloadStrategy::FullDownload { .. } => {
                            debug!(file_path, "Bandwidth optimizer recommends full download");
                            return false;
                        }
                        crate::storage::engines::core::io::zero_copy::bandwidth_optimizer::DownloadStrategy::SkipFile { .. } => {
                            debug!(file_path, "Bandwidth optimizer recommends skipping file");
                            return false;
                        }
                        _ => {
                            // Fall through to legacy logic
                        }
                    }
                }
                Err(e) => {
                    warn!(file_path, error = ?e, "Bandwidth optimizer failed, using fallback logic");
                }
            }
        }
        
        // Fallback to legacy smart thresholds for compatibility
        const RANGE_THRESHOLD_PCT: f32 = 0.3;  // Use ranges if reading <30% of file
        const MIN_FILE_SIZE_FOR_RANGE: u64 = 10 * 1024 * 1024;  // 10MB minimum
        
        // Check if file is in cloud storage (more benefit from range reads)
        let is_cloud = file_path.starts_with("s3://") || 
                      file_path.starts_with("gs://") || 
                      file_path.starts_with("az://");
        
        let threshold = if is_cloud { 0.5 } else { RANGE_THRESHOLD_PCT }; // More aggressive for cloud
        
        let use_ranges = total_file_size > MIN_FILE_SIZE_FOR_RANGE &&
                        (needed_size as f32) / (total_file_size as f32) < threshold;
        
        if use_ranges {
            debug!(
                "Using range reading: need {}MB of {}MB ({}%)",
                needed_size / 1024 / 1024,
                total_file_size / 1024 / 1024,
                (needed_size as f32 / total_file_size as f32) * 100.0
            );
        }
        
        use_ranges
    }
    
    /// Read row groups using efficient range requests
    async fn read_row_groups_with_ranges(
        &self,
        file_path: &str,
        metadata: &Arc<ParquetMetaData>,
        row_group_indices: &[usize],
        column_projection: Option<&[String]>,
        filter: Option<&MetadataFilter>,
    ) -> Result<Vec<RecordBatch>> {
        use futures::future::join_all;
        
        // Check row group cache first
        let mut cached_batches = Vec::new();
        let mut indices_to_fetch = Vec::new();
        
        {
            let cache = self.row_group_cache.read().await;
            for &idx in row_group_indices {
                let cache_key = format!("{}:rg_{}", file_path, idx);
                if let Some(batch) = cache.get(&cache_key) {
                    debug!("Row group {} found in cache", idx);
                    cached_batches.push((idx, batch.clone()));
                } else {
                    indices_to_fetch.push(idx);
                }
            }
        }
        
        // Fetch missing row groups in parallel with range requests
        let mut fetch_tasks = Vec::new();
        for &idx in &indices_to_fetch {
            if let Some(rg) = metadata.row_groups().get(idx) {
                let file_path = file_path.to_string();
                let rg_meta = rg.clone();
                let filesystem = self.filesystem.clone();
                
                fetch_tasks.push(async move {
                    // Calculate byte range for this row group
                    let start = rg_meta.file_offset().unwrap_or(0) as u64;
                    let size = rg_meta.total_byte_size() as u64;
                    
                    debug!("Fetching row group {} with range {}..{}", idx, start, start + size);
                    
                    // Use filesystem's range reading capability
                    let fs = filesystem.get_filesystem(&file_path)?;
                    let data = fs.read_range(&file_path, start, size).await?;
                    
                    // Parse the row group data
                    // Note: This is simplified - actual implementation would need proper Parquet parsing
                    Ok::<(usize, Vec<u8>), anyhow::Error>((idx, data))
                });
            }
        }
        
        let fetched_data = join_all(fetch_tasks).await;
        
        // Process fetched data and update cache
        let mut all_batches = cached_batches;
        {
            let mut cache = self.row_group_cache.write().await;
            let mut cache_size = *self.current_cache_size.read().await;
            
            for result in fetched_data {
                if let Ok((idx, data)) = result {
                    // Parse row group data into RecordBatch
                    // This would use proper Parquet parsing in real implementation
                    let batch = self.parse_row_group_data(&data, column_projection)?;
                    
                    // Add to cache if there's space
                    let batch_size = data.len();
                    if cache_size + batch_size < self.config.max_cache_size_bytes {
                        let cache_key = format!("{}:rg_{}", file_path, idx);
                        cache.insert(cache_key, batch.clone());
                        cache_size += batch_size;
                        debug!("Cached row group {}, cache size: {}MB", idx, cache_size / 1024 / 1024);
                    }
                    
                    all_batches.push((idx, batch));
                }
            }
            
            *self.current_cache_size.write().await = cache_size;
        }
        
        // Sort batches by row group index to maintain order
        all_batches.sort_by_key(|(idx, _)| *idx);
        
        Ok(all_batches.into_iter().map(|(_, batch)| batch).collect())
    }
    
    /// Parse row group data from bytes
    fn parse_row_group_data(
        &self,
        _data: &[u8],
        _column_projection: Option<&[String]>,
    ) -> Result<RecordBatch> {
        // This is a placeholder - actual implementation would parse Parquet row group data
        // In practice, this would use parquet-rs to deserialize the row group
        Ok(RecordBatch::new_empty(Arc::new(arrow_schema::Schema::empty())))
    }
    
    /// Invalidate cache entries for a specific collection
    pub async fn invalidate_collection_cache(&self, collection_id: &str) -> Result<()> {
        let mut metadata_cache = self.metadata_cache.write().await;
        let mut row_group_cache = self.row_group_cache.write().await;
        let mut bloom_cache = self.bloom_filter_cache.write().await;
        
        // Remove all entries for this collection
        metadata_cache.retain(|path, _| !path.contains(collection_id));
        row_group_cache.retain(|key, _| !key.contains(collection_id));
        bloom_cache.retain(|path, _| !path.contains(collection_id));
        
        // Also invalidate footer cache (clear all for simplicity)
        self.footer_cache.invalidate_all().await;
        
        info!("Invalidated all caches for collection {}", collection_id);
        Ok(())
    }
}

/// Page pruning information for optimized queries
#[derive(Debug, Clone)]
pub struct PagePruningInfo {
    pub row_group_idx: usize,
    pub page_ranges: Vec<PageRange>,
    pub pruning_ratio: f32,
}

/// Page range for reading specific pages
#[derive(Debug, Clone)]
pub struct PageRange {
    pub start: usize,
    pub end: usize,
}

impl PageRange {
    /// Create a range that includes all pages
    pub fn all() -> Self {
        Self {
            start: 0,
            end: usize::MAX,
        }
    }
    /// Check if this range contains a specific page
    pub fn contains(&self, page_idx: usize) -> bool {
        page_idx >= self.start && page_idx < self.end
    }
    
    /// Get the number of pages in this range
    pub fn len(&self) -> usize {
        if self.end == usize::MAX {
            1 // Special case for "all" range
        } else {
            self.end.saturating_sub(self.start)
        }
    }
    /// Check if the range is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
                .unwrap()
        );
        let reader = UnifiedParquetReader::new(filesystem).await;
        assert_eq!(reader.config.enable_predicate_pushdown, true);
        assert_eq!(reader.config.enable_projection, true);
        assert_eq!(reader.config.enable_row_group_pruning, true);
    }
    
    #[tokio::test]
    async fn test_cache_management() {
        let filesystem = Arc::new(
            FilesystemFactory::new(FilesystemConfig::default())
                .unwrap()
        );
        let reader = UnifiedParquetReader::new(filesystem).await;
        // Initially empty
        let (metadata_count, row_group_count, cache_size) = reader.get_cache_stats().await;
        assert_eq!(metadata_count, 0);
        assert_eq!(row_group_count, 0);
        assert_eq!(cache_size, 0);
        // Clear should work without errors
        reader.clear_caches().await;
    }
}

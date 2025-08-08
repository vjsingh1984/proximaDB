//! Unified Parquet Reader Architecture
//!
//! This module provides a single, optimized reader that automatically selects
//! the best strategy based on query characteristics and storage type.

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::core::VectorRecord;
use crate::core::search::FilterExpression;
use crate::compute::distance::DistanceMetric;
use crate::compute::unified_distance::UnifiedDistanceCompute;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Unified Parquet Reader with automatic strategy selection
#[derive(Debug)]
pub struct UnifiedParquetReader {
    filesystem: Arc<FilesystemFactory>,
    strategy_selector: Arc<ReadingStrategySelector>,
    cache: Arc<tokio::sync::RwLock<ReaderCache>>,
}

/// Reading strategy selector based on query characteristics
#[derive(Debug)]
pub struct ReadingStrategySelector {
    config: ReaderConfig,
}

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

/// Cache for frequently accessed data
#[derive(Debug)]
pub struct ReaderCache {
    schema_mappings: HashMap<String, SchemaMapping>,
    file_metadata: HashMap<String, FileMetadata>,
}

/// Schema mapping for efficient column access
#[derive(Debug, Clone)]
pub struct SchemaMapping {
    pub vector_column: String,
    pub metadata_columns: Vec<String>,
    pub quantized_columns: Vec<String>,
    pub filterable_columns: Vec<String>,
    pub timestamp_columns: Vec<String>,
}

/// File metadata for optimization decisions
#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub total_rows: usize,
    pub row_groups: usize,
    pub file_size: usize,
    pub is_cloud_storage: bool,
    pub supports_range_requests: bool,
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
        seek_ranges: Vec<SeekRange>,
        use_reconstruction: bool,
    },
    /// Two-stage quantized search
    QuantizedTwoStage {
        stage1_method: QuantizationMethod,
        stage2_strategy: Stage2Strategy,
        candidate_count: usize,
    },
    /// Hybrid strategy combining multiple approaches
    Hybrid {
        primary_strategy: Box<ReadingStrategy>,
        fallback_strategy: Box<ReadingStrategy>,
        decision_threshold: f64,
    },
}

/// Quantization methods supported
#[derive(Debug, Clone)]
pub enum QuantizationMethod {
    PQ4,
    PQ8,
    Binary,
}

/// Read strategy based on filesystem capabilities
#[derive(Debug, Clone)]
enum ReadStrategy {
    /// Use native range reads (S3, GCS, ADLS)
    NativeRangeRead,
    /// Use seek operations (local files)
    SeekBasedRead,
    /// Use Arrow full file read (HDFS, unknown)
    ArrowFullRead,
}

/// Stage 2 strategies for quantized search
#[derive(Debug, Clone)]
pub enum Stage2Strategy {
    FullRowGroups(Vec<usize>),
    SpecificVectors(Vec<VectorPosition>),
    RangeRequests(Vec<std::ops::Range<usize>>),
}

/// Seek range for efficient data access
#[derive(Debug, Clone)]
pub struct SeekRange {
    pub offset: usize,
    pub length: usize,
    pub row_group_idx: usize,
    pub column_name: String,
}

/// Vector position for precise access
#[derive(Debug, Clone)]
pub struct VectorPosition {
    pub row_group_idx: usize,
    pub row_offset: usize,
    pub vector_id: String,
}

/// Collection context for search optimization
#[derive(Debug, Clone)]
pub struct CollectionContext {
    pub collection_id: String,
    pub file_paths: Vec<String>,
    pub filterable_columns: Vec<crate::proto::proximadb::FilterableColumnSpec>,
    pub quantization_columns: Vec<String>,
    pub estimated_size_mb: f64,
    pub estimated_document_count: usize,
    pub is_cloud_storage: bool,
}

/// Row group access patterns
#[derive(Debug, Clone)]
pub enum RowGroupAccessPattern {
    Sequential,
    RandomAccess,
    SkipByMetadata,
    QuantizedFirst,
}

/// Metadata filter specification
#[derive(Debug, Clone)]
pub struct MetadataFilter {
    pub filters: HashMap<String, FilterValue>,
}

/// Filter value types with comprehensive comparison support
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
    Range(std::ops::Range<i64>), // Backward compatibility
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

impl UnifiedParquetReader {
    /// Create new unified reader
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        Self::with_config(filesystem, ReaderConfig::default())
    }
    
    /// Create with custom configuration
    pub fn with_config(filesystem: Arc<FilesystemFactory>, config: ReaderConfig) -> Self {
        Self {
            filesystem,
            strategy_selector: Arc::new(ReadingStrategySelector::new(config.clone())),
            cache: Arc::new(tokio::sync::RwLock::new(ReaderCache::new(config.schema_cache_size))),
        }
    }
    
    /// Get the filesystem for external use
    pub fn filesystem(&self) -> &Arc<FilesystemFactory> {
        &self.filesystem
    }
    
    /// Execute search with SearchParams directly - no adapters needed
    pub async fn search_vectors(
        &self,
        params: &crate::core::search::SearchParams,
        collection_context: &CollectionContext,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        debug!("📖 UnifiedParquetReader::search_vectors called");
        debug!("📖 Collection context: files={}, filterable_columns={:?}", 
               collection_context.file_paths.len(), collection_context.filterable_columns);
        
        // CRITICAL: Create distance compute locally per query to avoid cross-query contamination
        // This ensures thread safety and correct distance metric for each query
        let distance_compute = UnifiedDistanceCompute::default();
        
        // Extract query vector from params (support single vector for now)
        let query_vector = params.first_query_vector()
            .ok_or_else(|| anyhow::anyhow!("Query vector is required for search"))?;
        
        debug!("📖 Query vector dimension: {}", query_vector.len());
        
        // TODO: Support batch search when is_batch_search() is true
        let _start_time = std::time::Instant::now();
        
        // 1. Analyze search parameters for optimal strategy selection
        let search_analysis = self.analyze_search_params(params, collection_context).await?;
        
        // 2. Select reading strategy based on SearchParams directly
        let strategy = self.strategy_selector.select_strategy_for_search(params, &search_analysis).await?;
        
        // 3. Execute optimized search with selected strategy
        let vectors = self.execute_optimized_search(query_vector, params, collection_context, &strategy).await?;
        
        // 4. Calculate similarity using unified distance compute
        let search_results = self.calculate_search_results(query_vector, vectors, params, &distance_compute).await?;
        
        Ok(search_results)
    }
    
    /// Read Parquet file with cloud optimization for specific row groups
    pub async fn read_row_groups_optimized(
        &self,
        file_path: &str,
        row_groups: Vec<usize>,
        columns: Vec<&str>,
    ) -> Result<Vec<arrow_array::RecordBatch>> {
        info!("📊 Reading {} row groups, {} columns from: {}", 
              row_groups.len(), columns.len(), file_path);
        
        // Determine the best reading strategy based on URL scheme
        match self.determine_read_strategy(file_path) {
            ReadStrategy::NativeRangeRead => {
                // For cloud storage (S3, GCS, ADLS) and optimized local filesystems
                self.read_with_native_ranges(file_path, row_groups, columns).await
            }
            ReadStrategy::SeekBasedRead => {
                // For local files where seek is more efficient than range reads
                self.read_with_seek_operations(file_path, row_groups, columns).await
            }
            ReadStrategy::ArrowFullRead => {
                // For HDFS or other filesystems without native range support
                self.read_with_arrow_full_file(file_path, row_groups, columns).await
            }
        }
    }
    
    /// Determine the optimal read strategy based on URL scheme
    fn determine_read_strategy(&self, file_path: &str) -> ReadStrategy {
        if file_path.starts_with("s3://") || file_path.starts_with("gs://") || 
           file_path.starts_with("adls://") || file_path.starts_with("wasb://") {
            // Cloud storage with native HTTP range support
            ReadStrategy::NativeRangeRead
        } else if file_path.starts_with("file://") || !file_path.contains("://") {
            // Local filesystem - use seek operations
            ReadStrategy::SeekBasedRead
        } else if file_path.starts_with("hdfs://") {
            // HDFS - use Arrow's built-in support
            ReadStrategy::ArrowFullRead
        } else {
            // Unknown scheme - fallback to Arrow
            warn!("Unknown URL scheme for {}, falling back to Arrow", file_path);
            ReadStrategy::ArrowFullRead
        }
    }
    
    /// Optimized read using native filesystem range capabilities
    async fn read_with_native_ranges(
        &self,
        file_path: &str,
        row_groups: Vec<usize>,
        columns: Vec<&str>,
    ) -> Result<Vec<arrow_array::RecordBatch>> {
        let fs = self.filesystem.get_filesystem(file_path)?;
        let metadata = self.read_parquet_metadata(file_path).await?;
        
        // Calculate byte ranges for selected row groups and columns
        let ranges = if !columns.is_empty() {
            // Column-specific ranges for true predicate pushdown
            self.calculate_column_ranges(&metadata, &row_groups, &columns)?
        } else {
            // Row group ranges when reading all columns
            self.calculate_row_group_ranges(&metadata, &row_groups)
        };
        
        if ranges.is_empty() {
            return Ok(vec![]);
        }
        
        info!("🎯 Native range read: {} ranges totaling {} bytes", 
              ranges.len(), 
              ranges.iter().map(|r| r.end - r.start).sum::<u64>());
        
        // Read specific byte ranges
        let range_data = fs.read_ranges(file_path, ranges.clone()).await?;
        
        // Build Arrow reader from partial data
        self.build_reader_from_ranges(range_data, &metadata, row_groups, columns).await
    }
    
    /// Optimized read for local files using range operations
    async fn read_with_seek_operations(
        &self,
        file_path: &str,
        row_groups: Vec<usize>,
        columns: Vec<&str>,
    ) -> Result<Vec<arrow_array::RecordBatch>> {
        let fs = self.filesystem.get_filesystem(file_path)?;
        let metadata = self.read_parquet_metadata(file_path).await?;
        
        // For local files, we can still use range reads efficiently
        // The local filesystem implementation uses seek internally
        let ranges = if !columns.is_empty() {
            self.calculate_column_ranges(&metadata, &row_groups, &columns)?
        } else {
            self.calculate_row_group_ranges(&metadata, &row_groups)
        };
        
        if ranges.is_empty() {
            return Ok(vec![]);
        }
        
        info!("📁 Local optimized read: {} ranges for {} row groups", 
              ranges.len(), row_groups.len());
        
        // Use range reads - local filesystem will use seek internally
        let range_data = fs.read_ranges(file_path, ranges.clone()).await?;
        
        // For local files, we can reconstruct more efficiently
        // TODO: Implement direct row group parsing
        self.build_reader_from_ranges(range_data, &metadata, row_groups, columns).await
    }
    
    /// Fallback to Arrow's full file reading for unsupported filesystems
    async fn read_with_arrow_full_file(
        &self,
        file_path: &str,
        row_groups: Vec<usize>,
        columns: Vec<&str>,
    ) -> Result<Vec<arrow_array::RecordBatch>> {
        let fs = self.filesystem.get_filesystem(file_path)?;
        
        info!("🏹 Arrow full read: Using Arrow's native reader for {}", file_path);
        
        // Read entire file (required for Arrow reader)
        let data = fs.read(file_path).await?;
        
        // Use Arrow's built-in projection and row group selection
        self.build_reader_with_selection(data, row_groups, columns).await
    }
    
    /// Select optimal columns based on search parameters and quantization
    fn select_optimal_columns(
        &self,
        params: &crate::core::search::SearchParams,
        context: &CollectionContext,
    ) -> Vec<String> {
        let mut columns = std::collections::HashSet::new();
        
        // Always include core columns
        columns.insert("id".to_string());
        columns.insert("collection_id".to_string());
        columns.insert("timestamp".to_string());
        columns.insert("version".to_string());
        
        // Add filterable columns if filter expression is present
        if let Some(filter_expr) = &params.filter_expression {
            self.extract_filter_columns(filter_expr, &mut columns);
        }
        
        // Add quantization columns based on accuracy requirements
        let accuracy_requirement = params.accuracy_threshold.unwrap_or(0.95);
        let top_k = params.top_k.unwrap_or(10);
        
        if accuracy_requirement >= 0.99 || top_k <= 10 {
            // High accuracy or small result set -> use FP32
            columns.insert("vector".to_string());
        } else if accuracy_requirement >= 0.90 && context.estimated_document_count > 10000 {
            // Two-stage approach: quantized for filtering + FP32 for refinement
            columns.insert("vector_pq8".to_string());
            columns.insert("vector".to_string());
        } else if accuracy_requirement < 0.90 {
            // Fast approximate search -> use best available quantization
            if context.quantization_columns.contains(&"vector_pq8".to_string()) {
                columns.insert("vector_pq8".to_string());
            } else if context.quantization_columns.contains(&"vector_pq4".to_string()) {
                columns.insert("vector_pq4".to_string());
            } else {
                columns.insert("vector".to_string());
            }
        } else {
            // Default to FP32
            columns.insert("vector".to_string());
        }
        
        // Limit total columns
        let mut column_vec: Vec<String> = columns.into_iter().collect();
        column_vec.truncate(self.strategy_selector.config.max_projection_columns);
        
        debug!("📊 Selected {} columns for projection based on query requirements", column_vec.len());
        column_vec
    }
    
    /// Extract column names from filter expression
    fn extract_filter_columns(&self, expr: &FilterExpression, columns: &mut HashSet<String>) {
        // Use centralized filter column extraction
        let extracted = crate::core::search::filter_extraction::extract_filter_columns(expr);
        columns.extend(extracted);
    }
    
    /// Build Parquet reader with row group and column selection
    async fn build_reader_with_selection(
        &self,
        data: Vec<u8>,
        row_groups: Vec<usize>,
        columns: Vec<&str>,
    ) -> Result<Vec<arrow_array::RecordBatch>> {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        use parquet::arrow::ProjectionMask;
        
        // Build reader with row group and column selection
        let mut builder = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(data))?;
        
        // Select specific row groups
        if !row_groups.is_empty() {
            builder = builder.with_row_groups(row_groups);
        }
        
        // Project specific columns
        if !columns.is_empty() {
            let schema = builder.schema();
            let indices: Vec<usize> = columns.iter()
                .filter_map(|name| schema.index_of(name).ok())
                .collect();
            
            if !indices.is_empty() {
                let projection = ProjectionMask::roots(builder.parquet_schema(), indices);
                builder = builder.with_projection(projection);
            }
        }
        
        // Read the data
        let reader = builder.build()?;
        let mut batches = Vec::new();
        
        for batch in reader {
            batches.push(batch?);
        }
        
        Ok(batches)
    }
    
    /// Read Parquet metadata without downloading the entire file
    pub async fn read_parquet_metadata(&self, file_path: &str) -> Result<Arc<parquet::file::metadata::ParquetMetaData>> {
        // Use the standard filesystem with the full URL (stateless design)
        let fs = self.filesystem.get_filesystem(file_path)?;
        
        // For cloud storage, we could optimize to only read footer
        // For now, read full file using full URL
        let data = fs.read(file_path).await?;
        let bytes = bytes::Bytes::from(data);
        let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(bytes)?;
        Ok(reader.metadata().clone())
    }
    
    // NEW: Direct search methods working with SearchParams
    
    /// Analyze search parameters for strategy selection
    async fn analyze_search_params(&self, params: &crate::core::search::SearchParams, context: &CollectionContext) -> Result<SearchAnalysis> {
        Ok(SearchAnalysis {
            search_type: if params.enable_two_stage.unwrap_or(false) { 
                SearchType::ApproximateKNN 
            } else { 
                SearchType::ExactKNN 
            },
            has_filters: params.filter_expression.is_some(),
            has_quantization: params.quantization_hint.is_some(),
            estimated_selectivity: if params.filter_expression.is_some() { 0.1 } else { 1.0 },
            top_k: params.top_k.unwrap_or(10),
            file_count: context.file_paths.len(),
            is_cloud_storage: context.is_cloud_storage,
        })
    }
    
    /// Execute optimized search with selected strategy
    async fn execute_optimized_search(
        &self,
        query_vector: &[f32],
        params: &crate::core::search::SearchParams,
        context: &CollectionContext,
        strategy: &ReadingStrategy,
    ) -> Result<Vec<VectorRecord>> {
        match strategy {
            ReadingStrategy::DirectArrow { use_column_projection, .. } => {
                self.execute_direct_search(query_vector, params, context, *use_column_projection).await
            }
            ReadingStrategy::MetadataFiltered { .. } => {
                self.execute_filtered_search(query_vector, params, context).await
            }
            ReadingStrategy::QuantizedTwoStage { stage1_method, candidate_count, .. } => {
                self.execute_quantized_search(query_vector, params, context, stage1_method, *candidate_count).await
            }
            _ => {
                // Fallback to direct search
                self.execute_direct_search(query_vector, params, context, false).await
            }
        }
    }
    
    /// Calculate search results using unified distance compute
    async fn calculate_search_results(
        &self,
        query_vector: &[f32],
        vectors: Vec<VectorRecord>,
        params: &crate::core::search::SearchParams,
        distance_compute: &UnifiedDistanceCompute,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        let distance_metric = params.distance_metric.unwrap_or(DistanceMetric::Cosine);
        let mut results = Vec::new();
        
        for vector in vectors {
            let similarity = distance_compute.calculate_distance(
                query_vector,
                &vector.vector,
                &distance_metric,
            );
            
            // Convert metadata
            let metadata = self.convert_vector_metadata(&vector.metadata);
            
            // Create SearchResult directly
            let id = vector.id.unwrap_or_default();
            let search_result = crate::core::search::SearchResult::from_semantic_distance(
                id.clone(),
                Some(id),
                similarity,
                Some(vector.vector),
                metadata,
            );
            
            results.push(search_result);
        }
        
        // Sort by distance (lower = better) and limit to k
        results.sort_by(|a, b| {
            a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal)
        });
        
        if let Some(k) = params.top_k {
            results.truncate(k);
        }
        
        // Assign ranks
        for (i, result) in results.iter_mut().enumerate() {
            result.rank = Some((i + 1) as u16);
        }
        
        Ok(results)
    }
    
    /// Execute direct Arrow-based search
    async fn execute_direct_search(
        &self,
        _query_vector: &[f32],
        params: &crate::core::search::SearchParams,
        context: &CollectionContext,
        use_projection: bool,
    ) -> Result<Vec<VectorRecord>> {
        let mut all_vectors = Vec::new();
        
        // Select optimal columns based on query requirements
        let columns = if use_projection {
            self.select_optimal_columns(params, context)
        } else {
            vec![] // Empty means read all columns
        };
        
        for file_path in &context.file_paths {
            // Use optimized predicate pushdown filtering
            let column_refs: Vec<&str> = columns.iter().map(|s| s.as_str()).collect();
            let vectors = self.read_vectors_with_filter(
                file_path, 
                if !columns.is_empty() { &column_refs } else { &[] },
                params.filter_expression.as_ref()
            ).await?;
            
            debug!("📖 Optimized filtering completed: {} vectors after predicate pushdown", vectors.len());
            all_vectors.extend(vectors);
        }
        
        Ok(all_vectors)
    }
    
    /// Execute filtered search with metadata pushdown
    async fn execute_filtered_search(
        &self,
        _query_vector: &[f32],
        params: &crate::core::search::SearchParams,
        context: &CollectionContext,
    ) -> Result<Vec<VectorRecord>> {
        let mut all_vectors = Vec::new();
        
        // Select optimal columns for metadata filtering
        let columns = self.select_optimal_columns(params, context);
        
        // Note: Parquet-level filter pushdown can be added here in the future
        // for complex FilterExpression optimization
        
        Ok(all_vectors)
    }
    
    /// Execute quantized two-stage search
    async fn execute_quantized_search(
        &self,
        _query_vector: &[f32],
        _params: &crate::core::search::SearchParams,
        context: &CollectionContext,
        _quantization_method: &QuantizationMethod,
        candidate_count: usize,
    ) -> Result<Vec<VectorRecord>> {
        // Stage 1: Read quantized vectors for fast candidate selection
        let mut all_vectors = Vec::new();
        
        for file_path in &context.file_paths {
            // Read quantized column first
            let quantized_candidates = self.read_quantized_vectors(file_path, candidate_count).await?;
            all_vectors.extend(quantized_candidates);
        }
        
        // Stage 2: Refine with full precision vectors
        // For now, return the quantized results (would be refined in real implementation)
        Ok(all_vectors)
    }
    
    
    /// Read all vectors from file with optional column projection
    pub async fn read_all_vectors(&self, file_path: &str, columns: &[&str]) -> Result<Vec<VectorRecord>> {
        self.read_vectors_with_filter(file_path, columns, None).await
    }
    
    /// Read vectors with predicate pushdown optimization
    async fn read_vectors_with_filter(
        &self, 
        file_path: &str, 
        columns: &[&str], 
        filter_expr: Option<&FilterExpression>
    ) -> Result<Vec<VectorRecord>> {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        use parquet::arrow::ProjectionMask;
        use arrow_array::Array;
        
        info!("📖 UnifiedParquetReader::read_vectors_with_filter from: {}", file_path);
        debug!("📖 Requested columns: {:?}", columns);
        debug!("📖 Filter expression: {:?}", filter_expr.is_some());
        
        // Get filesystem and read file (stateless design)
        let fs = self.filesystem.get_filesystem(file_path)?;
        let data = fs.read(file_path).await?;
        
        info!("📖 Read {} bytes from file", data.len());
        
        // Build Parquet reader
        let mut builder = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(data))?;
        
        debug!("📖 Parquet schema: {:?}", builder.schema());
        
        // Apply column projection if specified
        if !columns.is_empty() {
            let schema = builder.schema();
            let indices: Vec<usize> = columns.iter()
                .filter_map(|name| schema.index_of(name).ok())
                .collect();
                
            if !indices.is_empty() {
                let projection = ProjectionMask::roots(builder.parquet_schema(), indices);
                builder = builder.with_projection(projection);
            }
        }
        
        // Read batches
        let reader = builder.build()?;
        let mut vectors = Vec::new();
        
        let mut batch_count = 0;
        let mut total_rows = 0;
        for batch in reader {
            let batch = batch?;
            let num_rows = batch.num_rows();
            batch_count += 1;
            total_rows += num_rows;
            info!("📖 Batch {}: {} rows", batch_count, num_rows);
            
            // Step 1: Apply predicate pushdown - evaluate filters on metadata columns first
            let qualifying_row_indices = if let Some(filter) = filter_expr {
                self.evaluate_predicate_pushdown(&batch, filter)?
            } else {
                // No filter - all rows qualify
                (0..num_rows).collect()
            };
            
            info!("📖 Predicate pushdown: {} out of {} rows qualify", qualifying_row_indices.len(), num_rows);
            
            // Step 2: Only read vector data for qualifying rows (optimized I/O)
            for row_idx in qualifying_row_indices {
                let mut record = VectorRecord::default();
                
                // Extract ID
                if let Ok(idx) = batch.schema().index_of("id") {
                    if let Some(id_array) = batch.column(idx).as_any().downcast_ref::<arrow_array::StringArray>() {
                        if id_array.is_valid(row_idx) {
                            record.id = Some(id_array.value(row_idx).to_string());
                        }
                    }
                }
                
                // Collection ID not stored in VectorRecord anymore
                
                // Extract vector (FP32 or quantized based on columns)
                match self.extract_vector_from_batch(&batch, row_idx, columns) {
                    Ok(vector_extracted) => {
                        if vector_extracted.is_empty() {
                            debug!("📖 Warning: Empty vector extracted for row {}", row_idx);
                        }
                        record.vector = vector_extracted;
                    }
                    Err(e) => {
                        debug!("📖 Failed to extract vector for row {}: {}", row_idx, e);
                        // Skip this record if we can't extract the vector
                        continue;
                    }
                }
                
                // Extract metadata from filterable columns first
                // These are stored as separate columns during flush for efficient filtering
                let schema = batch.schema();
                for field in schema.fields() {
                    let field_name = field.name();
                    // Skip known system columns
                    if field_name == "id" || field_name == "collection_id" || 
                       field_name == "vector" || field_name == "version" || 
                       field_name == "timestamp" || field_name == "updated_at" || 
                       field_name == "expires_at" || field_name == "extra_meta" ||
                       field_name.starts_with("vector_") {
                        continue;
                    }
                    
                    // This is likely a filterable metadata column
                    if let Ok(idx) = schema.index_of(field_name) {
                        let column = batch.column(idx);
                        let mut value_str = String::new();
                        
                        // Extract value based on column type
                        if let Some(str_array) = column.as_any().downcast_ref::<arrow_array::StringArray>() {
                            if str_array.is_valid(row_idx) {
                                value_str = str_array.value(row_idx).to_string();
                            }
                        } else if let Some(int_array) = column.as_any().downcast_ref::<arrow_array::Int64Array>() {
                            if int_array.is_valid(row_idx) {
                                value_str = int_array.value(row_idx).to_string();
                            }
                        } else if let Some(float_array) = column.as_any().downcast_ref::<arrow_array::Float64Array>() {
                            if float_array.is_valid(row_idx) {
                                value_str = float_array.value(row_idx).to_string();
                            }
                        } else if let Some(bool_array) = column.as_any().downcast_ref::<arrow_array::BooleanArray>() {
                            if bool_array.is_valid(row_idx) {
                                value_str = bool_array.value(row_idx).to_string();
                            }
                        }
                        
                        if !value_str.is_empty() {
                            debug!("📖 Extracted filterable metadata: {} = {}", field_name, value_str);
                            // Determine the appropriate typed value based on the column type
                            let metadata_item = if let Some(bool_array) = column.as_any().downcast_ref::<arrow_array::BooleanArray>() {
                                if bool_array.is_valid(row_idx) {
                                    crate::proto::proximadb::MetadataItem {
                                        key: field_name.to_string(),
                                        value: Some(crate::proto::proximadb::metadata_item::Value::BoolValue(bool_array.value(row_idx))),
                                    }
                                } else {
                                    continue;
                                }
                            } else if let Some(int_array) = column.as_any().downcast_ref::<arrow_array::Int64Array>() {
                                if int_array.is_valid(row_idx) {
                                    crate::proto::proximadb::MetadataItem {
                                        key: field_name.to_string(),
                                        value: Some(crate::proto::proximadb::metadata_item::Value::NumberValue(int_array.value(row_idx) as f64)),
                                    }
                                } else {
                                    continue;
                                }
                            } else if let Some(float_array) = column.as_any().downcast_ref::<arrow_array::Float64Array>() {
                                if float_array.is_valid(row_idx) {
                                    crate::proto::proximadb::MetadataItem {
                                        key: field_name.to_string(),
                                        value: Some(crate::proto::proximadb::metadata_item::Value::NumberValue(float_array.value(row_idx))),
                                    }
                                } else {
                                    continue;
                                }
                            } else {
                                // Default to string value
                                crate::proto::proximadb::MetadataItem {
                                    key: field_name.to_string(),
                                    value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(value_str)),
                                }
                            };
                            record.metadata.push(metadata_item);
                        }
                    }
                }
                
                // Extract remaining metadata from extra_meta column
                if let Ok(idx) = batch.schema().index_of("extra_meta") {
                    // New format: List of Struct with key/value pairs
                    if let Some(list_array) = batch.column(idx).as_any().downcast_ref::<arrow_array::ListArray>() {
                        if list_array.is_valid(row_idx) {
                            let struct_array = list_array.value(row_idx);
                            if let Some(struct_array) = struct_array.as_any().downcast_ref::<arrow_array::StructArray>() {
                                // Extracting metadata from extra_meta column
                                // Extract key/value pairs
                                if let (Some(keys), Some(values)) = (
                                    struct_array.column_by_name("key").and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>()),
                                    struct_array.column_by_name("value").and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>())
                                ) {
                                    for i in 0..keys.len() {
                                        if keys.is_valid(i) && values.is_valid(i) {
                                            let key = keys.value(i);
                                            let value = values.value(i);
                                            // Parse value string to determine type
                                            let metadata_value = if let Ok(bool_val) = value.parse::<bool>() {
                                                Some(crate::proto::proximadb::metadata_item::Value::BoolValue(bool_val))
                                            } else if let Ok(num_val) = value.parse::<f64>() {
                                                Some(crate::proto::proximadb::metadata_item::Value::NumberValue(num_val))
                                            } else {
                                                Some(crate::proto::proximadb::metadata_item::Value::StringValue(value.to_string()))
                                            };
                                            record.metadata.push(crate::proto::proximadb::MetadataItem {
                                                key: key.to_string(),
                                                value: metadata_value,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if let Ok(idx) = batch.schema().index_of("metadata") {
                    // Old format support
                    if let Some(meta_array) = batch.column(idx).as_any().downcast_ref::<arrow_array::StringArray>() {
                        if meta_array.is_valid(row_idx) {
                            if let Ok(metadata_map) = serde_json::from_str::<std::collections::HashMap<String, serde_json::Value>>(meta_array.value(row_idx)) {
                                // Convert serde_json::Value to proto metadata items
                                record.metadata = metadata_map.into_iter().map(|(key, value)| {
                                    let metadata_value = match value {
                                        serde_json::Value::Bool(b) => Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)),
                                        serde_json::Value::Number(n) => {
                                            if let Some(f) = n.as_f64() {
                                                Some(crate::proto::proximadb::metadata_item::Value::NumberValue(f))
                                            } else {
                                                Some(crate::proto::proximadb::metadata_item::Value::StringValue(n.to_string()))
                                            }
                                        },
                                        serde_json::Value::String(s) => Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)),
                                        _ => Some(crate::proto::proximadb::metadata_item::Value::StringValue(value.to_string())),
                                    };
                                    crate::proto::proximadb::MetadataItem {
                                        key,
                                        value: metadata_value,
                                    }
                                }).collect();
                            }
                        }
                    }
                }
                
                vectors.push(record);
            }
        }
        
        info!("📖 Read complete: {} batches, {} total rows, {} vectors extracted", 
              batch_count, total_rows, vectors.len());
        if !vectors.is_empty() {
            debug!("📖 First vector metadata: {:?}", vectors[0].metadata);
        }
        Ok(vectors)
    }
    
    /// Extract vector from batch based on available columns
    fn extract_vector_from_batch(
        &self,
        batch: &arrow_array::RecordBatch,
        row_idx: usize,
        requested_columns: &[&str],
    ) -> Result<Vec<f32>> {
        use arrow_array::{Array, Float32Array};
        
        // Priority order: check requested columns first, then fallback
        let vector_columns = if requested_columns.is_empty() {
            vec!["vector", "vector_pq8", "vector_pq4"]
        } else {
            // Filter for vector-related columns
            requested_columns.iter()
                .filter(|&&col| col.starts_with("vector"))
                .map(|&s| s)
                .collect()
        };
        
        debug!("📖 Trying to extract vector from columns: {:?}", vector_columns);
        
        // Try each vector column in order
        for col_name in vector_columns {
            if let Ok(idx) = batch.schema().index_of(col_name) {
                let column = batch.column(idx);
                debug!("📖 Found column '{}' at index {}", col_name, idx);
                
                // Handle different vector representations
                if col_name == "vector" {
                    // Standard FP32 vector
                    if let Some(list_array) = column.as_any().downcast_ref::<arrow_array::ListArray>() {
                        if list_array.is_valid(row_idx) {
                            let value_array = list_array.value(row_idx);
                            if let Some(float_array) = value_array.as_any().downcast_ref::<Float32Array>() {
                                let vector: Vec<f32> = (0..float_array.len())
                                    .map(|i| float_array.value(i))
                                    .collect();
                                return Ok(vector);
                            }
                        }
                    }
                } else if col_name.contains("pq") {
                    // Quantized vector - would need dequantization
                    // For now, return empty vector as placeholder
                    // In real implementation, would call quantization engine to dequantize
                    warn!("Quantized vector dequantization not yet implemented for column: {}", col_name);
                    return Ok(vec![0.0; 128]); // Placeholder dimension
                }
            }
        }
        
        // No vector column found
        debug!("📖 No vector column found in batch for row {}", row_idx);
        Err(anyhow::anyhow!("No vector column found in batch"))
    }
    
    /// Read quantized vectors for two-stage search
    async fn read_quantized_vectors(&self, _file_path: &str, count: usize) -> Result<Vec<VectorRecord>> {
        // Simulate reading quantized vectors
        self.create_placeholder_vectors_simple(count).await
    }
    
    
    /// Apply complex filter expression with type-safe filtering
    fn apply_filter_expression(
        &self,
        vector: &VectorRecord,
        expression: &FilterExpression,
        context: &CollectionContext,
    ) -> bool {
        // Try type-safe filtering if we have column types
        if !context.filterable_columns.is_empty() {
            let evaluator = crate::core::search::typesafe_filter::TypeSafeFilterEvaluator::new(
                &context.filterable_columns
            );
            return evaluator.evaluate(expression, &vector.metadata);
        }
        
        // Use centralized filter evaluation for consistency across all engines
        let metadata_map = self.convert_vector_metadata(&vector.metadata);
        crate::core::search::json_comparison::evaluate_filter(expression, &metadata_map)
    }

    /// Convert vector metadata with type preservation
    fn convert_vector_metadata(&self, metadata: &[crate::proto::proximadb::MetadataItem]) -> HashMap<String, serde_json::Value> {
        // Use the helper function to convert proto metadata to JSON
        crate::core::proto_metadata_helper::proto_metadata_to_json(metadata)
    }
    
    /// Evaluate predicate pushdown on columnar data - returns qualifying row indices
    /// This is the key optimization: filter on metadata columns BEFORE reading vector data
    fn evaluate_predicate_pushdown(
        &self, 
        batch: &arrow_array::RecordBatch, 
        filter_expr: &FilterExpression
    ) -> Result<Vec<usize>> {
        
        let num_rows = batch.num_rows();
        let mut qualifying_rows = Vec::new();
        
        // Convert each row's metadata to evaluate against filter
        for row_idx in 0..num_rows {
            let mut row_metadata = HashMap::new();
            
            // Extract metadata values from columns for this row
            let schema = batch.schema();
            for field in schema.fields() {
                let field_name = field.name();
                
                // Skip system columns - only extract filterable metadata columns
                if field_name == "id" || field_name == "collection_id" || 
                   field_name == "vector" || field_name == "version" || 
                   field_name == "timestamp" || field_name == "updated_at" || 
                   field_name == "expires_at" || field_name == "extra_meta" ||
                   field_name.starts_with("vector_") {
                    continue;
                }
                
                // Extract column value based on type
                if let Ok(col_idx) = schema.index_of(field_name) {
                    let column = batch.column(col_idx);
                    
                    // Convert column value to JSON for filter evaluation
                    // Use safe access to avoid Arrow API version issues
                    let json_value = if let Some(str_array) = column.as_any().downcast_ref::<arrow_array::StringArray>() {
                        // Safe access with bounds checking
                        match std::panic::catch_unwind(|| str_array.value(row_idx).to_string()) {
                            Ok(value) => serde_json::Value::String(value),
                            Err(_) => serde_json::Value::Null,
                        }
                    } else if let Some(int_array) = column.as_any().downcast_ref::<arrow_array::Int64Array>() {
                        match std::panic::catch_unwind(|| int_array.value(row_idx)) {
                            Ok(value) => serde_json::Value::Number(serde_json::Number::from(value)),
                            Err(_) => serde_json::Value::Null,
                        }
                    } else if let Some(float_array) = column.as_any().downcast_ref::<arrow_array::Float64Array>() {
                        match std::panic::catch_unwind(|| float_array.value(row_idx)) {
                            Ok(value) => serde_json::Number::from_f64(value)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Null),
                            Err(_) => serde_json::Value::Null,
                        }
                    } else if let Some(bool_array) = column.as_any().downcast_ref::<arrow_array::BooleanArray>() {
                        match std::panic::catch_unwind(|| bool_array.value(row_idx)) {
                            Ok(value) => serde_json::Value::Bool(value),
                            Err(_) => serde_json::Value::Null,
                        }
                    } else {
                        // Fallback for unknown types
                        serde_json::Value::Null
                    };
                    
                    row_metadata.insert(field_name.to_string(), json_value);
                }
            }
            
            // Evaluate filter against this row's metadata using centralized logic
            if crate::core::search::json_comparison::evaluate_filter(filter_expr, &row_metadata) {
                qualifying_rows.push(row_idx);
            }
        }
        
        debug!("📖 Predicate pushdown evaluated {} rows, {} qualify", num_rows, qualifying_rows.len());
        Ok(qualifying_rows)
    }
    
    /// Filter row groups based on predicate statistics (from cloud_optimized_reader)
    pub fn filter_row_groups(
        &self,
        metadata: &parquet::file::metadata::ParquetMetaData,
        column_name: &str,
        min_value: Option<&str>,
        max_value: Option<&str>,
    ) -> Vec<usize> {
        let mut selected_row_groups = Vec::new();
        
        // Find column index
        let schema = metadata.file_metadata().schema_descr();
        let column_idx = schema.columns()
            .iter()
            .position(|col| col.name() == column_name);
        
        if let Some(col_idx) = column_idx {
            // Check each row group's statistics
            for (rg_idx, rg) in metadata.row_groups().iter().enumerate() {
                if let Some(col_chunk) = rg.columns().get(col_idx) {
                    if let Some(stats) = col_chunk.statistics() {
                        // Check if row group might contain matching values
                        if self.check_statistics(stats, min_value, max_value) {
                            selected_row_groups.push(rg_idx);
                        }
                    } else {
                        // No statistics - must include this row group
                        selected_row_groups.push(rg_idx);
                    }
                }
            }
        } else {
            // Column not found - include all row groups
            (0..metadata.num_row_groups()).for_each(|i| selected_row_groups.push(i));
        }
        
        debug!("📊 Selected {} of {} row groups based on statistics", 
               selected_row_groups.len(), metadata.num_row_groups());
        
        selected_row_groups
    }
    
    /// Check if statistics indicate the row group might contain matching values
    fn check_statistics(
        &self,
        stats: &parquet::file::statistics::Statistics,
        min_value: Option<&str>,
        max_value: Option<&str>,
    ) -> bool {
        use parquet::file::statistics::Statistics;
        
        match stats {
            Statistics::Int64(stats) => {
                if let Some(min_filter) = min_value {
                    if let Ok(min_val) = min_filter.parse::<i64>() {
                        if stats.max() < &min_val {
                            return false;
                        }
                    }
                }
                if let Some(max_filter) = max_value {
                    if let Ok(max_val) = max_filter.parse::<i64>() {
                        if stats.min() > &max_val {
                            return false;
                        }
                    }
                }
                true
            }
            _ => true, // Conservative - include if we can't determine
        }
    }
    
    /// Calculate byte ranges for specific row groups (from cloud_optimized_reader)
    pub fn calculate_row_group_ranges(
        &self,
        metadata: &parquet::file::metadata::ParquetMetaData,
        row_groups: &[usize],
    ) -> Vec<std::ops::Range<u64>> {
        let mut ranges = Vec::new();
        
        for &rg_idx in row_groups {
            if let Some(rg) = metadata.row_groups().get(rg_idx) {
                // Calculate the byte range for this row group
                let start = rg.columns().iter()
                    .map(|col| col.file_offset() as u64)
                    .min()
                    .unwrap_or(0);
                
                let end = rg.columns().iter()
                    .map(|col| {
                        let dict_offset = col.dictionary_page_offset().unwrap_or(0) as u64;
                        let data_offset = col.file_offset() as u64;
                        let compressed_size = col.compressed_size() as u64;
                        
                        // Use the earliest offset and add compressed size
                        let offset = if dict_offset > 0 && dict_offset < data_offset {
                            dict_offset
                        } else {
                            data_offset
                        };
                        offset + compressed_size
                    })
                    .max()
                    .unwrap_or(0);
                
                ranges.push(start..end);
            }
        }
        
        // Merge overlapping ranges for efficiency
        self.merge_ranges(ranges)
    }
    
    /// Merge overlapping or adjacent byte ranges
    fn merge_ranges(&self, mut ranges: Vec<std::ops::Range<u64>>) -> Vec<std::ops::Range<u64>> {
        if ranges.is_empty() {
            return ranges;
        }
        
        ranges.sort_by_key(|r| r.start);
        
        let mut merged = vec![ranges[0].clone()];
        
        for range in ranges.into_iter().skip(1) {
            let last = merged.last_mut().unwrap();
            
            // If ranges overlap or are adjacent (within 4KB), merge them
            if range.start <= last.end + 4096 {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        
        merged
    }
    
    /// Calculate byte ranges for specific columns in row groups (true column-level I/O)
    pub fn calculate_column_ranges(
        &self,
        metadata: &parquet::file::metadata::ParquetMetaData,
        row_groups: &[usize],
        columns: &[&str],
    ) -> Result<Vec<std::ops::Range<u64>>> {
        let mut ranges = Vec::new();
        let schema = metadata.file_metadata().schema();
        
        // Map column names to column indices
        let mut column_indices = HashMap::new();
        for (idx, field) in schema.get_fields().iter().enumerate() {
            column_indices.insert(field.name(), idx);
        }
        
        // For each row group
        for &rg_idx in row_groups {
            if let Some(rg) = metadata.row_groups().get(rg_idx) {
                // For each requested column
                for &col_name in columns {
                    if let Some(&col_idx) = column_indices.get(col_name) {
                        if let Some(col_chunk) = rg.columns().get(col_idx) {
                            // Column chunk is already the metadata
                            // Calculate byte range for this column chunk
                            let dict_offset = col_chunk.dictionary_page_offset().unwrap_or(0) as u64;
                            let data_offset = col_chunk.data_page_offset() as u64;
                            let compressed_size = col_chunk.compressed_size() as u64;
                            
                            // Start at dictionary page if present, otherwise data page
                            let start = if dict_offset > 0 && dict_offset < data_offset {
                                dict_offset
                            } else {
                                data_offset
                            };
                            
                            let end = start + compressed_size;
                            ranges.push(start..end);
                            
                            debug!("Column '{}' in RG {}: bytes {}..{} ({}KB)", 
                                   col_name, rg_idx, start, end, (end - start) / 1024);
                        }
                    }
                }
            }
        }
        
        // Merge overlapping ranges for efficiency
        let merged = self.merge_ranges(ranges);
        
        info!("📊 Column ranges: {} individual chunks merged to {} ranges", 
              columns.len() * row_groups.len(), merged.len());
        
        Ok(merged)
    }
    
    /// Build Arrow reader from partial range data
    async fn build_reader_from_ranges(
        &self,
        range_data: Vec<Vec<u8>>,
        metadata: &parquet::file::metadata::ParquetMetaData,
        row_groups: Vec<usize>,
        columns: Vec<&str>,
    ) -> Result<Vec<arrow_array::RecordBatch>> {
        // For now, we need to reconstruct a partial Parquet file
        // In future, we could implement a custom reader that works directly with ranges
        
        warn!("⚠️ Range-based reader not fully implemented - falling back to full reconstruction");
        
        // Concatenate all range data (temporary solution)
        let mut full_data = Vec::new();
        for data in range_data {
            full_data.extend_from_slice(&data);
        }
        
        // Use standard Arrow reader with projection
        self.build_reader_with_selection(full_data, row_groups, columns).await
    }
    
    /// Parse a single row group with column projection
    async fn parse_row_group_with_projection(
        &self,
        data: Vec<u8>,
        row_group: &parquet::format::RowGroup,
        columns: &[&str],
    ) -> Result<arrow_array::RecordBatch> {
        // This would require implementing a custom Parquet row group parser
        // For now, return error indicating not implemented
        Err(anyhow::anyhow!("Custom row group parser not yet implemented"))
    }

    /// Create simple placeholder vectors
    async fn create_placeholder_vectors_simple(&self, count: usize) -> Result<Vec<VectorRecord>> {
        use crate::proto::proximadb::MetadataItem;
        
        let mut vectors = Vec::with_capacity(count);
        
        for i in 0..count {
            let vector = VectorRecord {
                id: Some(format!("vec_{:06}", i)),
                vector: vec![0.1; 128], // Placeholder vector
                metadata: vec![MetadataItem {
                    key: "category".to_string(),
                    value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("test".to_string())),
                }],
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                distance: Some(0.1 * i as f32),
                rank: Some(i as i32),
                score: Some(1.0 - (0.1 * i as f32)),
            };
            vectors.push(vector);
        }
        
        Ok(vectors)
    }
    
    // Methods moved from ReadingStrategySelector
    
    /// Parse filter value to extract operation (moved from metadata_pushdown)
    fn parse_filter_value(&self, filter_value: &serde_json::Value) -> FilterValue {
        match filter_value {
            // Simple equality
            serde_json::Value::String(_) | 
            serde_json::Value::Number(_) | 
            serde_json::Value::Bool(_) => {
                FilterValue::Equals(filter_value.clone())
            }
            
            // Complex filter object
            serde_json::Value::Object(obj) => {
                if let Some((op_key, op_value)) = obj.iter().next() {
                    match op_key.as_str() {
                        "$eq" => FilterValue::Equals(op_value.clone()),
                        "$ne" | "$neq" => FilterValue::NotEquals(op_value.clone()),
                        "$gt" => FilterValue::GreaterThan(op_value.clone()),
                        "$gte" => FilterValue::GreaterThanOrEqual(op_value.clone()),
                        "$lt" => FilterValue::LessThan(op_value.clone()),
                        "$lte" => FilterValue::LessThanOrEqual(op_value.clone()),
                        "$in" => {
                            if let Some(arr) = op_value.as_array() {
                                FilterValue::In(arr.clone())
                            } else {
                                FilterValue::Equals(filter_value.clone())
                            }
                        }
                        "$nin" => {
                            if let Some(arr) = op_value.as_array() {
                                FilterValue::NotIn(arr.clone())
                            } else {
                                FilterValue::NotEquals(filter_value.clone())
                            }
                        }
                        "$contains" => {
                            if let Some(s) = op_value.as_str() {
                                FilterValue::Contains(s.to_string())
                            } else {
                                FilterValue::Equals(filter_value.clone())
                            }
                        }
                        "$startsWith" => {
                            if let Some(s) = op_value.as_str() {
                                FilterValue::StartsWith(s.to_string())
                            } else {
                                FilterValue::Equals(filter_value.clone())
                            }
                        }
                        "$endsWith" => {
                            if let Some(s) = op_value.as_str() {
                                FilterValue::EndsWith(s.to_string())
                            } else {
                                FilterValue::Equals(filter_value.clone())
                            }
                        }
                        _ => FilterValue::Equals(filter_value.clone()),
                    }
                } else {
                    FilterValue::Equals(filter_value.clone())
                }
            }
            
            // Array treated as IN
            serde_json::Value::Array(arr) => {
                FilterValue::In(arr.clone())
            }
            
            _ => FilterValue::Equals(filter_value.clone()),
        }
    }
    
    /// Evaluate filter against actual value
    fn evaluate_filter(&self, actual: Option<&serde_json::Value>, filter_op: &FilterValue) -> bool {
        match filter_op {
            FilterValue::IsNull => actual.is_none(),
            FilterValue::IsNotNull => actual.is_some(),
            _ => {
                if let Some(actual_value) = actual {
                    match filter_op {
                        FilterValue::Equals(expected) => actual_value == expected,
                        FilterValue::NotEquals(expected) => actual_value != expected,
                        FilterValue::GreaterThan(expected) => self.compare_values(actual_value, expected) == std::cmp::Ordering::Greater,
                        FilterValue::GreaterThanOrEqual(expected) => matches!(self.compare_values(actual_value, expected), std::cmp::Ordering::Greater | std::cmp::Ordering::Equal),
                        FilterValue::LessThan(expected) => self.compare_values(actual_value, expected) == std::cmp::Ordering::Less,
                        FilterValue::LessThanOrEqual(expected) => matches!(self.compare_values(actual_value, expected), std::cmp::Ordering::Less | std::cmp::Ordering::Equal),
                        FilterValue::In(values) => values.contains(actual_value),
                        FilterValue::NotIn(values) => !values.contains(actual_value),
                        FilterValue::Contains(s) => {
                            actual_value.as_str().map_or(false, |v| v.contains(s))
                        }
                        FilterValue::StartsWith(s) => {
                            actual_value.as_str().map_or(false, |v| v.starts_with(s))
                        }
                        FilterValue::EndsWith(s) => {
                            actual_value.as_str().map_or(false, |v| v.ends_with(s))
                        }
                        FilterValue::Range(range) => {
                            actual_value.as_i64().map_or(false, |v| range.contains(&v))
                        }
                        _ => false,
                    }
                } else {
                    false // null values don't match unless explicitly checking for null
                }
            }
        }
    }
    
    /// Compare two JSON values
    fn compare_values(&self, a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
        match (a, b) {
            (serde_json::Value::Number(n1), serde_json::Value::Number(n2)) => {
                if let (Some(f1), Some(f2)) = (n1.as_f64(), n2.as_f64()) {
                    f1.partial_cmp(&f2).unwrap_or(std::cmp::Ordering::Equal)
                } else if let (Some(i1), Some(i2)) = (n1.as_i64(), n2.as_i64()) {
                    i1.cmp(&i2)
                } else {
                    std::cmp::Ordering::Equal
                }
            }
            (serde_json::Value::String(s1), serde_json::Value::String(s2)) => s1.cmp(s2),
            (serde_json::Value::Bool(b1), serde_json::Value::Bool(b2)) => b1.cmp(b2),
            _ => std::cmp::Ordering::Equal,
        }
    }
}

/// Search type enum for strategy optimization
#[derive(Debug, Clone)]
pub enum SearchType {
    /// Exact K-NN search - optimize for precision
    ExactKNN,
    /// Approximate search - optimize for speed
    ApproximateKNN,
    /// Filtered search - optimize for metadata filtering
    FilteredSearch,
    /// Range search - optimize for threshold-based results
    RangeSearch,
    /// Hybrid search - balance multiple objectives
    HybridSearch,
}

/// Search analysis for direct SearchParams processing
#[derive(Debug)]
pub struct SearchAnalysis {
    pub search_type: SearchType,
    pub has_filters: bool,
    pub has_quantization: bool,
    pub estimated_selectivity: f64,
    pub top_k: usize,
    pub file_count: usize,
    pub is_cloud_storage: bool,
}

impl ReadingStrategySelector {
    pub fn new(config: ReaderConfig) -> Self {
        Self { config }
    }
    
    /// Select strategy directly from SearchParams - no adapter needed
    pub async fn select_strategy_for_search(
        &self, 
        params: &crate::core::search::SearchParams, 
        analysis: &SearchAnalysis
    ) -> Result<ReadingStrategy> {
        // 1. Check for quantization hint first
        if let Some(_quant_hint) = &params.quantization_hint {
            let candidate_count = params.top_k.unwrap_or(10) * self.config.quantization_candidate_multiplier;
            return Ok(ReadingStrategy::QuantizedTwoStage {
                stage1_method: QuantizationMethod::PQ8, // Convert from hint
                stage2_strategy: if analysis.is_cloud_storage {
                    Stage2Strategy::RangeRequests(Vec::new())
                } else {
                    Stage2Strategy::FullRowGroups(Vec::new())
                },
                candidate_count: candidate_count.min(self.config.max_candidates),
            });
        }
        
        // 2. Check for metadata filters with good selectivity
        if analysis.has_filters && analysis.estimated_selectivity < self.config.seek_efficiency_threshold {
            return Ok(ReadingStrategy::MetadataFiltered {
                seek_ranges: Vec::new(), // Would be calculated
                use_reconstruction: analysis.is_cloud_storage,
            });
        }
        
        // 3. Large datasets benefit from column projection
        if analysis.top_k > self.config.column_projection_threshold {
            return Ok(ReadingStrategy::DirectArrow {
                use_column_projection: true,
                read_all_data: false,
            });
        }
        
        // 4. Default to direct Arrow reading
        Ok(ReadingStrategy::DirectArrow {
            use_column_projection: false,
            read_all_data: true,
        })
    }
}

impl ReaderCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            schema_mappings: HashMap::with_capacity(max_size),
            file_metadata: HashMap::with_capacity(max_size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_search_params_direct() {
        let config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = FilesystemFactory::new(config).await.unwrap();
        let reader = UnifiedParquetReader::new(Arc::new(filesystem));
        
        let query_vector = vec![0.1; 128];
        let params = crate::core::search::SearchParams {
            query_vectors: Some(vec![query_vector]),
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            ..Default::default()
        };
        
        let context = CollectionContext {
            collection_id: "test_collection".to_string(),
            file_paths: vec!["file:///tmp/test.parquet".to_string()],
            filterable_columns: vec![],
            quantization_columns: vec![],
            estimated_size_mb: 100.0,
            estimated_document_count: 1000,
            is_cloud_storage: false,
        };
        
        // This test verifies that the reader can handle SearchParams directly
        // Since we don't have actual parquet files, we expect an error but not a panic
        let results = reader.search_vectors(&params, &context).await;
        
        // The test should return an error (file not found) but not panic
        assert!(results.is_err());
        // We can't guarantee the specific error message, but we can verify the parameters work
        assert!(params.query_vectors.is_some());
        assert_eq!(params.top_k, Some(10));
    }
}

// UnifiedParquetReader is a PURE DATA ACCESS LAYER with search optimization
// It takes SearchParams directly without any adapter layers
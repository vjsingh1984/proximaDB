//! Unified Parquet Reader - Main entry point for query operations
//!
//! This module provides the UnifiedParquetReader that other parts of the
//! codebase expect, delegating to the appropriate modular components.

use anyhow::Result;
use arrow::datatypes::Schema;
use std::sync::Arc;
use std::collections::HashMap;
use crate::proto::proximadb_v1::{VectorRecord, MetadataFilter};
use crate::core::search::SearchPlan;
// SearchResponse should come from proto
type SearchResponse = crate::proto::proximadb_v1::VectorSearchResponse;

use super::{
    ParquetReader,
    QueryConfig,
    CacheStrategy,
    BranchedFilterExecutor,
};

/// Reading strategy selector
#[derive(Debug, Clone)]
pub struct ReadingStrategySelector {
    pub enable_ipc_optimization: bool,
    pub ipc_size_threshold_mb: f64,
    pub enable_adaptive_selection: bool,
    pub cache_ipc_decisions: bool,
}

impl Default for ReadingStrategySelector {
    fn default() -> Self {
        Self {
            enable_ipc_optimization: true,
            ipc_size_threshold_mb: 100.0,
            enable_adaptive_selection: true,
            cache_ipc_decisions: true,
        }
    }
}

/// Schema mapping for columnar storage
#[derive(Debug, Clone)]
pub struct SchemaMapping {
    pub arrow_schema: Arc<Schema>,
    pub field_indices: HashMap<String, usize>,
    pub filterable_columns: Vec<String>,
}

/// Collection context for queries
#[derive(Debug, Clone)]
pub struct CollectionContext {
    pub collection_id: String,
    pub dimension: usize,
    pub distance_metric: String,
    pub quantization_config: Option<crate::proto::proximadb_v1::QuantizationConfig>,
}

/// Reading strategy for data access
#[derive(Debug, Clone, Copy)]
pub enum ReadingStrategy {
    /// Use Arrow IPC format for fast memory-mapped access
    ArrowIPC,
    /// Use standard Parquet reading
    Parquet,
    /// Choose automatically based on file characteristics
    Auto,
}

/// Reader configuration
#[derive(Debug, Clone)]
pub struct ReaderConfig {
    pub enable_pushdown_predicates: bool,
    pub enable_row_group_pruning: bool,
    pub enable_page_index: bool,
    pub batch_size: usize,
    pub cache_metadata: bool,
    pub parallel_row_groups: bool,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            enable_pushdown_predicates: true,
            enable_row_group_pruning: true,
            enable_page_index: true,
            batch_size: 8192,
            cache_metadata: true,
            parallel_row_groups: true,
        }
    }
}

/// Filter value for metadata filtering
#[derive(Debug, Clone)]
pub enum FilterValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    StringList(Vec<String>),
    IntegerList(Vec<i64>),
    FloatList(Vec<f64>),
}

/// Quantization method for vectors
#[derive(Debug, Clone, Copy)]
pub enum QuantizationMethod {
    None,
    Binary,
    Int8,
    PQ,
}

/// Seek range for targeted reads
#[derive(Debug, Clone)]
pub struct SeekRange {
    pub start: usize,
    pub end: usize,
    pub row_groups: Vec<usize>,
}

/// Vector position in storage
#[derive(Debug, Clone)]
pub struct VectorPosition {
    pub row_group: usize,
    pub row_offset: usize,
    pub global_row: usize,
}

/// Stage 2 search strategy
#[derive(Debug, Clone, Copy)]
pub enum Stage2Strategy {
    ExactSearch,
    ApproximateSearch,
    HybridSearch,
}

/// Search type
#[derive(Debug, Clone, Copy)]
pub enum SearchType {
    KNN,
    RadiusSearch,
    FilteredSearch,
}

/// Row group access pattern
#[derive(Debug, Clone, Copy)]
pub enum RowGroupAccessPattern {
    Sequential,
    Random,
    Filtered,
    All,
}

/// Page pruning info
#[derive(Debug, Clone)]
pub struct PagePruningInfo {
    pub total_pages: usize,
    pub pruned_pages: usize,
    pub pages_read: usize,
    pub pruning_effectiveness: f64,
}

/// Page range
#[derive(Debug, Clone)]
pub struct PageRange {
    pub start_page: usize,
    pub end_page: usize,
    pub row_count: usize,
}

/// Unified Parquet reader (main entry point for compatibility)
#[derive(Clone)]
pub struct UnifiedParquetReader {
    pub file_paths: Vec<String>,
    pub dimension: usize,
    pub config: ReaderConfig,
    pub schema_mapping: Option<SchemaMapping>,
}

impl UnifiedParquetReader {
    /// Create new reader
    pub fn new(file_paths: Vec<String>, dimension: usize) -> Result<Self> {
        Ok(Self {
            file_paths,
            dimension,
            config: ReaderConfig::default(),
            schema_mapping: None,
        })
    }

    /// Set configuration
    pub fn with_config(mut self, config: ReaderConfig) -> Self {
        self.config = config;
        self
    }

    /// Read all records
    pub async fn read_all_records(
        &self,
        start_offset: usize,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        let query_config = QueryConfig {
            enable_pushdown: self.config.enable_pushdown_predicates,
            enable_projection: true,
            enable_statistics: self.config.enable_row_group_pruning,
            cache_strategy: CacheStrategy::LRU,
            limit,
            enable_parallel: self.config.parallel_row_groups,
            parallel_workers: 4,
        };

        let mut reader = ParquetReader::new(query_config);
        let mut all_records = Vec::new();

        for file_path in &self.file_paths {
            let records = reader.read_all(file_path).await?;
            all_records.extend(records);

            if let Some(limit) = limit {
                if all_records.len() >= limit {
                    all_records.truncate(limit);
                    break;
                }
            }
        }

        // Apply start offset if needed
        if start_offset > 0 && start_offset < all_records.len() {
            all_records = all_records.into_iter().skip(start_offset).collect();
        }

        Ok(all_records)
    }

    /// Query with metadata filters
    pub async fn query_with_metadata_filters(
        &self,
        filters: &[MetadataFilter],
    ) -> Result<Vec<VectorRecord>> {
        let filterable_columns = self.schema_mapping
            .as_ref()
            .map(|s| s.filterable_columns.clone())
            .unwrap_or_default();

        // TODO: Re-enable after BranchedFilterExecutor API is fixed
        let _executor = BranchedFilterExecutor::new(
            filterable_columns,
            self.file_paths.clone(),
            self.dimension,
        );

        // Temporarily return empty results until API is ready
        Ok(Vec::new())
    }

    /// Query by IDs
    pub async fn query_by_ids(&self, ids: &[String]) -> Result<Vec<VectorRecord>> {
        // This would use the ID index for efficient lookup
        // For now, scan all files and filter
        let all_records = self.read_all_records(0, None).await?;

        let id_set: std::collections::HashSet<_> = ids.iter().cloned().collect();
        Ok(all_records
            .into_iter()
            .filter(|r| id_set.contains(&r.id))
            .collect())
    }

    /// Check if should use IPC format for scanning
    pub fn should_use_ipc_for_scan(&self, file_path: &str) -> bool {
        // Simple heuristic: use IPC for large files
        if let Ok(metadata) = std::fs::metadata(file_path) {
            let file_size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
            file_size_mb > 100.0 // Use IPC for files > 100MB
        } else {
            false
        }
    }

    /// Read specific row groups with projection
    /// TODO: Implement actual row group reading with projection
    pub async fn read_row_groups_projected(
        &self,
        _collection_id: &str,
        row_groups: &[usize],
        _projection: Option<&[String]>,
    ) -> Result<Vec<VectorRecord>> {
        // Stub implementation - read all and filter by row groups
        // In production, this should use Parquet's row group API directly
        // read_all_records expects usize for number of files, not &Vec<String>
        let all_records = self.read_all_records(&self.file_paths, None).await?;

        // For now, just return all records (row group filtering not implemented)
        // TODO: Implement actual row group filtering
        Ok(all_records)
    }

    /// Get collection context - returns metadata about the collection
    pub async fn get_collection_context(&self) -> CollectionContext {
        CollectionContext {
            dimension: self.dimension,
            collection_id: String::new(),
            distance_metric: "cosine".to_string(),
            quantization_config: None,
        }
    }

    /// Search vectors using unified search interface
    /// TODO: Implement actual vector search
    pub async fn search_vectors(
        &self,
        _search_plan: &SearchPlan,
        _collection_context: &CollectionContext,
    ) -> Result<SearchResponse> {
        // Stub implementation
        Ok(SearchResponse {
            results: vec![],
            total_results: 0,
            metadata: None,
        })
    }

    /// Read vectors for similarity search
    pub fn read_for_similarity_search(
        &self,
        _file_paths: &[String],
        _filter: Option<&MetadataFilter>,
        _top_k: usize,
    ) -> Result<Vec<VectorRecord>> {
        // Stub implementation - in production would use optimized search
        Ok(vec![])
    }
}
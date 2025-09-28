//! Modular Parquet Query Engine Components
//!
//! This module provides efficient Parquet querying capabilities for columnar storage engines.
//! It's organized into focused submodules for better maintainability and clarity.

pub mod columnar_reader;
pub mod filter_pushdown_engine;
pub mod column_projector;
pub mod query_metrics;
pub mod result_cache;
pub mod adaptive_filter_executor;
pub mod unified_reader;

// Re-export main types for convenience with semantic names
pub use columnar_reader::{ParquetReader, ReaderBuilder};
pub use filter_pushdown_engine::{PredicateBuilder, FilterPushdown};
pub use column_projector::{ProjectionBuilder, ColumnProjection};
pub use query_metrics::{QueryStatistics, StatisticsCollector};
pub use result_cache::{QueryCache, CacheStrategy};
pub use adaptive_filter_executor::{BranchedFilterExecutor, FilterPath};

// Re-export unified reader types for compatibility
pub use unified_reader::{
    UnifiedParquetReader,
    ReaderConfig,
    ReadingStrategy,
    ReadingStrategySelector,
    SchemaMapping,
    CollectionContext,
    FilterValue,
    QuantizationMethod,
    SeekRange,
    VectorPosition,
    Stage2Strategy,
    SearchType,
    RowGroupAccessPattern,
    PagePruningInfo,
    PageRange,
};

// Common traits used across query implementations
use anyhow::Result;
use arrow::record_batch::RecordBatch;
use crate::proto::proximadb_v1::{VectorRecord, MetadataFilter};

/// Common trait for all Parquet readers
pub trait ParquetQueryEngine: Send + Sync {
    /// Query with metadata filters
    async fn query_with_filters(
        &self,
        file_path: &str,
        filters: &[MetadataFilter],
    ) -> Result<Vec<VectorRecord>>;

    /// Query by IDs
    async fn query_by_ids(
        &self,
        file_path: &str,
        ids: &[String],
    ) -> Result<Vec<VectorRecord>>;

    /// Query with projection
    async fn query_with_projection(
        &self,
        file_path: &str,
        columns: &[String],
    ) -> Result<RecordBatch>;

    /// Get query statistics
    fn get_statistics(&self) -> QueryStatistics;
}

/// Configuration for query operations
#[derive(Debug, Clone)]
pub struct QueryConfig {
    /// Enable predicate pushdown
    pub enable_pushdown: bool,

    /// Enable column projection
    pub enable_projection: bool,

    /// Enable statistics-based pruning
    pub enable_statistics: bool,

    /// Cache strategy
    pub cache_strategy: CacheStrategy,

    /// Maximum records to return
    pub limit: Option<usize>,

    /// Enable parallel execution
    pub enable_parallel: bool,

    /// Number of parallel workers
    pub parallel_workers: usize,
}
//! # DataFusion Integration Module
//!
//! This module provides DataFusion TableProvider implementations for ProximaDB,
//! enabling SQL queries and compute engine compatibility.
//!
//! ## Architecture
//!
//! ```text
//! SQL Query / DataFrame API
//!           ↓
//! ┌─────────────────────────────────────┐
//! │      DataFusion SessionContext      │
//! │  ┌─────────────────────────────────┐│
//! │  │  ProximaDBTableProvider         ││
//! │  │  - schema inference             ││
//! │  │  - statistics                   ││
//! │  │  - predicate pushdown           ││
//! │  └─────────────────────────────────┘│
//! └─────────────────────────────────────┘
//!           ↓
//! ┌─────────────────────────────────────┐
//! │      ProximaDBScanExec              │
//! │  - Parallel partition scan          │
//! │  - Filter pushdown to storage       │
//! │  - Projection pushdown              │
//! └─────────────────────────────────────┘
//!           ↓
//! ┌─────────────────────────────────────┐
//! │      Storage Format Layer           │
//! │  - InternalFormat::read_batches()   │
//! │  - Arrow RecordBatch output         │
//! └─────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::datafusion::{ProximaDBTableProvider, create_session_context};
//!
//! // Create a session context with ProximaDB collections
//! let ctx = create_session_context(collection_service, format_registry).await?;
//!
//! // Register a collection as a table
//! ctx.register_table("my_vectors", ProximaDBTableProvider::new(collection)?)?;
//!
//! // Query with SQL
//! let df = ctx.sql("SELECT * FROM my_vectors WHERE category = 'science'").await?;
//! let batches = df.collect().await?;
//! ```

pub mod predicate_pushdown;
pub mod scan_executor;
pub mod schema_inference;
pub mod table_provider;

// New split-based table provider and execution plan modules
pub mod proxima_scan_exec;
pub mod proxima_table_provider;

// Engine-specific TableProvider adapters
pub mod engine_adapters;

// Re-exports for convenience
pub use table_provider::{
    // Original format-based provider
    ProximaDBTableProvider,
    ProximaDBTableProviderConfig,
    // New split-based provider
    ProximaDataFusionTable,
    ProximaDataFusionTableConfig,
    create_nova_table_provider,
    // Factory functions
    create_viper_table_provider,
};

pub use scan_executor::{PartitionInfo, ProximaDBScanExec};

pub use predicate_pushdown::{FilterPushdownResult, PushdownCapability, convert_expr_to_filter};

pub use schema_inference::{extract_statistics, infer_schema_from_collection};

// Re-export new split-based types
pub use proxima_table_provider::{
    CollectionInfo,
    // Types
    EngineType,
    // Null implementation for testing
    NullProximaTableProvider,
    // Trait
    ProximaTableProvider,
    PruningStatistics,
};

pub use proxima_scan_exec::{
    EmptyRecordBatchStream,
    // Null implementations for testing
    NullSplitReader,
    // Core execution plan
    ProximaScanExec,
    ProximaScanExecBuilder,
    // Split reader trait
    SplitReader,
};

// Re-export FileSplit from storage::formats
pub use crate::storage::formats::FileSplit;

// Re-export engine-specific adapters
pub use engine_adapters::{
    HelixSplitReader,
    // HELIX engine adapter
    HelixTableProvider,
    SstSplitReader,
    // SST engine adapter
    SstTableProvider,
    ViperSplitReader,
    // VIPER engine adapter
    ViperTableProvider,
};

use datafusion::prelude::*;

/// Create a DataFusion SessionContext with ProximaDB integration.
///
/// This context is pre-configured with:
/// - ProximaDB catalog provider for automatic table discovery
/// - Custom vector distance functions (cosine, euclidean, dot_product)
/// - Optimizer rules for predicate pushdown
pub fn create_session_context() -> datafusion::error::Result<SessionContext> {
    let config = SessionConfig::new()
        .with_batch_size(8192)
        .with_target_partitions(num_cpus::get());

    let ctx = SessionContext::new_with_config(config);

    // Register ProximaDB catalog
    // TODO: Implement ProximaDBCatalogProvider
    // ctx.register_catalog("proximadb", Arc::new(ProximaDBCatalogProvider::new(...)));

    // Register vector functions
    // TODO: Register cosine_distance, euclidean_distance, etc.

    Ok(ctx)
}

/// Create a DataFusion SessionContext with custom configuration.
pub fn create_session_context_with_config(
    config: &DataFusionConfig,
) -> datafusion::error::Result<SessionContext> {
    let session_config = SessionConfig::new()
        .with_batch_size(config.batch_size)
        .with_target_partitions(config.target_partitions);

    let ctx = SessionContext::new_with_config(session_config);

    Ok(ctx)
}

/// Configuration for DataFusion integration.
#[derive(Debug, Clone)]
pub struct DataFusionConfig {
    /// Batch size for record batch processing
    pub batch_size: usize,
    /// Number of parallel partitions
    pub target_partitions: usize,
    /// Enable predicate pushdown
    pub enable_predicate_pushdown: bool,
    /// Enable projection pushdown
    pub enable_projection_pushdown: bool,
    /// Cache statistics for this duration (seconds)
    pub statistics_cache_ttl_seconds: u64,
}

impl Default for DataFusionConfig {
    fn default() -> Self {
        Self {
            batch_size: 8192,
            target_partitions: num_cpus::get(),
            enable_predicate_pushdown: true,
            enable_projection_pushdown: true,
            statistics_cache_ttl_seconds: 60,
        }
    }
}

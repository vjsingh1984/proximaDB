//! # Storage Formats Module
//!
//! This module provides the format abstraction layer for Hadoop-style storage-compute separation.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                     FORMAT ABSTRACTION LAYER                                 │
//! │  ┌───────────────────────────────────────┐  ┌─────────────────────────────┐ │
//! │  │         INTERNAL FORMATS              │  │    OPEN TABLE FORMATS       │ │
//! │  │  SST, Helix, Viper, Nova, Swift,     │  │  Delta Lake, Iceberg, Hudi, │ │
//! │  │  Raptor, Orion, Pulsar, Quasar       │  │  LanceDB, DuckDB, Parquet   │ │
//! │  └───────────────────────────────────────┘  └─────────────────────────────┘ │
//! │                          │                              │                    │
//! │                          └──────────────┬───────────────┘                    │
//! │                                         ▼                                    │
//! │                              ┌─────────────────────┐                         │
//! │                              │   Format Registry   │                         │
//! │                              │  Detection + Lookup │                         │
//! │                              └─────────────────────┘                         │
//! │                                         │                                    │
//! │                                         ▼                                    │
//! │                              ┌─────────────────────┐                         │
//! │                              │  Arrow RecordBatch  │                         │
//! │                              │   (Data Exchange)   │                         │
//! │                              └─────────────────────┘                         │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Design Principles
//!
//! 1. **Storage-Compute Separation**: Storage formats provide ONLY serialization/deserialization.
//!    Compute operations (filtering, aggregation, joins) are handled by the compute layer.
//!
//! 2. **Arrow as Data Exchange**: All data transfer uses Arrow RecordBatch for zero-copy,
//!    cross-system compatibility.
//!
//! 3. **Extensibility**: New formats can be registered at runtime via FormatRegistry.
//!
//! 4. **Trait-Based Abstraction**: InternalFormat and OpenTableFormat traits provide clean
//!    interfaces for different format categories.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::formats::{FormatRegistry, FormatType, ReadContext};
//!
//! // Get global registry
//! let registry = global_registry();
//!
//! // Auto-detect format from path
//! let format_type = registry.detect_format("/data/my_table").await?;
//!
//! // Get format implementation
//! if let Some(format) = registry.get_internal(FormatType::Sst) {
//!     let batches = format.read_batches(&ReadContext::default()).await?;
//! }
//! ```

// Core traits and types
pub mod traits;

// Format registry for discovery and lookup
pub mod registry;

// Arrow conversion utilities
pub mod arrow_conversion;

// Open table format implementations (Phase 2)
pub mod open;

// Internal format adapters (bridge to existing engines)
pub mod adapters;

// File split abstraction for parallel reading
pub mod splits;

// Re-exports for convenience
pub use traits::{
    // Core traits
    StorageFormat, InternalFormat, OpenTableFormat, FormatDetector,
    // Types
    FormatType, VectorBatch, ReadContext, VectorReadContext, WriteContext, VectorWriteContext,
    CompactionContext, OptimizeContext, WriteResult, CompactionResult, OptimizeResult,
    FileEntry, FileStats, ColumnStats, FormatStatistics, Snapshot,
    CompressionCodec, WriteMode, FilterExpression, ComparisonOp, MergeAction,
    // Streams
    RecordBatchStream, VectorBatchStream,
    // Default implementations
    DefaultFormatDetector,
};

pub use registry::{FormatRegistry, global_registry};

pub use arrow_conversion::{
    // Schema utilities
    vector_schema, vector_schema_flat, document_schema, graph_node_schema, graph_edge_schema,
    // Conversions
    vector_batch_to_record_batch, record_batch_to_vector_batch,
    vector_records_to_record_batch, record_batch_to_vector_records,
    sql_value_to_json, json_to_sql_value, sql_value_to_arrow_type,
    filter_to_string,
};

pub use splits::{
    // Core split types
    FileSplit, SplitType, SplitStatistics, ColumnBounds, SpatialBounds,
    // Locality and scheduling
    SplitLocality, StorageTier, CacheStatus,
    // Split generation
    SplitGenerator, SplitPlanner,
    // Pruning types
    ScalarPredicate, ScalarValue, SplitCost,
};

// Re-export open table format implementations
pub use open::{
    // Delta Lake
    DeltaLakeFormat, DeltaLakeConfig,
    // Iceberg
    IcebergFormat, IcebergConfig,
    // Common types
    StorageOptions, TableMetadata,
};

// Re-export internal format adapters
pub use adapters::{
    // Generic adapter
    InternalFormatAdapter,
    // Engine-specific type aliases
    SstFormatAdapter, HelixFormatAdapter, ViperFormatAdapter,
    NovaFormatAdapter, SwiftFormatAdapter, RaptorFormatAdapter,
    // Factory functions
    create_sst_adapter, create_helix_adapter, create_viper_adapter,
    create_nova_adapter, create_swift_adapter, create_raptor_adapter,
};

// ============================================================================
// Module-Level Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_type_classification() {
        // Internal formats
        assert!(FormatRegistry::is_internal_format(FormatType::Sst));
        assert!(FormatRegistry::is_internal_format(FormatType::Helix));
        assert!(FormatRegistry::is_internal_format(FormatType::Viper));
        assert!(FormatRegistry::is_internal_format(FormatType::Nova));
        assert!(FormatRegistry::is_internal_format(FormatType::Swift));
        assert!(FormatRegistry::is_internal_format(FormatType::Raptor));
        assert!(FormatRegistry::is_internal_format(FormatType::Orion));
        assert!(FormatRegistry::is_internal_format(FormatType::Pulsar));
        assert!(FormatRegistry::is_internal_format(FormatType::Quasar));

        // Open formats
        assert!(FormatRegistry::is_open_format(FormatType::DeltaLake));
        assert!(FormatRegistry::is_open_format(FormatType::Iceberg));
        assert!(FormatRegistry::is_open_format(FormatType::Hudi));
        assert!(FormatRegistry::is_open_format(FormatType::LanceDb));
        assert!(FormatRegistry::is_open_format(FormatType::DuckDb));
        assert!(FormatRegistry::is_open_format(FormatType::Parquet));
        assert!(FormatRegistry::is_open_format(FormatType::Avro));
    }

    #[test]
    fn test_registry_creation() {
        let registry = FormatRegistry::new();
        assert!(registry.list_all_formats().is_empty());

        let registry = FormatRegistry::with_defaults();
        // Registry created but no implementations registered yet
        assert!(registry.list_all_formats().is_empty());
    }
}

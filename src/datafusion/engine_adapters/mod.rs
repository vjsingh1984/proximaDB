//! # Engine-Specific TableProvider Adapters
//!
//! This module provides engine-specific implementations of the `ProximaTableProvider` trait
//! for SST, HELIX, and VIPER storage engines.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                      ENGINE ADAPTERS MODULE                                  │
//! │  ┌───────────────────────────────────────────────────────────────────────┐  │
//! │  │                    ProximaTableProvider Trait                          │  │
//! │  │  (defined in proxima_table_provider.rs)                                │  │
//! │  └───────────────────────────────────────────────────────────────────────┘  │
//! │                                    ▲                                        │
//! │         ┌────────────────────────────────────────────────────┐              │
//! │         │                   Implementations                   │              │
//! │  ┌──────┴──────┐    ┌──────────────┐    ┌────────────────┐   │              │
//! │  │ SST Adapter │    │ HELIX Adapter │    │ VIPER Adapter │   │              │
//! │  │ Block-based │    │ Hilbert-based │    │ Parquet-based │   │              │
//! │  │ + Bloom     │    │ + Spatial     │    │ + Columnar    │   │              │
//! │  └─────────────┘    └──────────────┘    └────────────────┘   │              │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Design Principles
//!
//! Following SOLID principles:
//! - **S (Single Responsibility)**: Each adapter handles one engine type
//! - **O (Open/Closed)**: New engines can add adapters without modifying existing code
//! - **L (Liskov Substitution)**: All adapters are interchangeable via ProximaTableProvider
//! - **I (Interface Segregation)**: Adapters implement only required traits
//! - **D (Dependency Inversion)**: Adapters depend on abstract traits, not concrete engines
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::datafusion::engine_adapters::{
//!     SstTableProvider, HelixTableProvider, ViperTableProvider
//! };
//!
//! // Create SST table provider
//! let sst_provider = SstTableProvider::new(collection_info, base_path);
//!
//! // Use as DataFusion TableProvider
//! ctx.register_table("vectors", Arc::new(sst_provider))?;
//! ```

pub mod filesystem_parquet_reader;
pub mod helix_adapter;
pub mod object_store_parquet_reader;
pub mod sst_adapter;
pub mod viper_adapter;

// Re-export main types for convenience
pub use filesystem_parquet_reader::{
    FilesystemParquetSplitReader, FilesystemParquetTable, register_parquet_path,
};
pub use helix_adapter::{HelixSplitReader, HelixTableProvider};
pub use object_store_parquet_reader::{
    ObjectStoreParquetSplitReader, ObjectStoreParquetTable, register_object_store_parquet_location,
};
pub use sst_adapter::{SstSplitReader, SstTableProvider};
pub use viper_adapter::{ViperSplitReader, ViperTableProvider};

/// Common schema/utility helpers shared across the engine adapters.
pub mod common {
    use arrow_schema::{DataType, Field, Schema, SchemaRef};
    use std::sync::Arc;

    /// Standard vector collection schema used across all engines.
    ///
    /// This schema matches ProximaDB's VectorRecord structure:
    /// - id: UTF8 (primary key)
    /// - vector: FixedSizeList<Float32> (embedding)
    /// - metadata: Map<UTF8, UTF8> (JSON-encoded metadata)
    /// - timestamp: Int64 (creation time)
    /// - updated_at: Int64 (last update time, optional)
    /// - expires_at: Int64 (TTL expiry, optional)
    /// - version: Int64 (MVCC version, optional)
    pub fn vector_collection_schema(dimension: usize) -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    dimension as i32,
                ),
                false,
            ),
            Field::new(
                "metadata",
                DataType::Map(
                    Arc::new(Field::new(
                        "entries",
                        DataType::Struct(
                            vec![
                                Field::new("key", DataType::Utf8, false),
                                Field::new("value", DataType::Utf8, true),
                            ]
                            .into(),
                        ),
                        false,
                    )),
                    false, // keys_sorted
                ),
                true,
            ),
            Field::new("timestamp", DataType::Int64, true),
            Field::new("updated_at", DataType::Int64, true),
            Field::new("expires_at", DataType::Int64, true),
            Field::new("version", DataType::Int64, true),
        ]))
    }

    /// Simplified flat schema for basic vector operations.
    ///
    /// Used when full metadata mapping is not needed:
    /// - id: UTF8
    /// - vector: FixedSizeBinary (raw bytes)
    /// - metadata: UTF8 (JSON string)
    pub fn flat_vector_schema(dimension: usize) -> SchemaRef {
        // FixedSizeBinary size = dimension * 4 (f32 = 4 bytes)
        let vector_bytes = dimension * 4;
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeBinary(vector_bytes as i32),
                false,
            ),
            Field::new("metadata", DataType::Utf8, true),
        ]))
    }

    /// Calculate estimated storage size for a vector record.
    ///
    /// Returns bytes estimate including:
    /// - ID: average 32 bytes
    /// - Vector: dimension * 4 bytes (f32)
    /// - Metadata: average 256 bytes
    /// - Timestamps/version: 32 bytes
    pub fn estimate_record_size(dimension: usize) -> usize {
        32 + (dimension * 4) + 256 + 32
    }
}

#[cfg(test)]
mod tests {
    use super::common::*;

    #[test]
    fn test_vector_collection_schema() {
        let schema = vector_collection_schema(128);
        assert_eq!(schema.fields().len(), 7);
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(schema.field(1).name(), "vector");
        assert_eq!(schema.field(2).name(), "metadata");
    }

    #[test]
    fn test_flat_vector_schema() {
        let schema = flat_vector_schema(768);
        assert_eq!(schema.fields().len(), 3);
        // 768 * 4 = 3072 bytes
        assert!(matches!(
            schema.field(1).data_type(),
            arrow_schema::DataType::FixedSizeBinary(3072)
        ));
    }

    #[test]
    fn test_estimate_record_size() {
        let size = estimate_record_size(128);
        // 32 + 512 + 256 + 32 = 832
        assert_eq!(size, 832);
    }
}

//! Parquet Reading Implementations
//!
//! ## Unified Architecture (Consolidated with Columnar Module)
//! - UnifiedParquetReader: Now re-exported from columnar module to avoid duplication
//! - All VIPER-specific reader functionality has been merged into columnar::UnifiedParquetReader
//! - Automatic optimization based on query characteristics and storage type
//! - Support for all query patterns: direct reading, metadata filtering, quantized search

// DEPRECATED: unified_parquet_reader.rs - functionality moved to columnar module
// pub mod unified_parquet_reader; // TODO: Remove this file after migration complete

pub mod test_data_generator;

// Supporting modules
pub mod parquet_reconstructor;

#[cfg(test)]
pub mod tests;

// Re-export from columnar module to maintain API compatibility
pub use crate::storage::engines::columnar::{
    UnifiedParquetReader, ReadingStrategy, SchemaMapping, CollectionContext,
    // All VIPER types now consolidated in columnar module
    ReaderConfig, FilterValue, QuantizationMethod, SeekRange, VectorPosition,
    Stage2Strategy, SearchType, RowGroupAccessPattern,
    // Note: MetadataFilter is already in columnar module
};

// Supporting types
pub use parquet_reconstructor::{ParquetReconstructor, ReconstructedParquetData};
pub use test_data_generator::{ParquetTestDataGenerator, TestDataConfig, QuantizationType};
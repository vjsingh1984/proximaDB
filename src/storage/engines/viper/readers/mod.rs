//! Parquet Reading Implementations
//!
//! ## Unified Architecture
//! - UnifiedParquetReader: Single entry point with automatic strategy selection
//! - Automatic optimization based on query characteristics and storage type
//! - Unified caching, configuration, and error handling
//! - Support for all query patterns: direct reading, metadata filtering, quantized search

// Core unified architecture
pub mod unified_parquet_reader;
pub mod test_data_generator;

// Supporting modules
pub mod parquet_reconstructor;

#[cfg(test)]
pub mod tests;

// Public API - Direct search without adapters
pub use unified_parquet_reader::{
    UnifiedParquetReader, ReadingStrategy, ReaderConfig, MetadataFilter, FilterValue,
    QuantizationMethod, SeekRange, VectorPosition, Stage2Strategy,
    CollectionContext, SearchType, RowGroupAccessPattern,
};

// Supporting types
pub use parquet_reconstructor::{ParquetReconstructor, ReconstructedParquetData};
pub use test_data_generator::{ParquetTestDataGenerator, TestDataConfig, QuantizationType};
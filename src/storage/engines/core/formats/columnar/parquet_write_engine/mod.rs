//! Modular Parquet Writer Components
//!
//! This module provides efficient Parquet writing capabilities for columnar storage engines.
//! It's organized into focused submodules for better maintainability and clarity.

pub mod writer_config;
pub mod streaming_writer;
pub mod batch_writer;
pub mod writer_statistics;
pub mod implicit_id_generator;
pub mod schema_builder;
pub mod bloom_filter_builder;

// Re-export main types for convenience
pub use writer_config::ParquetWriterConfig;
pub use streaming_writer::{StreamingParquetWriter, StreamingWriterBuilder};
pub use batch_writer::{BatchParquetWriter, BatchWriterBuilder};
pub use writer_statistics::StreamingParquetWriterStats;
pub use implicit_id_generator::IdLessLookup;
// MetadataCollector is already in parent module, not here

// Common traits used across writer implementations
use anyhow::Result;
use crate::proto::proximadb_v1::VectorRecord;

/// Common trait for all Parquet writers
pub trait ParquetWriter: Send + Sync {
    /// Write a batch of records
    async fn write_batch(&mut self, records: &[VectorRecord]) -> Result<()>;

    /// Finalize the writer and return statistics
    async fn finalize(self) -> Result<StreamingParquetWriterStats>;

    /// Get current statistics without finalizing
    fn stats(&self) -> StreamingParquetWriterStats;
}

/// Builder pattern trait for creating writers
pub trait WriterBuilder {
    type Writer: ParquetWriter;

    /// Build the writer with configuration
    fn build(self) -> Result<Self::Writer>;

    /// Set the file path
    fn with_path(self, path: impl AsRef<std::path::Path>) -> Self;

    /// Set the vector dimension
    fn with_dimension(self, dimension: usize) -> Self;

    /// Set the configuration
    fn with_config(self, config: ParquetWriterConfig) -> Self;
}
//! Block Format Abstraction for SST Engine
//!
//! Provides an abstraction layer for different block storage formats,
//! enabling gradual migration from ProximaBlocks to Arrow-native storage.
//!
//! # Performance Benchmarks (1000 vectors, 128 dimensions)
//!
//! | Format        | Write (ms) | Size (KB) | Scan (ms) | Filter (ms) | PyArrow |
//! |---------------|------------|-----------|-----------|-------------|---------|
//! | ProximaBlocks | 331.8      | 573.9     | 21.4      | 16.6        | via Flight |
//! | ArrowBlock    | 240.8      | 609.5     | 8.8       | 8.3         | Direct  |
//!
//! **Key findings:**
//! - ArrowBlock: 27% faster writes, 59% faster scans
//! - ProximaBlocks: 6% smaller files, accessible via Arrow Flight
//!
//! # Available Block Formats
//!
//! The SST engine supports two interchangeable block formats:
//!
//! ## ProximaBlocks (Default)
//!
//! ProximaDB's native format optimized for vector database workloads.
//!
//! **Pros:**
//! - Integrated B+ tree index for O(log n) ID lookups
//! - Optimized for range reads and sequential I/O patterns
//! - Z-Order/Hilbert curve spatial sorting for better locality
//! - Block-level compression with LZ4/Zstd support
//! - Optimized for vector similarity search operations
//! - **Now accessible via Arrow Flight** (on-the-fly conversion)
//!
//! **Cons:**
//! - Slightly slower scans compared to ArrowBlock
//! - External tools require Arrow Flight connection
//!
//! **Best for:**
//! - Production vector search workloads
//! - High-throughput ingestion and querying
//! - When data stays within ProximaDB ecosystem
//!
//! ## ArrowBlock
//!
//! Standard Apache Arrow IPC format for ecosystem interoperability.
//!
//! **Pros:**
//! - Readable by PyArrow, DuckDB, Polars, Spark, etc.
//! - Columnar layout excellent for analytics
//! - Zero-copy sharing with Arrow-compatible tools
//! - Great for data export and external analysis
//! - **Fastest scan performance** (8.8ms vs 21.4ms)
//!
//! **Cons:**
//! - Sidecar `.idx` file needed for ID lookups (separate from pure Arrow)
//! - 6% larger file size than ProximaBlocks
//!
//! **Best for:**
//! - Data science workflows with Jupyter/pandas/polars
//! - Export for external analytics tools
//! - Cross-system data sharing
//! - Development and debugging (inspect with standard tools)
//!
//! # Configuration
//!
//! Set the block format in your configuration:
//!
//! ```toml
//! [storage.sst_config]
//! block_format = "ProximaBlocks"  # Default, optimal for vector workloads
//! # block_format = "ArrowBlock"   # For interoperability with Arrow ecosystem
//! ```
//!
//! # Interoperability
//!
//! Both formats can coexist in the same collection:
//! - Flush creates files in the configured format
//! - Compaction outputs files in the configured format
//! - Search reads both `.sst` and `.arrow` files transparently
//! - Migration can happen gradually by changing config and letting compaction convert files
//! - **ProximaBlocks are now accessible via Arrow Flight** for external tool access

use std::path::Path;

use anyhow::Result;
use tracing::{debug, info};

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::formats::arrow_block::{
    ArrowBlockConfig, ArrowBlockReader, ArrowBlockWriter,
};

/// Block format selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum BlockFormat {
    /// ProximaDB's native ProximaBlocks format
    #[default]
    ProximaBlocks,
    /// Arrow IPC based storage format
    ArrowBlock,
}

impl BlockFormat {
    /// Parse block format from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "arrowblock" | "arrow" | "arrow_block" => BlockFormat::ArrowBlock,
            _ => BlockFormat::ProximaBlocks,
        }
    }

    /// Get file extension for this format
    pub fn file_extension(&self) -> &str {
        match self {
            BlockFormat::ProximaBlocks => ".sst",
            BlockFormat::ArrowBlock => ".arrow",
        }
    }
}


/// Adapter for writing blocks in different formats
pub struct BlockFormatWriter {
    format: BlockFormat,
    dimension: u32,
}

impl BlockFormatWriter {
    /// Create new writer for the specified format
    pub fn new(format: BlockFormat, dimension: u32) -> Self {
        Self { format, dimension }
    }

    /// Write records to a file using the configured format
    pub fn write_records<P: AsRef<Path>>(&self, path: P, records: &[VectorRecord]) -> Result<()> {
        match self.format {
            BlockFormat::ProximaBlocks => {
                // ProximaBlocks uses existing SST writer path
                debug!("Using ProximaBlocks format for {}", path.as_ref().display());
                Ok(()) // Caller handles ProximaBlocks writing
            }
            BlockFormat::ArrowBlock => {
                let config = ArrowBlockConfig::new(self.dimension);
                let mut writer = ArrowBlockWriter::new(&path, config)?;
                writer.write_block(records)?;
                writer.finalize()?;
                info!(
                    "Wrote {} records to Arrow block: {}",
                    records.len(),
                    path.as_ref().display()
                );
                Ok(())
            }
        }
    }

    /// Check if this format is ArrowBlock
    pub fn is_arrow_block(&self) -> bool {
        matches!(self.format, BlockFormat::ArrowBlock)
    }
}

/// Adapter for reading blocks in different formats
pub struct BlockFormatReader {
    format: BlockFormat,
    #[allow(dead_code)]
    dimension: u32,
}

impl BlockFormatReader {
    /// Create new reader for the specified format
    pub fn new(format: BlockFormat, dimension: u32) -> Self {
        Self { format, dimension }
    }

    /// Detect format from file path
    pub fn detect_format<P: AsRef<Path>>(path: P) -> BlockFormat {
        let path = path.as_ref();
        if path.extension().map(|e| e == "arrow").unwrap_or(false) {
            BlockFormat::ArrowBlock
        } else {
            BlockFormat::ProximaBlocks
        }
    }

    /// Lookup a vector by ID
    pub fn lookup_by_id<P: AsRef<Path>>(
        &self,
        path: P,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        match self.format {
            BlockFormat::ProximaBlocks => {
                // ProximaBlocks uses existing SST reader path
                debug!(
                    "ProximaBlocks lookup for {} in {}",
                    vector_id,
                    path.as_ref().display()
                );
                Ok(None) // Caller handles ProximaBlocks reading
            }
            BlockFormat::ArrowBlock => {
                let reader = ArrowBlockReader::open(&path)?;
                let result = reader.lookup_by_id(vector_id)?;
                Ok(result)
            }
        }
    }

    /// Read all records from a file
    pub fn read_all<P: AsRef<Path>>(&self, path: P) -> Result<Vec<VectorRecord>> {
        match self.format {
            BlockFormat::ProximaBlocks => {
                debug!("ProximaBlocks read_all from {}", path.as_ref().display());
                Ok(vec![]) // Caller handles ProximaBlocks reading
            }
            BlockFormat::ArrowBlock => {
                let reader = ArrowBlockReader::open(&path)?;
                let records = reader.read_all()?;
                Ok(records)
            }
        }
    }

    /// Batch lookup multiple IDs
    pub fn batch_lookup<P: AsRef<Path>>(
        &self,
        path: P,
        ids: &[&str],
    ) -> Result<Vec<(String, VectorRecord)>> {
        match self.format {
            BlockFormat::ProximaBlocks => {
                debug!("ProximaBlocks batch_lookup in {}", path.as_ref().display());
                Ok(vec![]) // Caller handles ProximaBlocks reading
            }
            BlockFormat::ArrowBlock => {
                let reader = ArrowBlockReader::open(&path)?;
                let results = reader.lookup_batch(ids)?;
                Ok(results)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_parsing() {
        assert_eq!(
            BlockFormat::from_str("ProximaBlocks"),
            BlockFormat::ProximaBlocks
        );
        assert_eq!(BlockFormat::from_str("ArrowBlock"), BlockFormat::ArrowBlock);
        assert_eq!(BlockFormat::from_str("arrow"), BlockFormat::ArrowBlock);
        assert_eq!(BlockFormat::from_str("unknown"), BlockFormat::ProximaBlocks);
    }

    #[test]
    fn test_file_extension() {
        assert_eq!(BlockFormat::ProximaBlocks.file_extension(), ".sst");
        assert_eq!(BlockFormat::ArrowBlock.file_extension(), ".arrow");
    }

    #[test]
    fn test_format_detection() {
        assert_eq!(
            BlockFormatReader::detect_format("/path/to/file.sst"),
            BlockFormat::ProximaBlocks
        );
        assert_eq!(
            BlockFormatReader::detect_format("/path/to/file.arrow"),
            BlockFormat::ArrowBlock
        );
    }
}

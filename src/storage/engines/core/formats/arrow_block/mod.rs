//! # Arrow Block Storage Format
//!
//! Arrow-native block format using Arrow IPC for on-disk storage.
//! This provides zero-copy reads via memory mapping and standard
//! interoperability with Arrow-based systems.
//!
//! ## Design Goals
//!
//! 1. **Zero-Copy Reads**: Memory-mapped Arrow IPC files for direct access
//! 2. **Columnar Layout**: Efficient for analytics and batch operations
//! 3. **Standard Format**: Interop with PyArrow, DuckDB, Polars, etc.
//! 4. **Gradual Migration**: Works alongside ProximaBlocks via feature flag
//!
//! ## File Layout
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │                    Arrow IPC Stream                       │
//! ├──────────────────────────────────────────────────────────┤
//! │  Schema Message (with ProximaDB metadata)                │
//! ├──────────────────────────────────────────────────────────┤
//! │  RecordBatch 0 (block 0 data)                            │
//! │    - id: StringArray                                     │
//! │    - vector: FixedSizeListArray<Float32>                │
//! │    - metadata: Utf8 (JSON) or Struct                    │
//! │    - timestamp: Int64                                    │
//! │    - version: Int64                                      │
//! ├──────────────────────────────────────────────────────────┤
//! │  RecordBatch 1 (block 1 data)                            │
//! ├──────────────────────────────────────────────────────────┤
//! │  ...                                                      │
//! ├──────────────────────────────────────────────────────────┤
//! │  Block Index (offsets for O(log n) lookup)               │
//! │    - B+ tree entries: [(min_id, max_id, offset, size)]   │
//! ├──────────────────────────────────────────────────────────┤
//! │  Footer                                                   │
//! │    - magic: "PRXARROW"                                   │
//! │    - version: u32                                        │
//! │    - num_blocks: u32                                     │
//! │    - index_offset: u64                                   │
//! │    - total_records: u64                                  │
//! │    - checksum: u64                                       │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::engines::core::formats::arrow_block::{
//!     ArrowBlockWriter, ArrowBlockReader, ArrowBlockConfig
//! };
//!
//! // Writing
//! let config = ArrowBlockConfig::new(dimension);
//! let mut writer = ArrowBlockWriter::new(path, config)?;
//! writer.write_block(&records)?;
//! writer.finalize()?;
//!
//! // Reading
//! let reader = ArrowBlockReader::open(path)?;
//! let records = reader.read_block(0)?;
//! let record = reader.lookup_by_id("vec_123")?;
//! ```

pub mod config;
pub mod index;
pub mod reader;
pub mod writer;

// Re-exports
pub use config::{ArrowBlockConfig, ArrowBlockMetadata};
pub use index::{ArrowBlockIndex, ArrowIndexEntry};
pub use reader::ArrowBlockReader;
pub use writer::ArrowBlockWriter;

use std::io;
use thiserror::Error;

/// Magic bytes for Arrow block files
pub const ARROW_BLOCK_MAGIC: &[u8; 8] = b"PRXARROW";

/// Current format version
pub const ARROW_BLOCK_VERSION: u32 = 1;

/// Footer size in bytes (fixed)
pub const ARROW_BLOCK_FOOTER_SIZE: usize = 48;

/// Errors specific to Arrow block operations
#[derive(Error, Debug)]
pub enum ArrowBlockError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("Invalid magic bytes")]
    InvalidMagic,

    #[error("Unsupported version: {0}")]
    UnsupportedVersion(u32),

    #[error("Block not found: {0}")]
    BlockNotFound(usize),

    #[error("Vector not found: {0}")]
    VectorNotFound(String),

    #[error("Checksum mismatch")]
    ChecksumMismatch,

    #[error("Schema mismatch: {0}")]
    SchemaMismatch(String),

    #[error("Conversion error: {0}")]
    ConversionError(String),
}

impl From<anyhow::Error> for ArrowBlockError {
    fn from(e: anyhow::Error) -> Self {
        ArrowBlockError::ConversionError(e.to_string())
    }
}

/// Result type for Arrow block operations
pub type ArrowBlockResult<T> = Result<T, ArrowBlockError>;

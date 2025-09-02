//! Core infrastructure for storage engines
//! 
//! This module contains all shared infrastructure used by storage engine implementations.
//! It provides a clean separation between infrastructure (how things work) and
//! implementations (what engines do).

pub mod io;      // I/O operations: zero-copy, filesystem abstractions
pub mod formats; // Storage formats: row-based, columnar
pub mod search;  // Search infrastructure: progressive search, filtering
pub mod ops;     // Common operations: compression, encoding, optimization

// Re-export commonly used types for convenience
pub use io::zero_copy::ZeroCopyIOSystem;
pub use formats::{
    fastlanes_blocks::{FastLanesDataBlock, FastLanesBlockMetadata, RowBasedUtilities},
    columnar::{ColumnarSchema, ParquetQueryEngine, ParquetIOLayer},
};
pub use search::{ProgressiveSearchEngine, SearchContext};
// Common operations exports available from ops module directly
//! Core infrastructure for storage engines
//!
//! This module contains all shared infrastructure used by storage engine implementations.
//! It provides a clean separation between infrastructure (how things work) and
//! implementations (what engines do).

pub mod formats; // Storage formats: row-based, columnar
pub mod io; // I/O operations: zero-copy, filesystem abstractions
pub mod ops;
pub mod search; // Search infrastructure: progressive search, filtering // Common operations: compression, encoding, optimization

// Re-export commonly used types for convenience
pub use formats::{
    columnar::{ColumnarSchema, ParquetIOLayer, ParquetQueryEngine},
    fastlanes_blocks::{FastLanesBlockMetadata, FastLanesDataBlock, RowBasedUtilities},
};
pub use io::zero_copy::ZeroCopyIOSystem;
pub use search::{ProgressiveSearchEngine, SearchContext};
// Common operations exports available from ops module directly

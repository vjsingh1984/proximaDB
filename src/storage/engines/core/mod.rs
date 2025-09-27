//! Core infrastructure for storage engines
//!
//! This module contains all shared infrastructure used by storage engine implementations.
//! It provides a clean separation between infrastructure (how things work) and
//! implementations (what engines do).

pub mod constants; // Centralized constants for all storage engines
pub mod filter_evaluator;
pub mod formats; // Storage formats: row-based, columnar
pub mod io; // I/O operations: zero-copy, filesystem abstractions
pub mod ops; // Common operations: compression, encoding, optimization
pub mod read_strategy; // Unified read access strategy for all engines
pub mod adaptive_strategy_optimizer; // Intelligent strategy optimization and tuning
pub mod search; // Search infrastructure: progressive search, filtering // Unified filter evaluation for all engines

// Re-export commonly used types for convenience
pub use filter_evaluator::{
    UnifiedFilterEvaluator, create_filter_fn, create_json_filter_fn, evaluate_filter,
    evaluate_filter_strings,
};
pub use formats::{
    columnar::{ColumnarSchema, ParquetIOLayer, ParquetQueryEngine},
    proximablocks::{ProximaBlockMetadata, ProximaDataBlock, RowBasedUtilities},
};
pub use io::zero_copy::ZeroCopyIOSystem;
pub use search::{ProgressiveSearchEngine, SearchContext};
// Common operations exports available from ops module directly

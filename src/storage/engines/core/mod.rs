//! Core infrastructure for storage engines
//!
//! This module contains all shared infrastructure used by storage engine implementations.
//! It provides a clean separation between infrastructure (how things work) and
//! implementations (what engines do).

pub mod adaptive_strategy_optimizer; // Intelligent strategy optimization and tuning
pub mod constants; // Centralized constants for all storage engines
pub mod filter_evaluator; // Unified filter evaluation for all engines
pub mod formats; // Storage formats: row-based, columnar
pub mod io; // I/O operations: zero-copy, filesystem abstractions
pub mod metadata_serializer; // Shared metadata serializer helpers (DRY across engines)
pub mod raptor_unified_metadata_serializer; // RAPTOR metadata serializer moved to shared core module
pub mod helix_unified_metadata_serializer; // HELIX metadata serializer moved to shared core module
pub mod viper_unified_metadata_serializer; // VIPER metadata serializer moved to shared core module
pub mod swift_unified_metadata_serializer; // SWIFT metadata serializer moved to shared core module
pub mod nova_unified_metadata_serializer; // NOVA metadata serializer moved to shared core module
pub mod sst_unified_metadata_serializer; // SST metadata serializer moved to shared core module
pub mod ops; // Common operations: compression, encoding, optimization
pub mod pca; // PCA model management for spatial clustering
pub mod progressive; // ISP-compliant progressive search stages
pub mod read_strategy; // Unified read access strategy for all engines
pub mod search; // Search infrastructure: progressive search, filtering

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
pub use pca::{EnhancedPCAModel, PCAConfig, PCAManagerConfig, PCAModelManager};
pub use progressive::{
    BinaryStage, CoordinatorConfig, Fp32Stage, Int8Stage, PqStage, ProgressiveSearchCoordinator,
    ProgressiveSearchStage, QuantizationLevel as ProgressiveQuantizationLevel, ScoredCandidate,
    StageStats,
};
pub use search::{ProgressiveSearchEngine, SearchContext};
// Common operations exports available from ops module directly

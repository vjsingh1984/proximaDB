//! Storage Engines Module
//!
//! This module contains all storage engine implementations following the Strategy Pattern.
//! VIPER is the default strategy, with SST as an alternative for comparison.
//! SWIFT and NOVA are dual-mode engines for zero-overhead vector storage.
//! The Universal Adapter provides unified distance computation across all engines.

pub mod sst;
pub mod viper;   // Columnar storage engine with Parquet format
pub mod swift;   // Storage With Instant Fast Traversal - dual-mode SST with ID-based lookups
pub mod nova;    // Next-gen Optimized Vector Analytics - dual-mode VIPER with columnar quantization
pub mod prism;   // Progressive Retrieval through Indexed Storage Management - memory-optimized hierarchical engine
pub mod raptor;  // Row-Aligned Predicated Tensor Optimized Repository - cloud-optimized with embedded HNSW
pub mod columnar; // Shared columnar storage infrastructure for NOVA and VIPER
pub mod row_based; // Shared row-based storage infrastructure for SST and SWIFT
pub mod common; // Universal engine infrastructure shared across all engines
pub mod factory; // Engine factory for creating instances
pub mod migration; // Engine migration utilities
pub mod universal; // Universal distance adapter system with PQ and INT8 optimizations
// pub mod hybrid; // Future implementation

// Re-export main engine types
pub use sst::SstStorage;
pub use viper::ViperEngine;
pub use swift::SwiftEngine;
pub use nova::NovaEngine;
pub use prism::PrismEngine;
pub use raptor::RaptorEngine;

// Re-export factory and utilities
pub use factory::{
    StorageEngineFactory, WorkloadType, EngineRequirements, EngineComparison
};

// InsertResult structure for vector operations
#[derive(Debug, Clone)]
pub struct InsertResult {
    /// Number of entries written
    pub entries_written: i64,
    /// Duration of the operation in microseconds
    pub duration_micros: i64,
    /// Bytes written to storage
    pub bytes_written: i64,
}

// Re-export universal adapter system
pub use universal::{
    UniversalDistanceAdapter, DistanceComputationRequest, DistanceComputationResult,
    CandidateVector, UniversalAdapterConfig, ProgressiveRefinementConfig, StorageFormat,
    EngineType, StorageEngineAdapter, QuantizedFormat, CompressionFormat,
};

// Strategy pattern exports
pub use crate::storage::traits::{
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics, FlushParameters,
    FlushResult, StorageEngineStrategy, UnifiedStorageEngine,
};

// Stub types for missing definitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageEngineCompatibility {
    ViperOnly = 1,
    AllEngines = 2,
    LsmAndViper = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizedFormat {
    Standard = 1,
    Compressed = 2,
    Quantized = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelType {
    Hot = 1,
    Warm = 2,
    Cold = 3,
}


//! Storage Engines Module
//!
//! This module provides a clean architecture for storage engines:
//! - `core/`: Shared infrastructure (I/O, formats, search, operations)
//! - `impls/`: Actual engine implementations (SST, VIPER, SWIFT, NOVA, PRISM, RAPTOR)
//! - `traits.rs`: Common traits all engines implement
//!
//! This separation ensures:
//! - No code duplication between engines
//! - Clear boundaries between infrastructure and implementation
//! - Easy addition of new engines
//! - Consistent behavior across engines

pub mod core;
pub mod impls;

// Keep these at the top level for now (will be moved/refactored later)
pub mod factory;
pub mod migration;
pub mod universal;
pub mod progressive_search_trait;
pub mod event_log_integration;

// Re-export traits
pub use crate::storage::traits::{
    UnifiedStorageEngine, StorageEngineStrategy,
    FlushParameters, FlushResult,
    CompactionParameters, CompactionResult,
};

// Re-export main engine types
pub use impls::{
    sst::SstStorage,
    viper::ViperEngine,
    swift::SwiftEngine,
    nova::NovaEngine,
    prism::PrismEngine,
    raptor::RaptorEngine,
};

// Re-export factory
pub use factory::{
    StorageEngineFactory, WorkloadType, 
    EngineRequirements, EngineComparison,
};

// Re-export universal adapter
pub use universal::{
    UniversalDistanceAdapter, DistanceComputationRequest,
    DistanceComputationResult, CandidateVector,
};

// InsertResult structure
#[derive(Debug, Clone)]
pub struct InsertResult {
    pub entries_written: i64,
    pub duration_micros: i64,
    pub bytes_written: i64,
}
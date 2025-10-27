//! # Storage Engines Module - Multi-Engine Storage Architecture
//!
//! This module provides ProximaDB's sophisticated multi-engine storage system, allowing
//! optimal storage strategies for different workloads. Each engine is specialized for
//! specific access patterns while sharing common infrastructure.
//!
//! ## Role in ProximaDB Architecture
//!
//! The storage engines layer sits at the core of data persistence:
//! ```text
//! Service Layer → Storage Trait → Engine Selection
//!                                       ↓
//!                    ┌──────────────────┴───────────────────┐
//!                    │         Storage Engines               │
//!                    ├────────────────────────────────────────┤
//!                    │ SST │ VIPER │ NOVA │ SWIFT │ RAPTOR │ HELIX │
//!                    └────────────────────────────────────────┘
//!                                       ↓
//!                         Core Infrastructure (Shared)
//!                    ┌────────────────────────────────────────┐
//!                    │ I/O │ Formats │ Search │ Compression  │
//!                    └────────────────────────────────────────┘
//! ```
//!
//! ## Module Organization
//!
//! - **`constants`**: Engine type constants and magic bytes
//! - **`core/`**: Shared infrastructure used by all engines
//!   - `formats/`: Row-based and columnar format implementations
//!   - `io/`: Zero-copy I/O system with prefetching
//!   - `search/`: Progressive search and common search logic
//!   - `ops/`: Compression, encoding, and performance optimizations
//!
//! - **`impls/`**: Engine implementations with unique characteristics
//!   - `sst/`: Hybrid columnar SSTable (ProximaBlocks) for OLTP workloads
//!   - `viper/`: Columnar Parquet for analytics
//!   - `nova/`: Hybrid quantized columnar engine
//!   - `swift/`: High-speed hierarchical blocks
//!   - `raptor/`: Matrix-optimized with adaptive PXK
//!
//! ## Engine Selection Guide
//!
//! | Engine | Best For | Storage Format | Key Features |
//! |--------|----------|----------------|--------------|
//! | **SST** | Real-time queries, frequent updates | Hybrid columnar (ProximaBlocks) | Three-stage filtering, bloom filters |
//! | **VIPER** | Analytics, batch operations | Columnar Parquet | Advanced quantization, high compression |
//! | **NOVA** | Mixed workloads | Hybrid columnar | Quantized columns, progressive search |
//! | **SWIFT** | High-throughput | Hierarchical blocks | Superblock caching, ID indexing |
//! | **RAPTOR** | Matrix operations | Matrix-optimized | Adaptive PXK, boundary detection |
//!
//! ## Performance Characteristics
//!
//! - **Write Performance**: 100K-500K vectors/sec (engine dependent)
//! - **Query Latency**: < 10ms for 1M vectors (with indexing)
//! - **Compression Ratios**: 2x-10x depending on engine and data
//! - **Memory Efficiency**: Configurable with quantization support
//!
//! ## Automatic Engine Selection
//!
//! The `StorageEngineFactory` automatically selects the optimal engine based on:
//! - Workload type (OLTP, OLAP, mixed)
//! - Data characteristics (dimensionality, update frequency)
//! - Resource constraints (memory, storage)
//! - Query patterns (point lookup, range scan, analytics)
//!
//! ## Shared Infrastructure
//!
//! All engines benefit from common infrastructure:
//! - **Zero-Copy I/O**: Memory-mapped files with prefetching
//! - **Progressive Search**: Multi-tier deduplication
//! - **Compression**: 13 algorithms with context-aware selection
//! - **Hardware Acceleration**: SIMD/GPU automatic detection
//!
//! ## Migration and Compatibility
//!
//! Engines can be migrated with zero downtime:
//! 1. Background data migration to new engine
//! 2. Gradual traffic shifting
//! 3. Atomic switchover
//! 4. Old engine cleanup
//!
//! ## Usage Example
//!
//! ```rust,ignore
//! use proximadb::storage::engines::{StorageEngineFactory, WorkloadType};
//!
//! // Automatic engine selection
//! let engine = StorageEngineFactory::create_optimal_engine(
//!     WorkloadType::OLTP,
//!     &collection_config
//! )?;
//!
//! // Direct engine selection
//! let viper = StorageEngineFactory::create_engine(
//!     "viper",
//!     &collection_config
//! )?;
//! ```

pub mod constants;
pub mod core;
pub mod impls;

// Keep these at the top level for now (will be moved/refactored later)
pub mod event_log_integration;
pub mod factory;
pub mod migration;
pub mod progressive_search_trait;
pub mod universal;

// Re-export traits
pub use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageEngineStrategy,
    UnifiedStorageEngine,
};

// Re-export main engine types
pub use impls::{
    nova::NovaEngine, raptor::RaptorEngine, sst::SstEngine, swift::SwiftEngine, viper::ViperEngine,
};

// Re-export constants
pub use constants::*;

// Re-export factory
pub use factory::{EngineComparison, EngineRequirements, StorageEngineFactory, WorkloadType};

// Re-export universal adapter
pub use universal::{
    CandidateVector, DistanceComputationRequest, DistanceComputationResult,
    UniversalDistanceAdapter,
};

// InsertResult structure
#[derive(Debug, Clone)]
pub struct InsertResult {
    pub entries_written: i64,
    pub duration_micros: i64,
    pub bytes_written: i64,
}

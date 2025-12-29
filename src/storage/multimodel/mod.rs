//! # Multi-Model Storage Module
//!
//! This module provides a unified storage facade that orchestrates multiple specialized
//! storage engines for different data models: Vector, Document, Graph, RDBMS, and Observability.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │               MultiModelStorageFacade                            │
//! │  Unified entry point for all multi-model storage operations     │
//! └─────────────────────────────────────────────────────────────────┘
//!                               │
//!         ┌─────────────────────┼─────────────────────┐
//!         ▼                     ▼                     ▼
//! ┌───────────────┐    ┌───────────────┐    ┌───────────────┐
//! │  VectorStore  │    │  GraphStore   │    │  RDBMSStore   │
//! │  (Helix+SST)  │    │   (Orion)     │    │  (SST+Viper)  │
//! └───────────────┘    └───────────────┘    └───────────────┘
//!         │                     │                     │
//! ┌───────────────────────────────────────────────────────────────┐
//! │              SHARED INFRASTRUCTURE                             │
//! │  UnifiedCacheOrchestrator | Quantization | WAL | Catalog      │
//! └───────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Design Philosophy
//!
//! **Dedicated Storage + Unified Query**: Each data model has its own optimized storage
//! engine, but all are accessed through a single unified interface.
//!
//! ## Storage Engine Mapping
//!
//! | Model Type | Primary Engine | Secondary Engine | Rationale |
//! |------------|----------------|------------------|-----------|
//! | Vector | HELIX | SST | Hilbert curve locality + real-time |
//! | Document | RAPTOR | SST | Adaptive row-groups + hot tier |
//! | Graph | ORION | - | Native CSR format |
//! | RDBMS | SST (OLTP) | VIPER (OLAP) | HTAP separation |
//! | Observability | VIPER | Tantivy | Columnar + log indexing |

pub mod facade;
pub mod htap;
pub mod observability;
pub mod stores;
pub mod traits;
pub mod transaction;

// Re-exports
pub use facade::MultiModelStorageFacade;
pub use stores::{
    DocumentStore,
    GraphStore,
    ObservabilityStore,
    RDBMSStore,
    VectorStore,
};
pub use traits::{
    MultiModelStorageEngine,
    ModelType,
    StoreCapabilities,
};
pub use htap::{
    ReplicationCoordinator, ReplicationConfig, ReplicationStats,
    WorkloadRouter, QueryCharacteristics, WorkloadType, RoutingDecision,
};
pub use observability::{
    TimePartitioner, PartitionConfig, Partition, PartitionRange, PartitionGranularity,
    RollupManager, RollupConfig, RollupInterval, RollupView, AggregationFunction,
    CardinalityLimiter, CardinalityConfig, LabelStats, CheckResult, LimitAction,
};
pub use transaction::{
    TransactionCoordinator, Transaction, TransactionConfig, TransactionStats,
    TwoPhaseCommitProtocol, PrepareResult, CommitResult, TransactionState,
    IsolationLevel, IsolationManager, ReadSnapshot, WriteSet,
};

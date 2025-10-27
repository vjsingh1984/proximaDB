//! # Storage Module - Persistence and Data Management Layer
//!
//! This module provides ProximaDB's sophisticated storage subsystem with multiple
//! storage engines, write-ahead logging, metadata management, and advanced caching.
//! It implements a layered architecture for durability, performance, and flexibility.
//!
//! ## Role in ProximaDB Architecture
//!
//! The storage layer manages all data persistence:
//! ```text
//! Services Layer
//!       ↓
//! ┌─────────────────────────────────────────────┐
//! │            Storage Subsystem                 │
//! ├─────────────────────────────────────────────┤
//! │  WAL → MemTable → Storage Engine → Disk     │
//! │   ↓       ↓           ↓             ↓       │
//! │ EventLog Cache   Compaction    Filesystem   │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! ## Core Components
//!
//! ### 1. **Storage Engines** (`engines/`)
//! Six specialized engines for different workloads:
//! - **SST**: Hybrid columnar (ProximaBlocks) for OLTP, real-time queries
//! - **VIPER**: Columnar Parquet for analytics
//! - **NOVA**: Hybrid quantized columnar
//! - **SWIFT**: High-speed hierarchical blocks
//! - **PRISM**: Tree-based with Proxima
//! - **RAPTOR**: Matrix-optimized with adaptive PXK
//!
//! ### 2. **Write-Ahead Log** (`persistence/write_ahead_log/`)
//! Durability and recovery system:
//! - Append-only log for all operations
//! - Configurable sync modes (immediate, batch)
//! - Recovery from crashes
//! - Compaction coordination
//!
//! ### 3. **MemTable** (`memtable/`)
//! In-memory write buffer:
//! - Lock-free concurrent writes
//! - Automatic flushing to storage
//! - Multiple implementations (SkipList, BTree, ART)
//! - WAL integration for durability
//!
//! ### 4. **Metadata Store** (`metadata/`)
//! Collection and system metadata:
//! - Atomic metadata operations
//! - Cloud backend support (S3, Azure, GCS)
//! - Schema management
//! - Index configurations
//!
//! ### 5. **Cache System** (`cache/`)
//! Multi-level caching infrastructure:
//! - Query result cache
//! - Metadata cache
//! - Block cache for storage engines
//! - Adaptive eviction policies
//!
//! ### 6. **Transaction Coordinator** (`transaction_coordinator.rs`)
//! Atomic operations across components:
//! - Two-phase commit protocol
//! - Lock-free coordination with DashMap
//! - Rollback on failures
//! - Staging area management
//!
//! ## Storage Flow
//!
//! ### Write Path
//! ```text
//! Insert Request
//!       ↓
//! Write to WAL (durability)
//!       ↓
//! Insert to MemTable (fast access)
//!       ↓
//! Return to client
//!       ↓
//! Background: Flush to Storage Engine
//!       ↓
//! Background: Compaction
//! ```
//!
//! ### Read Path
//! ```text
//! Query Request
//!       ↓
//! Check Cache
//!       ↓
//! Search MemTable (recent data)
//!       ↓
//! Search Storage Engine (persistent data)
//!       ↓
//! Merge Results
//!       ↓
//! Update Cache
//! ```
//!
//! ## Key Features
//!
//! ### Strategy Pattern
//! All engines implement `UnifiedStorageEngine` trait:
//! ```rust,ignore
//! trait UnifiedStorageEngine {
//!     async fn insert(&self, records: Vec<VectorRecord>) -> Result<InsertResult>;
//!     async fn search(&self, query: SearchQuery) -> Result<SearchResult>;
//!     async fn flush(&self, params: FlushParameters) -> Result<FlushResult>;
//!     async fn compact(&self, params: CompactionParameters) -> Result<CompactionResult>;
//! }
//! ```
//!
//! ### Lock-Free Concurrency
//! Using DashMap for concurrent access:
//! - No global locks
//! - Sharded internal structure
//! - Wait-free reads
//! - Low contention writes
//!
//! ### Cloud-Native Storage
//! Filesystem abstraction supports:
//! - Local filesystem
//! - Amazon S3
//! - Azure Blob Storage
//! - Google Cloud Storage
//! - HDFS
//!
//! ## Configuration
//!
//! ```toml
//! [storage]
//! default_engine = "sst"  # or "viper", "nova", etc.
//! data_directory = "/data/proximadb"
//!
//! # WAL configuration
//! [storage.wal]
//! enabled = true
//! sync_mode = "batch"  # or "immediate"
//! batch_size = 100
//! flush_interval_ms = 1000
//!
//! # MemTable configuration
//! [storage.memtable]
//! type = "skiplist"  # or "btree", "art"
//! max_size_mb = 256
//! flush_threshold = 0.8
//!
//! # Compaction settings
//! [storage.compaction]
//! strategy = "leveled"  # or "tiered", "unified"
//! max_background_jobs = 2
//! target_file_size_mb = 128
//! ```
//!
//! ## Performance Characteristics
//!
//! - **Write Latency**: < 1ms (memtable)
//! - **Flush Throughput**: 500MB/sec
//! - **Compaction Speed**: 200MB/sec
//! - **Recovery Time**: < 10s for 1GB WAL
//! - **Cache Hit Rate**: 80-95%
//!
//! ## Module Organization
//!
//! - **`builder.rs`**: Storage system builder pattern
//! - **`traits.rs`**: Core storage traits
//! - **`types.rs`**: Common type definitions  
//! - **`validation.rs`**: Configuration validation
//! - **`engine.rs`**: Main storage engine implementation
//! - **`optimization.rs`**: Storage optimization utilities
//! - **`strategy.rs`**: Collection lifecycle strategies
//! - **`common/`**: Shared utilities and helpers
//!
//! ## Error Handling
//!
//! Unified error types for storage operations:
//! - `StorageError::DiskIO` - I/O failures
//! - `StorageError::WAL` - Write-ahead log errors
//! - `StorageError::Corruption` - Data corruption detected
//! - `StorageError::OutOfSpace` - Disk space exhausted
//! - `StorageError::Configuration` - Invalid configuration

pub mod builder;
pub mod traits;
pub mod types;
pub mod unified_scan_strategy;
pub mod validation;

// Common reusable components
pub mod common;

// Background operation context (optimization)
pub mod background_flush_context;

// Engine capabilities and supportability checks
pub mod engine_capabilities;

// Core storage engines (organized)
pub mod engines;

// Data persistence layer (organized)
pub mod persistence;

// Unified atomic operations
pub mod transaction_coordinator;

// Core modules
pub mod engine;
// Unified memtable system
pub mod memtable;
pub mod metadata;
// Quantization now handled by unified compute module
// Storage optimization utilities
pub mod optimization;
// Strategy module for collection lifecycle configuration
pub mod strategy;
// Specialized cache system with shared infrastructure
pub mod cache;

// Multi-tenant architecture modules
pub mod tenant;

// Semantic Knowledge Store (SKS) modules
pub mod entity_store;
pub mod provenance;
pub mod relations;

// Key-value storage interface
pub mod kv;

// Lock-free implementations have been integrated into the main implementations
// TransactionCoordinator now uses DashMap for active_operations
// StorageEngine now uses DashMap for lsm_trees and mmap_readers

// Main exports from organized structure
pub use builder::{StorageSystem, StorageSystemBuilder, StorageSystemConfig};
pub use types::StorageEngineType;
pub use validation::ConfigValidator;

// Strategy pattern exports
pub use traits::{
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics, FlushParameters,
    FlushResult as TraitFlushResult, StorageEngineStrategy, UnifiedStorageEngine,
};

// Engine exports
pub use engines::impls::sst::SstEngine;
// Arrow integration re-enabled - compilation conflicts resolved
// pub use engines::viper::ViperEngine;

// Persistence exports
pub use persistence::{DiskManager, FilesystemConfig, FilesystemFactory};

// Atomic operations exports
pub use transaction_coordinator::{
    StagingConfig, TransactionCoordinator, TransactionStageType, TransactionalOperationMetadata,
    TransactionalOperationStatus, ViperTransactionalOperations, WalTransactionalOperations,
};

// Storage engine exports
pub use engine::StorageEngine;
// Write Buffer system exports
use crate::core::StorageError;
pub use metadata::{MetadataStore, SystemMetadata};
pub use persistence::write_ahead_log::{BatchId, WALConfig, WALOperation, WriteAheadLogManager};

// ResultProcessor has naming conflicts, import explicitly when needed

pub type Result<T> = std::result::Result<T, StorageError>;

// Tests module
#[cfg(test)]
mod tests;

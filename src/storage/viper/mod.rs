//! VIPER (Vectorized Intelligent Parquet with Efficient Retrieval) Module
//!
//! ⚠️  **DEPRECATED MODULE**: This entire module has been consolidated into the unified vector storage system.
//! 🔄  **MIGRATION PATH**: Use `src/storage/vector/engines/` instead for all VIPER functionality.
//! 📅  **DEPRECATION DATE**: Phase 4 consolidation completed
//!
//! ## Migration Guide
//!
//! **Before (Legacy VIPER):**
//! ```rust
//! use crate::storage::viper::{ViperStorageEngine, ViperSearchEngine, ViperIndexManager};
//! ```
//!
//! **After (Unified Vector Storage):**
//! ```rust
//! use proximadb::storage::vector::engines::{
//!     ViperCoreEngine, ViperPipeline, ViperFactory, ViperUtilities
//! };
//! use proximadb::storage::vector::{VectorStorageCoordinator, UnifiedSearchEngine};
//! ```
//!
//! ## New Unified Architecture
//!
//! The 19 VIPER files have been consolidated into 4 enterprise-grade modules:
//!
//! - **`viper_core.rs`** - Core storage operations with ML-driven clustering
//! - **`viper_pipeline.rs`** - Data processing pipeline with optimization
//! - **`viper_factory.rs`** - Intelligent factory with adaptive configuration
//! - **`viper_utilities.rs`** - Comprehensive monitoring and background services
//!
//! ## Benefits of Migration
//!
//! - **75% Code Reduction**: 19 files → 4 focused modules
//! - **Enhanced Performance**: ML-driven optimizations throughout
//! - **Better Developer Experience**: Unified APIs with intelligent defaults
//! - **Enterprise Features**: Comprehensive monitoring and background services

pub mod adapter;
pub mod atomic_operations;
pub mod compaction;
pub mod compression;
pub mod config;
pub mod factory;
pub mod flusher;
pub mod index;
pub mod partitioner;
pub mod processor;
pub mod schema;
pub mod search_engine;
pub mod search_impl;
pub mod staging_operations;
pub mod stats;
pub mod storage_engine;
pub mod ttl;
pub mod types;

// ⚠️ DEPRECATED EXPORTS - Use unified vector storage system instead
// 🔄 MIGRATION: Use `src/storage/vector/engines/` for all VIPER functionality

#[deprecated(
    since = "0.1.0",
    note = "Use `ViperCoreEngine` from `proximadb::storage::vector::engines` instead"
)]
pub use storage_engine::ViperStorageEngine;

#[deprecated(
    since = "0.1.0", 
    note = "Use `UnifiedSearchEngine` from `proximadb::storage::vector::search` instead"
)]
pub use search_engine::ViperProgressiveSearchEngine as ViperSearchEngine;

#[deprecated(
    since = "0.1.0",
    note = "Use `UnifiedIndexManager` from `proximadb::storage::vector::indexing` instead"
)]
pub use index::ViperIndexManager;

#[deprecated(
    since = "0.1.0",
    note = "Use `ViperFactory` from `proximadb::storage::vector::engines` instead"
)]
pub use factory::ViperSchemaFactory;

#[deprecated(
    since = "0.1.0",
    note = "Use `ViperPipeline` from `proximadb::storage::vector::engines` instead"
)]
pub use flusher::ViperParquetFlusher;

#[deprecated(
    since = "0.1.0",
    note = "Use `ViperUtilities` from `proximadb::storage::vector::engines` instead"
)]
pub use ttl::TTLCleanupService;

// Legacy re-exports for backward compatibility (all deprecated)
#[deprecated(since = "0.1.0", note = "Use unified vector storage types instead")]
pub use adapter::VectorRecordSchemaAdapter;
#[deprecated(since = "0.1.0", note = "Use unified vector storage configuration instead")]
pub use config::{CompactionConfig, TTLConfig, ViperConfig, ViperSchemaBuilder};
#[deprecated(since = "0.1.0", note = "Use unified vector storage index types instead")]
pub use index::{HNSWConfig, HNSWIndex, IndexStrategy, VectorIndex};
#[deprecated(since = "0.1.0", note = "Use unified vector storage processors instead")]
pub use processor::{VectorProcessor, VectorRecordProcessor};
#[deprecated(since = "0.1.0", note = "Use unified vector storage schema instead")]
pub use schema::{SchemaGenerationStrategy, ViperSchemaStrategy};
#[deprecated(since = "0.1.0", note = "Use unified vector storage search instead")]
pub use search_engine::{SearchStats, ViperProgressiveSearchEngine};
#[deprecated(since = "0.1.0", note = "Use unified vector storage statistics instead")]
pub use stats::ViperStats;
#[deprecated(since = "0.1.0", note = "Use unified vector storage utilities instead")]
pub use ttl::{CleanupResult, TTLStats};
#[deprecated(since = "0.1.0", note = "Use unified vector storage types instead")]
pub use types::{SearchStrategy, ViperSearchContext, ViperSearchResult};
#[deprecated(since = "0.1.0", note = "Use unified vector storage results instead")]
pub use flusher::FlushResult;

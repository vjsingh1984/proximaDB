//! VIPER (Vectorized Intelligent Parquet with Efficient Retrieval) Module
//! 
//! This module provides a comprehensive, design pattern-based implementation
//! for efficient Parquet storage with dynamic schema generation and intelligent
//! data organization.
//! 
//! ## Architecture Overview
//! 
//! The VIPER module follows several design patterns for flexibility and maintainability:
//! 
//! - **Strategy Pattern**: Multiple schema generation strategies
//! - **Template Method Pattern**: Configurable vector processing algorithms  
//! - **Builder Pattern**: Complex schema configuration
//! - **Adapter Pattern**: Vector record to schema adaptation
//! - **Factory Pattern**: Strategy creation and management
//! 
//! ## Module Organization
//! 
//! - `config.rs` - Configuration structures and builders
//! - `schema.rs` - Schema generation strategies and traits
//! - `processor.rs` - Vector processing and template methods
//! - `adapter.rs` - Record to schema adaptation logic
//! - `factory.rs` - Strategy factories and creation patterns
//! - `flusher.rs` - Core Parquet flushing implementation
//! - `atomic_operations.rs` - Atomic flush/compaction with staging directories
//! - `stats.rs` - Performance statistics and monitoring

pub mod config;
pub mod schema;
pub mod processor;
pub mod adapter;
pub mod atomic_operations;
pub mod staging_operations;
pub mod factory;
pub mod flusher;
pub mod stats;
pub mod types;
pub mod storage_engine;
pub mod search_engine;
pub mod compaction;
pub mod compression;
pub mod partitioner;
pub mod ttl;
pub mod index;
pub mod search_impl;

// Re-export main types for convenience
pub use config::{ViperConfig, ViperSchemaBuilder, TTLConfig, CompactionConfig};
pub use schema::{SchemaGenerationStrategy, ViperSchemaStrategy};
pub use processor::{VectorProcessor, VectorRecordProcessor};
pub use adapter::VectorRecordSchemaAdapter;
pub use factory::ViperSchemaFactory;
pub use flusher::ViperParquetFlusher;
pub use search_engine::ViperProgressiveSearchEngine as ViperSearchEngine;
pub use stats::ViperStats;
pub use types::{SearchStrategy, ViperSearchContext, ViperSearchResult};
pub use storage_engine::ViperStorageEngine;
pub use search_engine::{ViperProgressiveSearchEngine, SearchStats};
pub use ttl::{TTLCleanupService, TTLStats, CleanupResult};
pub use index::{VectorIndex, ViperIndexManager, IndexStrategy, HNSWIndex, HNSWConfig};

// Re-export core functionality for backward compatibility
pub use flusher::FlushResult;
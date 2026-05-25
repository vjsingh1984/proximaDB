//! # Core Module - Foundation Types and Utilities
//!
//! This module provides the foundational types, configurations, and utilities that are used
//! throughout ProximaDB. It serves as the common vocabulary for all other modules, ensuring
//! consistency and type safety across the codebase.
//!
//! ## Role in ProximaDB Architecture
//!
//! The core module sits at the base of the dependency hierarchy:
//! ```text
//! All Modules
//!      ↓
//! Core Module (types, config, errors, utilities)
//! ```
//!
//! ## Key Components
//!
//! - **Types & Service Types**: Common type definitions (Collection, SearchRequest, etc.)
//! - **Configuration**: System configuration and loading
//! - **Error Handling**: Unified error types with `thiserror`
//! - **Search**: Core search functionality and result types
//! - **Compression**: Unified compression algorithms (LZ4, Snappy, Zstd, etc.)
//! - **Bloom Filters**: Probabilistic data structures for fast lookups
//! - **Hardware Capabilities**: SIMD/GPU detection and optimization
//! - **Memory Management**: Memory pools and allocation strategies

/// Core service types for vector operations (Collection, SearchRequest, etc.)
pub mod service_types;

/// Base62 encoding utilities for compact ID generation
pub mod base62;

/// System configuration structures and defaults
pub mod config;

/// Configuration loading from files and environment
pub mod config_loader;

/// Dynamic configuration reloading for production deployments
pub mod config_reloader;

/// Type conversions and proto-to-native mappings
pub mod conversions;

/// Legacy error module (being replaced by errors module)
pub mod error;

/// gRPC metadata parsing utilities
pub mod grpc_metadata_parser;

/// Index-related core types and traits
pub mod index;

/// Metadata query parsing and execution
pub mod metadata_query;

/// Core search functionality, filters, and result types
pub mod search;

/// Serialization utilities for various formats
pub mod serialization;

/// Unified compression module with 13 algorithm support
pub mod compression;

/// Shared context for dependency injection
pub mod context;

/// Storage-related core types and traits
pub mod storage;

/// Foundation traits and base implementations
pub mod foundation;

/// Storage layout and path management
pub mod storage_layout;

/// Memory management, pools, and allocation strategies
pub mod memory;

/// Protocol buffer metadata helpers
pub mod proto_metadata_helper;

/// Bloom filter implementations for fast lookups
pub mod bloom;

/// Unified error types with thiserror
pub mod errors;

/// Hardware capability detection (SIMD, GPU, etc.)
pub mod hardware_capabilities;

/// Ultra-efficient enum packing for 75% storage savings
pub mod enum_packing;

/// Common utility functions (metadata conversion, vector ops, validation)
pub mod utils;

/// Strongly-typed metadata structures for performance optimization
pub mod metadata_types;

/// Resilience patterns for enterprise-grade reliability
/// Includes: Circuit Breaker, Retry with Exponential Backoff
pub mod resilience;

/// Rich type system for ProximaRecord
/// Includes: ColumnDataType, TypedValue, validators, TEXT storage strategies
pub mod types;

#[cfg(test)]
mod config_tests;

#[cfg(test)]
mod config_loader_tests;

#[cfg(test)]
mod flush_config_tests;

pub use config::*;
pub use config_loader::*;
pub use error::*;
// Core service types for vector operations
pub use service_types::{
    BatchSearchRequest, CollectionConfig, CollectionOperation, CollectionRequest,
    CollectionResponse, CompactionConfig, CompactionStrategy, CompressionAlgorithm, DistanceMetric,
    FieldCondition, HealthResponse, IndexStats, IndexingAlgorithm, LegacyVectorSearchRequest,
    MetadataFilter, MetricsResponse, NodeId, OperationResponse, SearchDebugInfo, SearchMetadata,
    SearchRequest, SearchStrategy, ServiceMetrics, StorageEngine, String, Vector, VectorId,
    VectorInsertRequest, VectorInsertResponse, VectorOperation, VectorOperationMetrics,
    VectorSearchResponse, WriteBufferMetrics,
};

pub use grpc_metadata_parser::*;
pub use metadata_query::*;

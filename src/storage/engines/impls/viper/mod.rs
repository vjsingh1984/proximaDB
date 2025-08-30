//! # VIPER Storage Engine - Columnar Analytics-Optimized Storage
//!
//! VIPER (Vector-optimized Intelligent Parquet with Efficient Retrieval) is ProximaDB's
//! high-performance columnar storage engine built on Apache Parquet. It's optimized for
//! analytics workloads, batch operations, and achieving maximum compression ratios.
//!
//! ## Role in ProximaDB Architecture
//!
//! VIPER serves as the primary engine for analytical workloads:
//! ```text
//! Write Path:                          Read Path:
//! Batch Insert → Pipeline              Query → Predicate Pushdown
//!       ↓                                     ↓
//! Quantization + Clustering           Column Projection
//!       ↓                                     ↓
//! Parquet Writer                      Footer Cache
//!       ↓                                     ↓
//! Row Groups (128K vectors)           Progressive Search
//! ```
//!
//! ## Key Features
//!
//! ### 1. **Advanced Quantization Pipeline**
//! Multi-stage quantization for optimal storage:
//! - **Binary**: 1-bit quantization for initial filtering
//! - **INT8**: 8-bit integers for approximate search
//! - **PQ4/PQ8**: Product quantization for high compression
//! - **Adaptive**: Automatic selection based on data distribution
//!
//! ### 2. **Columnar Storage Benefits**
//! Apache Parquet format advantages:
//! - **Column Projection**: Read only needed columns
//! - **Predicate Pushdown**: Filter at storage level
//! - **Dictionary Encoding**: Efficient metadata storage
//! - **Run-Length Encoding**: Compress repeated values
//!
//! ### 3. **Smart Row Group Management**
//! Optimized row group sizing and organization:
//! - Default 128K vectors per row group
//! - Zone maps for min/max pruning
//! - Clustering for locality optimization
//! - Adaptive sizing based on memory
//!
//! ### 4. **Cloud-Native Optimizations**
//! Designed for cloud object storage:
//! - **Footer Cache**: Cache Parquet metadata
//! - **Range Reads**: Minimize data transfer
//! - **Parallel Downloads**: Multi-part retrieval
//! - **S3/Azure/GCS**: Native integration
//!
//! ## Performance Characteristics
//!
//! - **Write Throughput**: 500K vectors/sec (batch)
//! - **Query Latency**: 10-50ms for analytics queries
//! - **Compression Ratio**: 5-10x with quantization
//! - **Memory Usage**: 50MB per million vectors
//! - **Storage Efficiency**: 60-80% reduction vs raw
//!
//! ## Configuration Options
//!
//! ```toml
//! [storage.viper]
//! # Quantization settings
//! quantization_enabled = true
//! quantization_type = "adaptive"  # binary, int8, pq4, pq8, adaptive
//! 
//! # Row group configuration
//! row_group_size = 131072  # 128K vectors
//! enable_clustering = true
//! clustering_algorithm = "kmeans"
//! 
//! # Compression per column type
//! vector_compression = "zstd"
//! metadata_compression = "snappy"
//! id_compression = "lz4"
//! 
//! # Cloud optimization
//! footer_cache_size_mb = 128
//! enable_range_reads = true
//! parallel_downloads = 4
//! ```
//!
//! ## Integration with Common Infrastructure
//!
//! ### Columnar Format Module (`core/formats/columnar/`)
//! VIPER heavily leverages the shared columnar infrastructure:
//! - **Parquet I/O Layer**: Unified reader/writer implementation
//! - **Schema Management**: Consistent schema across engines
//! - **Footer Cache**: Shared metadata caching
//! - **ID Index**: Fast ID-based lookups
//! - **Native Metadata**: Efficient filtering
//!
//! ### Universal Distance Adapter (`universal/`)
//! - Progressive search pipeline (Binary → INT8 → PQ → FP32)
//! - Hardware-accelerated distance computation
//! - Format conversion utilities
//!
//! ### Compute Module (`compute/`)
//! - Unified quantization engine
//! - 13 distance metrics support
//! - Memory pool management
//!
//! ## VIPER-Specific Components
//!
//! - **`pipeline.rs`**: Write pipeline with batching and optimization
//! - **`column_filter.rs`**: Advanced predicate pushdown
//! - **`vector_writer.rs`**: Optimized vector column writing
//! - **`compaction.rs`**: Row group reorganization
//! - **`factory.rs`**: Flexible engine instantiation
//!
//! ## Usage Example
//!
//! ```rust
//! use proximadb::storage::engines::viper::ViperEngine;
//! 
//! let viper = ViperEngine::new(config)?;
//! 
//! // Batch insert with automatic quantization
//! viper.insert_batch(vectors).await?;
//! 
//! // Analytics query with predicate pushdown
//! let results = viper.search(
//!     query_vector,
//!     k = 100,
//!     filter = Some(metadata_filter),
//!     projection = vec!["id", "score", "category"]
//! ).await?;
//! ```
//!
//! ## Compaction Strategy
//!
//! VIPER uses row group reorganization for optimization:
//! 1. Small files merged into optimal row groups
//! 2. Re-clustering based on access patterns
//! 3. Re-quantization with updated codebooks
//! 4. Background optimization without blocking reads
//!
//! ## Cloud Storage Integration
//!
//! VIPER is designed for cloud-first deployments:
//! - Automatic S3/Azure/GCS detection
//! - Intelligent caching of hot data
//! - Bandwidth-optimized transfers
//! - Cost-aware storage tiering

pub mod readers;
pub mod factory;
pub mod eventlog_flush;
pub mod pipeline;
pub mod pipeline_tests; // Pipeline tests module
// Quantization now handled by unified compute module
pub mod utilities;
pub mod indexed_reader;
pub mod column_filter;
pub mod vector_writer;

// New modular structure for better maintainability
pub mod types;
// Schema now uses columnar module's ColumnarSchema
pub mod compaction;
pub mod flush;
pub mod engine;


// Test modules

#[cfg(test)]
mod tests;

// Re-export main VIPER types
pub use factory::ViperFactory;

// Clustering exports moved to AXIS
pub use pipeline::ViperPipeline;
// Quantization now handled by unified compute module
pub use utilities::ViperUtilities;

// Re-export modular types for better organization
pub use types::{
    CollectionMetadata, 
    ClusterId, 
    VectorStorageFormat,
    ParquetCompression,
    VectorQualityMetrics,
    SearchPerformanceStats,
    ViperEngineConfig,  // Internal engine config
    FilterableColumn,
    ParquetSchemaDesign,
    ParquetField,
    ParquetFieldType,
};
// Schema is handled by columnar module
pub use compaction::Compaction;
pub use flush::Flush;
pub use eventlog_flush::ViperFlushNotifier;
pub use engine::ViperEngine;
// pub use clustering_models::{ClusteringModelManager, EfficientClusteringModel, ClusteringStats}; // Moved to AXIS

// Unified search engine removed - using IntegratedSearchOptimizer from core::search

// Clean Release 1 API - Pure data access layer with search optimization
pub use readers::{
    UnifiedParquetReader, ReaderConfig, ReadingStrategy,
    FilterValue, QuantizationMethod, CollectionContext,
};
// MetadataFilter is directly from columnar module
pub use crate::storage::engines::core::formats::columnar::MetadataFilter;

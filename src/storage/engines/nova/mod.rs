//! NOVA Engine: Next-gen Optimized Vector Analytics with columnar quantization
//! Optimized Release 2 implementation with streaming and hierarchical statistics
//!
//! ## NOVA vs VIPER: Key Differentiators
//!
//! While both NOVA and VIPER are columnar engines sharing common infrastructure,
//! NOVA is designed for advanced analytics and research workloads with unique features:
//!
//! ### VIPER (Production-Ready):
//! - **Focus**: High-throughput production workloads (5K vec/s)
//! - **Compression**: Standard 50-80% with per-column optimization
//! - **Search**: Direct columnar scan with basic pruning
//! - **Quantization**: Binary/INT8/PQ with fixed levels
//! - **Use Cases**: Real-time search, production deployments
//! - **Maturity**: Production-ready, battle-tested
//!
//! ### NOVA (Advanced Analytics):
//! - **Focus**: Complex analytics and research (4K vec/s, optimized for compression)
//! - **Compression**: Advanced 80-90% with hierarchical compression chains
//! - **Search**: 3-tier statistics hierarchy for 70-90% I/O reduction
//! - **Quantization**: Adaptive progressive quantization with cost-based selection
//! - **Use Cases**: Research, advanced analytics, maximum compression scenarios
//! - **Maturity**: Beta/Research stage, experimental features
//!
//! ### Unique NOVA Features:
//! 1. **SuperBlock Statistics**: Hierarchical metadata for efficient pruning
//! 2. **Advanced Zone Maps**: Multi-dimensional pruning beyond simple min/max
//! 3. **Streaming Processors**: Memory-efficient processing of TB-scale data
//! 4. **Cost-Based Optimizer**: Intelligent query planning based on statistics
//! 5. **Compression Chains**: Multi-stage compression for maximum efficiency
//! 6. **Progressive Search**: Adaptive refinement based on query requirements
//!
//! ## How NOVA Leverages Common Modules
//!
//! ### 1. Columnar Module Integration (`columnar::`)
//! - **Parquet Operations**: Uses `UnifiedParquetReader` and `StreamingParquetWriter`
//!   for all file I/O, eliminating duplicate Parquet handling code
//! - **Footer Cache**: Leverages `ParquetFooterCache` for 70-90% cloud API reduction
//! - **ID Index**: Uses `ColumnarIdIndex` for O(log n) ID lookups
//! - **Bloom Filters**: Shared bloom filter implementation for existence checks
//! - **Schema Management**: Uses `ColumnarSchemaManager` for consistent schemas
//! - **Optimization**: Leverages `ColumnarOptimizer` for query planning
//!
//! ### 2. Universal Adapter Integration (`universal::`)
//! - **Progressive Search**: Uses universal's multi-stage refinement pipeline
//! - **Distance Computation**: All distance calculations through UniversalDistanceAdapter
//! - **Format Conversion**: Automatic conversion between columnar formats
//! - **Hardware Acceleration**: SIMD optimization via universal adapter
//!
//! ### 3. Compute Module Integration (`compute::`)
//! - **Quantization**: Uses unified `StorageQuantizationEngine` instead of local impl
//! - **Distance Metrics**: Full suite of 13 metrics from `UnifiedDistanceCompute`
//! - **Memory Pools**: Shared `VectorMemoryPool` for buffer reuse
//!
//! ### 4. Core Module Integration (`core::`)
//! - **Compression**: Uses unified compression with Mixed strategy for columns
//! - **Hardware Detection**: Automatic SIMD capability detection
//! - **Serialization**: Arrow-based serialization with VectorRecord compatibility
//!
//! ## NOVA-Specific Optimizations
//! - **Hierarchical Statistics**: SuperBlock statistics for efficient pruning
//! - **Zone Maps**: Advanced min/max tracking for row group elimination
//! - **Streaming Processing**: Memory-efficient processing of large files
//! - **Cost-Based Optimization**: Query planning based on data statistics

pub mod engine;
pub mod quantized_columns;
pub mod columnar_search;
pub mod batch_operations;
pub mod progressive_refinement;
pub mod optimized_operations;

// New optimized modules
pub mod hierarchical_stats;
pub mod hierarchical_cache;
pub mod streaming_processor;
pub mod progressive_search;
pub mod zone_maps;
pub mod streaming_search;

// Unified columnar infrastructure integration
pub mod unified_columnar_integration;

// Re-export main engine type and optimized components
pub use engine::NovaEngine;

// Re-export unified columnar integration
pub use unified_columnar_integration::{
    NovaUnifiedEngine, NovaSpecificConfig, StreamingInsertResult, 
    AdvancedSearchResult, NovaPerformanceMetrics, HierarchicalStatistics,
    ZoneMap, AdvancedSearchOptions,
};
pub use hierarchical_stats::{SuperBlock, EnhancedRowGroupStats};
pub use hierarchical_cache::{NovaHierarchicalCache, HierarchicalStats, GlobalStatistics, OptimizationHints};
pub use streaming_processor::{StreamingRowGroupProcessor, StreamingConfig};
pub use progressive_search::{ProgressiveColumnarSearch, ProgressiveSearchConfig};
pub use zone_maps::{AdvancedZoneMap, CostBasedOptimizer, ZoneMapConfig};
pub use streaming_search::{StreamingSearchEngine, StreamingSearchConfig};

use anyhow::Result;
use arrow_array::{ArrayRef, Float32Array, BinaryArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use parquet::file::metadata::RowGroupMetaData;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::VectorRecord;
use crate::storage::engines::columnar::FilterCondition;
use crate::compute::distance_computation::DistanceMetric;

// Import shared columnar infrastructure
use crate::storage::engines::columnar::{
    ColumnarFileMetadata, ColumnarSearchMode, MetadataFilter, QuantizationConfig, 
    ColumnStatistics, ParquetLocation,
};

// Use columnar types directly - no aliases needed

/// NOVA file structure with optimized columnar capabilities
#[derive(Debug)]
pub struct NovaFile {
    /// File metadata (using shared columnar metadata)
    pub metadata: ColumnarFileMetadata,
    
    /// Row group metadata (from Parquet)
    pub row_groups: Vec<RowGroupMetaData>,
    
    /// Enhanced row group statistics for optimization
    pub enhanced_stats: Vec<EnhancedRowGroupStats>,
    
    /// SuperBlock hierarchy for efficient pruning
    pub superblocks: Vec<SuperBlock>,
    
    /// Advanced zone maps for multi-dimensional pruning (optimized: basic zone maps)
    pub advanced_zone_maps: Option<hierarchical_stats::BasicZoneMaps>,
    
    /// Quantized column metadata
    pub quantized_columns: quantized_columns::QuantizedColumnMetadata,
    
    /// Schema with quantized columns
    pub schema: Arc<Schema>,
}

/// Main NOVA operations trait with streaming optimizations
pub trait NovaOperations {
    /// Streaming progressive search with all optimizations
    async fn search_streaming(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<MetadataFilter>,
        config: Option<StreamingSearchConfig>,
    ) -> Result<streaming_search::StreamingSearchResult>;
    
    /// Progressive similarity search (legacy compatibility)
    async fn progressive_search(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<VectorRecord>>;
    
    /// Get vectors by IDs using columnar scanning (no separate index)
    async fn get_by_ids_columnar(&self, ids: &[String]) -> Result<Vec<VectorRecord>>;
    
    /// Get enhanced row group statistics
    fn get_enhanced_stats(&self) -> &[EnhancedRowGroupStats];
    
    /// Get SuperBlock hierarchy
    fn get_superblocks(&self) -> &[SuperBlock];
    
    /// Update statistics based on query patterns
    async fn update_adaptive_stats(&self, query_patterns: &[zone_maps::QueryPattern]) -> Result<()>;
}

/// Row group statistics for optimization
#[derive(Debug, Clone)]
pub struct RowGroupStats {
    pub row_group_id: usize,
    pub num_rows: u64,
    pub compressed_size: u64,
    pub id_range: (String, String),
    pub has_quantized_columns: bool,
}

/// VIPER-specific optimizations
pub struct ViperOptimizations {
    /// Columnar projection - only load needed columns
    pub projection: Vec<String>,
    
    /// Predicate pushdown - filter at storage level
    pub predicates: Vec<FilterCondition>,
    
    /// Row group pruning - skip irrelevant groups
    pub pruned_groups: Vec<usize>,
    
    /// Quantization level for search
    pub quantization_level: QuantizationLevel,
}

#[derive(Debug, Clone)]
pub enum QuantizationLevel {
    None,
    Binary,
    Int8,
    ProductQuantization,
    Progressive,
}

/// Create optimized Parquet schema for vectors
pub fn create_vector_schema(
    dimension: usize,
    config: &QuantizationConfig,
    filterable_columns: &[String],
) -> Arc<Schema> {
    let mut fields = vec![
        // Core fields
        Field::new("id", DataType::Utf8, false),
        Field::new("vector", DataType::FixedSizeBinary(dimension as i32 * 4), false),
        Field::new("timestamp", DataType::Int64, false),
        Field::new("version", DataType::Int64, true),
    ];
    
    // Add quantized columns if enabled
    if config.enable_binary {
        fields.push(Field::new(
            "vector_binary",
            DataType::FixedSizeBinary((dimension + 7) / 8),
            true,
        ));
    }
    
    if config.enable_int8 {
        fields.push(Field::new(
            "vector_int8",
            DataType::FixedSizeBinary(dimension as i32),
            true,
        ));
        fields.push(Field::new("int8_scale", DataType::Float32, true));
        fields.push(Field::new("int8_zero_point", DataType::Int8, true));
    }
    
    if config.enable_pq {
        fields.push(Field::new(
            "vector_pq",
            DataType::FixedSizeBinary(config.pq_segments as i32),
            true,
        ));
    }
    
    // Add filterable metadata columns
    for column in filterable_columns {
        // Infer type from first value (in production, use schema registry)
        fields.push(Field::new(column, DataType::Utf8, true));
    }
    
    Arc::new(Schema::new(fields))
}

/// Estimate memory usage for a row group
pub fn estimate_row_group_memory(
    row_group: &RowGroupMetaData,
    schema: &Schema,
) -> usize {
    let mut total = 0;
    
    for (idx, column) in row_group.columns().iter().enumerate() {
        let field = &schema.fields()[idx];
        let uncompressed_size = column.uncompressed_size() as usize;
        
        // Add overhead for Arrow arrays
        total += uncompressed_size + (uncompressed_size / 10); // 10% overhead estimate
    }
    
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_create_vector_schema() {
        let config = QuantizationConfig::default();
        let filterable = vec!["category".to_string(), "price".to_string()];
        
        let schema = create_vector_schema(768, &config, &filterable);
        
        // Check core fields
        assert!(schema.field_with_name("id").is_ok());
        assert!(schema.field_with_name("vector").is_ok());
        assert!(schema.field_with_name("timestamp").is_ok());
        
        // Check quantized fields
        assert!(schema.field_with_name("vector_binary").is_ok());
        assert!(schema.field_with_name("vector_int8").is_ok());
        assert!(schema.field_with_name("vector_pq").is_ok());
        
        // Check metadata fields
        assert!(schema.field_with_name("category").is_ok());
        assert!(schema.field_with_name("price").is_ok());
    }
    
    #[test]
    fn test_quantization_config() {
        let config = QuantizationConfig::default();
        
        assert!(config.enable_binary);
        assert!(config.enable_int8);
        assert!(config.enable_pq);
        assert_eq!(config.pq_segments, 16);
        assert_eq!(config.pq_bits, 8);
    }
}
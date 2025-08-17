// NOVA Engine: Next-gen Optimized Vector Analytics with columnar quantization
// Optimized Release 2 implementation with streaming and hierarchical statistics

pub mod engine;
pub mod quantized_columns;
pub mod columnar_search;
pub mod batch_operations;
pub mod progressive_refinement;
pub mod optimized_operations;

// New optimized modules
pub mod hierarchical_stats;
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
    pub row_groups: Vec<RowGroupMetadata>,
    
    /// Enhanced row group statistics for optimization
    pub enhanced_stats: Vec<EnhancedRowGroupStats>,
    
    /// SuperBlock hierarchy for efficient pruning
    pub superblocks: Vec<SuperBlock>,
    
    /// Advanced zone maps for multi-dimensional pruning
    pub advanced_zone_maps: Option<AdvancedZoneMap>,
    
    /// Quantized column metadata
    pub quantized_columns: quantized_columns::QuantizedColumnMetadata,
    
    /// Schema with quantized columns
    pub schema: Arc<Schema>,
}

// NovaMetadata removed - use ColumnarFileMetadata directly from columnar module

// QuantizationConfig already imported above - no need to re-export

/// NOVA can use ColumnarFileMetadata directly, no need for specific config
/// If NOVA-specific fields are needed in future, extend ColumnarFileMetadata

// Remove duplicate Default impl - it's already in columnar module

// ColumnStatistics already imported from columnar module - no local definition needed

// SearchMode already imported from columnar module as ColumnarSearchMode - use that type directly

// MetadataFilter is imported from columnar module - no local definition needed

#[derive(Debug, Clone)]
pub enum FilterLogic {
    And,
    Or,
}

#[derive(Debug, Clone)]
pub enum FilterCondition {
    Equals(String, serde_json::Value),
    Range(String, serde_json::Value, serde_json::Value),
    In(String, Vec<serde_json::Value>),
    IsNull(String),
    IsNotNull(String),
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
    row_group: &RowGroupMetadata,
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
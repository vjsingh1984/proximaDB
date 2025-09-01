//! Unified Columnar Schema Generation
use arrow_schema::DataType;
use arrow_array::{RecordBatch, StringArray};
// This module provides automatic quantization-aware schema generation for VIPER and NOVA engines.
// When QuantizationConfig is detected, schemas automatically include quantized columns with
// optimized mixed compression strategies per column type.

use anyhow::{Context, Result};
use arrow_schema::{Field, Schema, TimeUnit};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, trace, warn};

use crate::core::compression::CompressionAlgorithm;
use crate::compute::quantization::storage_engine::StorageQuantizationConfig;
use super::{QuantizationConfig, ColumnarFileMetadata};

/// Schema configuration with quantization awareness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnarSchemaConfig {
    /// Vector dimension
    pub dimension: usize,
    
    /// Quantization configuration from collection
    pub quantization: Option<QuantizationConfig>,
    
    /// Filterable columns specification
    pub filterable_columns: Vec<FilterableColumnSpec>,
    
    /// Schema optimization settings
    pub optimization: SchemaOptimization,
    
    /// Compression strategy per column type
    pub compression_strategy: CompressionStrategy,
}

/// Filterable column specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterableColumnSpec {
    pub name: String,
    pub data_type: FilterableData,
    pub nullable: bool,
    pub indexed: bool,
    pub estimated_cardinality: Option<usize>,
}

/// Supported filterable data types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterableData {
    String,
    Integer,
    Float,
    Boolean,
    Datetime,
    Array(Box<FilterableData>),
    Json,
}

/// Schema optimization settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaOptimization {
    /// Enable dictionary encoding for low-cardinality strings
    pub enable_dictionary_encoding: bool,
    
    /// Enable nullable optimization (reduce null overhead)
    pub optimize_nullability: bool,
    
    /// Enable timestamp precision optimization
    pub optimize_timestamp_precision: bool,
    
    /// Enable fixed-size binary optimization for vectors
    pub enable_fixed_size_binary: bool,
    
    /// Target row group size for optimal I/O
    pub target_row_group_size: usize,
}

/// Compression strategy per column type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionStrategy {
    /// Compression for FP32 vector columns
    pub fp32_vectors: CompressionAlgorithm,
    
    /// Compression for quantized vector columns
    pub quantized_vectors: CompressionAlgorithm,
    
    /// Compression for metadata columns
    pub metadata_columns: CompressionAlgorithm,
    
    /// Compression for ID columns
    pub id_columns: CompressionAlgorithm,
    
    /// No compression for binary sketches (already compact)
    pub binary_sketches: Option<CompressionAlgorithm>,
}

impl Default for CompressionStrategy {
    fn default() -> Self {
        Self {
            // ZSTD for FP32 vectors - best compression ratio
            fp32_vectors: CompressionAlgorithm::Zstd,
            
            // LZ4 for quantized vectors - fast decompression, already compact
            quantized_vectors: CompressionAlgorithm::Lz4,
            
            // ZSTD for metadata - excellent text compression
            metadata_columns: CompressionAlgorithm::Zstd,
            
            // Dictionary + ZSTD for IDs - handles repeated patterns
            id_columns: CompressionAlgorithm::Zstd,
            
            // No compression for binary sketches - they're already bit-packed
            binary_sketches: None,
        }
    }
}

impl Default for SchemaOptimization {
    fn default() -> Self {
        Self {
            enable_dictionary_encoding: true,
            optimize_nullability: true,
            optimize_timestamp_precision: true,
            enable_fixed_size_binary: true,
            target_row_group_size: 50_000,
        }
    }
}

/// Unified schema builder for VIPER and NOVA engines
pub struct ColumnarSchemaBuilder {
    /// Schema cache to avoid regenerating identical schemas
    schema_cache: Arc<RwLock<HashMap<String, CachedSchema>>>,
    
    /// Default optimization settings
    default_optimization: SchemaOptimization,
    
    /// Default compression strategy
    default_compression: CompressionStrategy,
}

/// Cached schema with expiration
#[derive(Debug, Clone)]
struct CachedSchema {
    schema: Arc<Schema>,
    compression_metadata: CompressionMetadata,
    timestamp: std::time::Instant,
    ttl: std::time::Duration,
}

/// Compression metadata for schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionMetadata {
    /// Compression settings per column
    pub column_compression: HashMap<String, CompressionAlgorithm>,
    
    /// Expected compression ratios
    pub compression_ratios: HashMap<String, f32>,
    
    /// Parquet writer properties
    pub writer_properties: WriterPropertiesConfig,
}

/// Parquet writer properties configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriterPropertiesConfig {
    pub row_group_size: usize,
    pub page_size: usize,
    pub dictionary_enabled: bool,
    pub statistics_enabled: bool,
    pub bloom_filter_enabled: bool,
}

impl Default for WriterPropertiesConfig {
    fn default() -> Self {
        Self {
            row_group_size: 50_000,
            page_size: 1024 * 1024, // 1MB pages
            dictionary_enabled: true,
            statistics_enabled: true,
            bloom_filter_enabled: true,
        }
    }
}

impl ColumnarSchemaBuilder {
    /// Create new schema builder
    pub fn new() -> Self {
        Self {
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
            default_optimization: SchemaOptimization::default(),
            default_compression: CompressionStrategy::default(),
        }
    }
    
    /// Create schema builder with custom defaults
    pub fn with_defaults(
        optimization: SchemaOptimization,
        compression: CompressionStrategy,
    ) -> Self {
        Self {
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
            default_optimization: optimization,
            default_compression: compression,
        }
    }
    
    /// Build optimized schema for collection with automatic quantization detection
    pub async fn build_schema(
        &self,
        collection_id: &str,
        config: &ColumnarSchemaConfig,
    ) -> Result<(Arc<Schema>, CompressionMetadata)> {
        // Generate cache key based on configuration
        let cache_key = self.generate_cache_key(collection_id, config);
        
        // Check cache first
        if let Some(cached) = self.get_cached_schema(&cache_key).await {
            if !cached.is_expired() {
                debug!("Schema cache hit for collection: {}", collection_id);
                return Ok((cached.schema, cached.compression_metadata));
            }
        }
        
        info!("Building quantization-aware schema for collection: {} (dim: {})", 
              collection_id, config.dimension);
        
        let (schema, compression_metadata) = self.build_schema_internal(config)?;
        
        // Cache the result
        self.cache_schema(cache_key, schema.clone(), compression_metadata.clone()).await;
        
        info!("Schema built with {} fields, {} quantized columns", 
              schema.fields().len(),
              self.count_quantized_columns(&schema));
        
        Ok((schema, compression_metadata))
    }
    
    /// Build schema with automatic quantization column generation
    fn build_schema_internal(
        &self,
        config: &ColumnarSchemaConfig,
    ) -> Result<(Arc<Schema>, CompressionMetadata)> {
        let mut fields = Vec::new();
        let mut column_compression = HashMap::new();
        let mut compression_ratios = HashMap::new();
        
        // Core fields - always present
        self.add_core_fields(&mut fields, &mut column_compression, &mut compression_ratios, config)?;
        
        // Vector field - FP32 baseline
        self.add_vector_field(&mut fields, &mut column_compression, &mut compression_ratios, config)?;
        
        // Quantized vector fields - automatic based on QuantizationConfig
        if let Some(ref quant_config) = config.quantization {
            self.add_quantized_fields(&mut fields, &mut column_compression, &mut compression_ratios, config, quant_config)?;
        }
        
        // Filterable metadata columns
        self.add_filterable_fields(&mut fields, &mut column_compression, &mut compression_ratios, config)?;
        
        // Extra metadata field for non-filterable data
        self.add_extra_metadata_field(&mut fields, &mut column_compression, &mut compression_ratios, config)?;
        
        let schema = Arc::new(Schema::new(fields));
        let compression_metadata = CompressionMetadata {
            column_compression,
            compression_ratios,
            writer_properties: WriterPropertiesConfig::default(),
        };
        
        Ok((schema, compression_metadata))
    }
    
    /// Add core fields (ID, timestamp, version, etc.)
    fn add_core_fields(
        &self,
        fields: &mut Vec<Field>,
        column_compression: &mut HashMap<String, CompressionAlgorithm>,
        compression_ratios: &mut HashMap<String, f32>,
        config: &ColumnarSchemaConfig,
    ) -> Result<()> {
        // ID field - required for customer APIs
        fields.push(Field::new("id", DataType::Utf8, false));
        column_compression.insert("id".to_string(), config.compression_strategy.id_columns);
        compression_ratios.insert("id".to_string(), 2.0); // Good compression for UUIDs
        
        // Timestamp fields with optimized precision
        let timestamp_type = if config.optimization.optimize_timestamp_precision {
            DataType::Timestamp(TimeUnit::Millisecond, None) // Sufficient for most use cases
        } else {
            DataType::Timestamp(TimeUnit::Microsecond, None)
        };
        
        fields.push(Field::new("timestamp", timestamp_type.clone(), false));
        fields.push(Field::new("created_at", timestamp_type.clone(), true));
        fields.push(Field::new("updated_at", timestamp_type, true));
        
        // No compression for timestamps - they're already compact
        for field in ["timestamp", "created_at", "updated_at"] {
            compression_ratios.insert(field.to_string(), 1.0);
        }
        
        // Version field for MVCC
        fields.push(Field::new("version", DataType::Int64, true));
        compression_ratios.insert("version".to_string(), 1.5);
        
        // Expiration field for TTL
        fields.push(Field::new("expires_at", DataType::Timestamp(TimeUnit::Millisecond, None), true));
        compression_ratios.insert("expires_at".to_string(), 1.0);
        
        debug!("Added {} core fields", fields.len());
        Ok(())
    }
    
    /// Add FP32 vector field with optimal configuration
    fn add_vector_field(
        &self,
        fields: &mut Vec<Field>,
        column_compression: &mut HashMap<String, CompressionAlgorithm>,
        compression_ratios: &mut HashMap<String, f32>,
        config: &ColumnarSchemaConfig,
    ) -> Result<()> {
        let vector_type = if config.optimization.enable_fixed_size_binary {
            // Fixed-size binary for better performance and compression
            DataType::FixedSizeBinary(config.dimension as i32 * 4)
        } else {
            // List of floats for flexibility
            DataType::List(Arc::new(Field::new("item", DataType::Float32, false)))
        };
        
        fields.push(Field::new("vector", vector_type, false));
        column_compression.insert("vector".to_string(), config.compression_strategy.fp32_vectors);
        
        // ZSTD typically achieves 2-4x compression on float vectors
        compression_ratios.insert("vector".to_string(), 3.0);
        
        trace!("Added FP32 vector field (dim: {})", config.dimension);
        Ok(())
    }
    
    /// Add quantized vector fields based on QuantizationConfig
    fn add_quantized_fields(
        &self,
        fields: &mut Vec<Field>,
        column_compression: &mut HashMap<String, CompressionAlgorithm>,
        compression_ratios: &mut HashMap<String, f32>,
        config: &ColumnarSchemaConfig,
        quant_config: &QuantizationConfig,
    ) -> Result<()> {
        let mut quantized_count = 0;
        
        // Binary quantization - ultra-fast filtering
        if quant_config.enable_binary {
            let binary_size = (config.dimension + 7) / 8; // Bits to bytes
            fields.push(Field::new(
                "vector_binary",
                DataType::FixedSizeBinary(binary_size as i32),
                true, // Nullable for progressive rollout
            ));
            
            // Binary sketches are already maximally compressed
            if let Some(compression) = config.compression_strategy.binary_sketches {
                column_compression.insert("vector_binary".to_string(), compression);
                compression_ratios.insert("vector_binary".to_string(), 1.2);
            } else {
                compression_ratios.insert("vector_binary".to_string(), 1.0);
            }
            
            quantized_count += 1;
            trace!("Added binary quantization field ({} bytes)", binary_size);
        }
        
        // INT8 quantization - good balance of speed and quality
        if quant_config.enable_int8 {
            fields.push(Field::new(
                "vector_int8",
                DataType::FixedSizeBinary(config.dimension as i32),
                true,
            ));
            fields.push(Field::new("int8_scale", DataType::Float32, true));
            fields.push(Field::new("int8_zero_point", DataType::Int8, true));
            
            column_compression.insert("vector_int8".to_string(), config.compression_strategy.quantized_vectors);
            compression_ratios.insert("vector_int8".to_string(), 1.5); // INT8 compresses moderately
            compression_ratios.insert("int8_scale".to_string(), 1.0);
            compression_ratios.insert("int8_zero_point".to_string(), 1.0);
            
            quantized_count += 1;
            trace!("Added INT8 quantization fields");
        }
        
        // Product Quantization - configurable precision
        if quant_config.enable_pq {
            let pq_size = quant_config.pq_segments as i32;
            fields.push(Field::new(
                "vector_pq",
                DataType::FixedSizeBinary(pq_size),
                true,
            ));
            
            // PQ codes benefit from dictionary encoding
            column_compression.insert("vector_pq".to_string(), config.compression_strategy.quantized_vectors);
            compression_ratios.insert("vector_pq".to_string(), 2.0); // Good compression for PQ codes
            
            quantized_count += 1;
            trace!("Added PQ quantization field ({} segments)", quant_config.pq_segments);
        }
        
        info!("Added {} quantized vector columns", quantized_count);
        Ok(())
    }
    
    /// Add filterable metadata columns
    fn add_filterable_fields(
        &self,
        fields: &mut Vec<Field>,
        column_compression: &mut HashMap<String, CompressionAlgorithm>,
        compression_ratios: &mut HashMap<String, f32>,
        config: &ColumnarSchemaConfig,
    ) -> Result<()> {
        for filterable in &config.filterable_columns {
            let data_type = self.convert_filterable_type(&filterable.data_type)?;
            
            // Enable dictionary encoding for low-cardinality strings
            let field = if config.optimization.enable_dictionary_encoding 
                && matches!(filterable.data_type, FilterableData::String)
                && filterable.estimated_cardinality.map_or(false, |c| c < 10000) 
            {
                Field::new(&filterable.name, data_type, filterable.nullable)
                    .with_metadata(HashMap::from([
                        ("encoding".to_string(), "dictionary".to_string())
                    ]))
            } else {
                Field::new(&filterable.name, data_type, filterable.nullable)
            };
            
            fields.push(field);
            column_compression.insert(filterable.name.clone(), config.compression_strategy.metadata_columns);
            
            // Estimate compression ratio based on data type
            let ratio = match filterable.data_type {
                FilterableData::String => 3.0, // Text compresses well
                FilterableData::Json => 4.0,   // JSON compresses very well
                FilterableData::Array(_) => 2.5,
                _ => 1.5, // Numbers compress moderately
            };
            compression_ratios.insert(filterable.name.clone(), ratio);
        }
        
        debug!("Added {} filterable columns", config.filterable_columns.len());
        Ok(())
    }
    
    /// Add extra metadata field for non-filterable data
    fn add_extra_metadata_field(
        &self,
        fields: &mut Vec<Field>,
        column_compression: &mut HashMap<String, CompressionAlgorithm>,
        compression_ratios: &mut HashMap<String, f32>,
        config: &ColumnarSchemaConfig,
    ) -> Result<()> {
        // Key-value pairs for arbitrary metadata
        let kv_struct = DataType::Struct(
            vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ].into()
        );
        
        fields.push(Field::new(
            "extra_metadata_info",
            DataType::List(Arc::new(Field::new("item", kv_struct, true))),
            true,
        ));
        
        column_compression.insert("extra_metadata_info".to_string(), config.compression_strategy.metadata_columns);
        compression_ratios.insert("extra_metadata_info".to_string(), 4.0); // JSON-like data compresses well
        
        trace!("Added extra metadata field");
        Ok(())
    }
    
    /// Convert filterable data type to Arrow data type
    fn convert_filterable_type(&self, data_type: &FilterableData) -> Result<DataType> {
        let arrow_type = match data_type {
            FilterableData::String => DataType::Utf8,
            FilterableData::Integer => DataType::Int64,
            FilterableData::Float => DataType::Float64,
            FilterableData::Boolean => DataType::Boolean,
            FilterableData::Datetime => DataType::Timestamp(TimeUnit::Millisecond, None),
            FilterableData::Json => DataType::Utf8, // Store as JSON string
            FilterableData::Array(inner) => {
                let inner_type = self.convert_filterable_type(inner)?;
                DataType::List(Arc::new(Field::new("item", inner_type, false)))
            }
        };
        Ok(arrow_type)
    }
    
    /// Generate cache key for schema configuration
    fn generate_cache_key(&self, collection_id: &str, config: &ColumnarSchemaConfig) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        config.dimension.hash(&mut hasher);
        
        if let Some(ref quant) = config.quantization {
            quant.enable_binary.hash(&mut hasher);
            quant.enable_int8.hash(&mut hasher);
            quant.enable_pq.hash(&mut hasher);
            quant.pq_segments.hash(&mut hasher);
        }
        
        for col in &config.filterable_columns {
            col.name.hash(&mut hasher);
        }
        
        format!("schema_{}_{:x}", collection_id, hasher.finish())
    }
    
    /// Get cached schema if valid
    async fn get_cached_schema(&self, cache_key: &str) -> Option<CachedSchema> {
        let cache = self.schema_cache.read().await;
        cache.get(cache_key).cloned()
    }
    
    /// Cache schema with TTL
    async fn cache_schema(
        &self,
        cache_key: String,
        schema: Arc<Schema>,
        compression_metadata: CompressionMetadata,
    ) {
        let cached = CachedSchema {
            schema,
            compression_metadata,
            timestamp: std::time::Instant::now(),
            ttl: std::time::Duration::from_secs(3600), // 1 hour TTL
        };
        
        let mut cache = self.schema_cache.write().await;
        cache.insert(cache_key, cached);
    }
    
    /// Count quantized columns in schema
    fn count_quantized_columns(&self, schema: &Schema) -> usize {
        schema.fields().iter()
            .filter(|field| field.name().starts_with("vector_"))
            .count()
    }
    
    /// Clear cache for collection
    pub async fn clear_cache(&self, collection_id: &str) {
        let mut cache = self.schema_cache.write().await;
        cache.retain(|key, _| !key.contains(collection_id));
        debug!("Cleared schema cache for collection: {}", collection_id);
    }
    
    /// Get cache statistics
    pub async fn cache_stats(&self) -> (usize, usize) {
        let cache = self.schema_cache.read().await;
        let total = cache.len();
        let expired = cache.values().filter(|cached| cached.is_expired()).count();
        (total, expired)
    }
}

impl CachedSchema {
    /// Check if cached schema has expired
    fn is_expired(&self) -> bool {
        self.timestamp.elapsed() > self.ttl
    }
}

impl Default for ColumnarSchemaBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function to create schema from collection metadata
pub async fn create_schema_from_collection(
    collection_id: &str,
    dimension: usize,
    quantization: Option<&QuantizationConfig>,
    filterable_columns: &[FilterableColumnSpec],
) -> Result<(Arc<Schema>, CompressionMetadata)> {
    let builder = ColumnarSchemaBuilder::new();
    
    let config = ColumnarSchemaConfig {
        dimension,
        quantization: quantization.cloned(),
        filterable_columns: filterable_columns.to_vec(),
        optimization: SchemaOptimization::default(),
        compression_strategy: CompressionStrategy::default(),
    };
    
    builder.build_schema(collection_id, &config).await
}

/// Validate schema compatibility with quantization config
pub fn validate_schema_compatibility(
    schema: &Schema,
    quantization: &QuantizationConfig,
) -> Result<()> {
    // Check that required quantized columns exist
    if quantization.enable_binary && schema.field_with_name("vector_binary").is_err() {
        return Err(anyhow::anyhow!("Binary quantization enabled but vector_binary column missing"));
    }
    
    if quantization.enable_int8 && schema.field_with_name("vector_int8").is_err() {
        return Err(anyhow::anyhow!("INT8 quantization enabled but vector_int8 column missing"));
    }
    
    if quantization.enable_pq && schema.field_with_name("vector_pq").is_err() {
        return Err(anyhow::anyhow!("PQ quantization enabled but vector_pq column missing"));
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_schema_generation_without_quantization() {
        let builder = ColumnarSchemaBuilder::new();
        
        let config = ColumnarSchemaConfig {
            dimension: 768,
            quantization: None,
            filterable_columns: vec![
                FilterableColumnSpec {
                    name: "category".to_string(),
                    // data_type removed -  FilterableDataType::String,
                    nullable: true,
                    indexed: false,
                    estimated_cardinality: Some(100),
                }
            ],
            optimization: SchemaOptimization::default(),
            compression_strategy: CompressionStrategy::default(),
        };
        
        let (schema, metadata) = builder.build_schema("test_collection", &config).await.unwrap();
        
        // Should have core fields + vector + filterable + extra_metadata
        assert!(schema.fields().len() >= 8);
        
        // Check core fields
        assert!(schema.field_with_name("id").is_ok());
        assert!(schema.field_with_name("vector").is_ok());
        assert!(schema.field_with_name("timestamp").is_ok());
        assert!(schema.field_with_name("category").is_ok());
        
        // Should not have quantized fields
        assert!(schema.field_with_name("vector_binary").is_err());
        assert!(schema.field_with_name("vector_int8").is_err());
        assert!(schema.field_with_name("vector_pq").is_err());
        
        // Check compression metadata
        assert!(metadata.column_compression.contains_key("vector"));
        assert!(metadata.compression_ratios.contains_key("vector"));
    }
    
    #[tokio::test]
    async fn test_schema_generation_with_quantization() {
        let builder = ColumnarSchemaBuilder::new();
        
        let quant_config = QuantizationConfig {
            enable_binary: true,
            enable_int8: true,
            enable_pq: true,
            pq_segments: 16,
            ..Default::default()
        };
        
        let config = ColumnarSchemaConfig {
            dimension: 768,
            quantization: Some(quant_config),
            filterable_columns: vec![],
            optimization: SchemaOptimization::default(),
            compression_strategy: CompressionStrategy::default(),
        };
        
        let (schema, metadata) = builder.build_schema("test_collection", &config).await.unwrap();
        
        // Should have quantized fields
        assert!(schema.field_with_name("vector_binary").is_ok());
        assert!(schema.field_with_name("vector_int8").is_ok());
        assert!(schema.field_with_name("int8_scale").is_ok());
        assert!(schema.field_with_name("int8_zero_point").is_ok());
        assert!(schema.field_with_name("vector_pq").is_ok());
        
        // Check compression strategies for quantized columns
        assert!(metadata.column_compression.contains_key("vector_int8"));
        assert!(metadata.column_compression.contains_key("vector_pq"));
        
        // Binary field might not have compression
        assert!(metadata.compression_ratios.contains_key("vector_binary"));
    }
    
    #[tokio::test]
    async fn test_schema_caching() {
        let builder = ColumnarSchemaBuilder::new();
        
        let config = ColumnarSchemaConfig {
            dimension: 512,
            quantization: None,
            filterable_columns: vec![],
            optimization: SchemaOptimization::default(),
            compression_strategy: CompressionStrategy::default(),
        };
        
        // First call - should build schema
        let (schema1, _) = builder.build_schema("cache_test", &config).await.unwrap();
        
        // Second call - should hit cache
        let (schema2, _) = builder.build_schema("cache_test", &config).await.unwrap();
        
        // Should be the same Arc instance (cached)
        assert!(Arc::ptr_eq(&schema1, &schema2));
        
        // Check cache stats
        let (total, expired) = builder.cache_stats().await;
        assert_eq!(total, 1);
        assert_eq!(expired, 0);
    }
    
    #[test]
    fn test_filterable_type_conversion() {
        let builder = ColumnarSchemaBuilder::new();
        
        assert!(matches!(
            builder.convert_filterable_type(&FilterableData::String).unwrap(),
            DataType::Utf8
        ));
        
        assert!(matches!(
            builder.convert_filterable_type(&FilterableData::Integer).unwrap(),
            DataType::Int64
        ));
        
        assert!(matches!(
            builder.convert_filterable_type(&FilterableData::Array(Box::new(FilterableData::String))).unwrap(),
            DataType::List(_)
        ));
    }
    
    #[test]
    fn test_schema_validation() {
        let fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("vector", DataType::FixedSizeBinary(768 * 4), false),
            Field::new("vector_binary", DataType::FixedSizeBinary(96), true),
            Field::new("vector_int8", DataType::FixedSizeBinary(768), true),
            Field::new("vector_pq", DataType::FixedSizeBinary(16), true),
        ];
        
        let schema = Schema::new(fields);
        
        let quant_config = QuantizationConfig {
            enable_binary: true,
            enable_int8: true,
            enable_pq: true,
            ..Default::default()
        };
        
        // Should validate successfully
        validate_schema_compatibility(&schema, &quant_config).unwrap();
        
        // Test missing field
        let quant_config_missing = QuantizationConfig {
            enable_binary: true,
            enable_int8: true,
            enable_pq: false,
            ..Default::default()
        };
        
        validate_schema_compatibility(&schema, &quant_config_missing).unwrap();
    }
}
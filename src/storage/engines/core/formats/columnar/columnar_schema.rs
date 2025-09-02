// Columnar Schema Manager
use arrow_schema::DataType;// Manages Parquet schemas with quantization support for NOVA and VIPER

use anyhow::Result;
use arrow_schema::{Field, Schema};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::{QuantizationConfig, ColumnarFileMetadata};

/// Schema operations for columnar storage with quantization support
#[derive(Debug)]
pub struct ColumnarSchema {
    /// Cached schemas by collection ID
    schema_cache: Arc<RwLock<HashMap<String, CachedSchema>>>,
    
    /// Default configuration
    default_config: QuantizationConfig,
}

impl ColumnarSchema {
    /// Create new schema manager
    pub fn new() -> Self {
        Self {
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
            default_config: QuantizationConfig::default(),
        }
    }
    
    /// Create new schema manager with configuration
    pub fn with_config(config: QuantizationConfig) -> Self {
        Self {
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
            default_config: config,
        }
    }
    
    /// Create optimized schema for vector storage
    pub async fn create_vector_schema(
        &self,
        collection_id: &str,
        dimension: usize,
        quantization: Option<&QuantizationConfig>,
        filterable_columns: &[FilterableColumn],
    ) -> Result<Arc<Schema>> {
        // Check cache first
        let cache_key = self.generate_cache_key(collection_id, dimension, quantization, filterable_columns);
        if let Some(cached) = self.get_cached_schema(&cache_key).await {
            if !cached.is_expired() {
                debug!("Schema cache hit for collection: {}", collection_id);
                return Ok(cached.schema);
            }
        }
        
        info!("Creating vector schema for collection: {} (dim: {})", collection_id, dimension);
        
        let default_config = QuantizationConfig::default();
        let config = quantization.unwrap_or(&default_config);
        let schema = self.build_schema(dimension, config, filterable_columns)?;
        
        // Cache the schema
        self.cache_schema(cache_key, schema.clone()).await;
        
        Ok(schema)
    }
    
    /// Build the actual schema
    fn build_schema(
        &self,
        dimension: usize,
        config: &QuantizationConfig,
        filterable_columns: &[FilterableColumn],
    ) -> Result<Arc<Schema>> {
        let mut fields = vec![
            // Core vector fields
            Field::new("id", DataType::Utf8, false),
            Field::new("collection_id", DataType::Utf8, false),
            Field::new("vector", self.get_vector_data_type(dimension), false),
            Field::new("timestamp", DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None), false),
            Field::new("created_at", DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None), true),
            Field::new("updated_at", DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None), true),
            Field::new("version", DataType::Int64, true),
            Field::new("expires_at", DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None), true),
        ];
        
        // Add quantized vector columns if enabled
        if config.enable_binary {
            fields.push(Field::new(
                "vector_binary",
                DataType::FixedSizeBinary(((dimension + 7) / 8) as i32),
                true,
            ));
            debug!("Added binary quantization column");
        }
        
        if config.enable_int8 {
            fields.extend([
                Field::new(
                    "vector_int8",
                    DataType::FixedSizeBinary(dimension as i32),
                    true,
                ),
                Field::new("int8_scale", DataType::Float32, true),
                Field::new("int8_zero_point", DataType::Int8, true),
            ]);
            debug!("Added INT8 quantization columns");
        }
        
        if config.enable_pq {
            fields.extend([
                Field::new(
                    "vector_pq",
                    DataType::FixedSizeBinary(config.pq_segments as i32),
                    true,
                ),
                Field::new("pq_codebook_id", DataType::Utf8, true),
            ]);
            debug!("Added PQ quantization columns");
        }
        
        // Add filterable metadata columns
        for column in filterable_columns {
            let field = self.create_filterable_field(column)?;
            fields.push(field);
            debug!("Added filterable column: {} ({})", column.name, column.data_type);
        }
        
        // Add metadata storage for non-filterable fields
        fields.push(Field::new(
            "extra_metadata_info",
            DataType::List(Arc::new(Field::new(
                "item",
                DataType::Struct(vec![
                    Field::new("key", DataType::Utf8, false),
                    Field::new("value", DataType::Utf8, true),
                ].into()),
                false,
            ))),
            true,
        ));
        
        Ok(Arc::new(Schema::new(fields)))
    }
    
    /// Get appropriate data type for vector storage
    fn get_vector_data_type(&self, dimension: usize) -> DataType {
        // Use FixedSizeBinary for efficient storage and SIMD operations
        DataType::FixedSizeBinary(dimension as i32 * 4) // 4 bytes per float32
    }
    
    /// Create field for filterable column
    fn create_filterable_field(&self, column: &FilterableColumn) -> Result<Field> {
        let data_type = match column.data_type.as_str() {
            "string" | "text" => DataType::Utf8,
            "int" | "integer" | "long" => DataType::Int64,
            "float" | "double" => DataType::Float64,
            "bool" | "boolean" => DataType::Boolean,
            "date" => DataType::Date32,
            "datetime" | "timestamp" => DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
            "list" => DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            _ => {
                debug!("Unknown data type '{}', defaulting to Utf8", column.data_type);
                DataType::Utf8
            }
        };
        
        Ok(Field::new(&column.name, data_type, column.nullable))
    }
    
    /// Validate schema compatibility
    pub async fn validate_schema_compatibility(
        &self,
        existing_schema: &Schema,
        new_schema: &Schema,
    ) -> Result<SchemaCompatibility> {
        info!("Validating schema compatibility");
        
        let mut compatibility = SchemaCompatibility {
            is_compatible: true,
            added_fields: Vec::new(),
            removed_fields: Vec::new(),
            changed_fields: Vec::new(),
            breaking_changes: Vec::new(),
        };
        
        // Check for removed fields
        for existing_field in existing_schema.fields() {
            if new_schema.field_with_name(existing_field.name()).is_err() {
                compatibility.removed_fields.push(existing_field.name().clone());
                
                // Removing non-nullable fields is a breaking change
                if !existing_field.is_nullable() {
                    compatibility.breaking_changes.push(format!(
                        "Removed non-nullable field: {}",
                        existing_field.name()
                    ));
                    compatibility.is_compatible = false;
                }
            }
        }
        
        // Check for added fields
        for new_field in new_schema.fields() {
            if existing_schema.field_with_name(new_field.name()).is_err() {
                compatibility.added_fields.push(new_field.name().clone());
                
                // Adding non-nullable fields without defaults is a breaking change
                if !new_field.is_nullable() {
                    compatibility.breaking_changes.push(format!(
                        "Added non-nullable field: {}",
                        new_field.name()
                    ));
                    compatibility.is_compatible = false;
                }
            }
        }
        
        // Check for changed fields
        for existing_field in existing_schema.fields() {
            if let Ok(new_field) = new_schema.field_with_name(existing_field.name()) {
                if existing_field.data_type() != new_field.data_type() {
                    let change = format!(
                        "{}: {:?} -> {:?}",
                        existing_field.name(),
                        existing_field.data_type(),
                        new_field.data_type()
                    );
                    compatibility.changed_fields.push(change.clone());
                    
                    // Type changes are generally breaking
                    if !self.is_compatible_type_change(existing_field.data_type(), new_field.data_type()) {
                        compatibility.breaking_changes.push(format!("Incompatible type change: {}", change));
                        compatibility.is_compatible = false;
                    }
                }
                
                // Nullability changes
                if existing_field.is_nullable() && !new_field.is_nullable() {
                    compatibility.breaking_changes.push(format!(
                        "Changed nullable field to non-nullable: {}",
                        existing_field.name()
                    ));
                    compatibility.is_compatible = false;
                }
            }
        }
        
        Ok(compatibility)
    }
    
    /// Check if type change is compatible
    fn is_compatible_type_change(&self, old_type: &DataType, new_type: &DataType) -> bool {
        match (old_type, new_type) {
            // Widening numeric types is safe
            (DataType::Int32, DataType::Int64) => true,
            (DataType::Float32, DataType::Float64) => true,
            
            // String types are generally compatible
            (DataType::Utf8, DataType::LargeUtf8) => true,
            (DataType::LargeUtf8, DataType::Utf8) => false, // Narrowing is not safe
            
            // Same types are compatible
            (a, b) if a == b => true,
            
            // Everything else is incompatible
            _ => false,
        }
    }
    
    /// Generate cache key for schema
    fn generate_cache_key(
        &self,
        collection_id: &str,
        dimension: usize,
        quantization: Option<&QuantizationConfig>,
        filterable_columns: &[FilterableColumn],
    ) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        dimension.hash(&mut hasher);
        
        if let Some(config) = quantization {
            config.enable_binary.hash(&mut hasher);
            config.enable_int8.hash(&mut hasher);
            config.enable_pq.hash(&mut hasher);
            config.pq_segments.hash(&mut hasher);
            config.pq_bits.hash(&mut hasher);
        }
        
        for column in filterable_columns {
            column.name.hash(&mut hasher);
            column.data_type.hash(&mut hasher);
            column.nullable.hash(&mut hasher);
        }
        
        format!("schema_{:x}", hasher.finish())
    }
    
    /// Get cached schema
    async fn get_cached_schema(&self, cache_key: &str) -> Option<CachedSchema> {
        let cache = self.schema_cache.read().await;
        cache.get(cache_key).cloned()
    }
    
    /// Cache schema
    async fn cache_schema(&self, cache_key: String, schema: Arc<Schema>) {
        let cached = CachedSchema {
            schema,
            timestamp: chrono::Utc::now(),
            ttl_seconds: 3600, // 1 hour TTL
        };
        
        let mut cache = self.schema_cache.write().await;
        cache.insert(cache_key, cached);
        
        // Simple cache eviction (keep last 50 schemas)
        if cache.len() > 50 {
            let oldest_key = cache.keys().next().cloned();
            if let Some(key) = oldest_key {
                cache.remove(&key);
            }
        }
    }
    
    /// Clear schema cache
    pub async fn clear_cache(&self) {
        let mut cache = self.schema_cache.write().await;
        cache.clear();
        info!("Cleared schema cache_info");
    }
    
    /// Get cache statistics
    pub async fn get_cache_stats(&self) -> SchemaCacheStats {
        let cache = self.schema_cache.read().await;
        
        SchemaCacheStats {
            entry_count: cache.len(),
            oldest_entry: cache.values()
                .map(|v| v.timestamp)
                .min(),
        }
    }
    
    /// Create schema from file metadata
    pub async fn create_schema_from_metadata(
        &self,
        metadata: &ColumnarFileMetadata,
        filterable_columns: &[FilterableColumn],
    ) -> Result<Arc<Schema>> {
        self.create_vector_schema(
            &metadata.collection_id,
            metadata.dimension,
            Some(&metadata.quantization),
            filterable_columns,
        ).await
    }
    
    /// Evolve schema for new requirements
    pub async fn evolve_schema(
        &self,
        existing_schema: &Schema,
        new_requirements: &SchemaEvolutionRequest,
    ) -> Result<Arc<Schema>> {
        info!("Evolving schema for new requirements");
        
        let mut fields: Vec<Arc<Field>> = existing_schema.fields().iter().cloned().collect();
        
        // Add new quantization columns if requested
        if let Some(ref quant_config) = new_requirements.new_quantization {
            if quant_config.enable_binary && existing_schema.field_with_name("vector_binary").is_err() {
                fields.push(Arc::new(Field::new(
                    "vector_binary",
                    DataType::FixedSizeBinary(((new_requirements.dimension + 7) / 8) as i32),
                    true,
                )));
            }
            
            if quant_config.enable_int8 && existing_schema.field_with_name("vector_int8").is_err() {
                fields.extend([
                    Arc::new(Field::new(
                        "vector_int8",
                        DataType::FixedSizeBinary(new_requirements.dimension as i32),
                        true,
                    )),
                    Arc::new(Field::new("int8_scale", DataType::Float32, true)),
                    Arc::new(Field::new("int8_zero_point", DataType::Int8, true)),
                ]);
            }
            
            if quant_config.enable_pq && existing_schema.field_with_name("vector_pq").is_err() {
                fields.extend([
                    Arc::new(Field::new(
                        "vector_pq",
                        DataType::FixedSizeBinary(quant_config.pq_segments as i32),
                        true,
                    )),
                    Arc::new(Field::new("pq_codebook_id", DataType::Utf8, true)),
                ]);
            }
        }
        
        // Add new filterable columns
        for column in &new_requirements.new_filterable_columns {
            if existing_schema.field_with_name(&column.name).is_err() {
                let field = self.create_filterable_field(column)?;
                fields.push(Arc::new(field));
            }
        }
        
        Ok(Arc::new(Schema::new(fields)))
    }
}

/// Filterable column specification
#[derive(Debug, Clone, Hash)]
pub struct FilterableColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub indexed: bool,
}

/// Cached schema with TTL
#[derive(Debug, Clone)]
struct CachedSchema {
    pub schema: Arc<Schema>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub ttl_seconds: i64,
}

impl CachedSchema {
    fn is_expired(&self) -> bool {
        let now = chrono::Utc::now();
        let age = now.signed_duration_since(self.timestamp);
        age.num_seconds() > self.ttl_seconds
    }
}

/// Schema compatibility result
#[derive(Debug)]
pub struct SchemaCompatibility {
    pub is_compatible: bool,
    pub added_fields: Vec<String>,
    pub removed_fields: Vec<String>,
    pub changed_fields: Vec<String>,
    pub breaking_changes: Vec<String>,
}

/// Schema cache statistics
#[derive(Debug)]
pub struct SchemaCacheStats {
    pub entry_count: usize,
    pub oldest_entry: Option<chrono::DateTime<chrono::Utc>>,
}

/// Schema evolution request
#[derive(Debug)]
pub struct SchemaEvolutionRequest {
    pub dimension: usize,
    pub new_quantization: Option<QuantizationConfig>,
    pub new_filterable_columns: Vec<FilterableColumn>,
}

impl Default for ColumnarSchema {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_schema_creation() {
        let manager = ColumnarSchema::new();
        
        let filterable_columns = vec![
            FilterableColumn {
                name: "category".to_string(),
                // data_type removed -  "string".to_string(),
                nullable: true,
                indexed: true,
            },
            FilterableColumn {
                name: "price".to_string(),
                // data_type removed -  "float".to_string(),
                nullable: true,
                indexed: false,
            },
        ];
        
        let schema = manager.create_vector_schema(
            "test_collection",
            768,
            None,
            &filterable_columns,
        ).await.unwrap();
        
        // Check core fields
        assert!(schema.field_with_name("id").is_ok());
        assert!(schema.field_with_name("vector").is_ok());
        assert!(schema.field_with_name("timestamp").is_ok());
        
        // Check filterable fields
        assert!(schema.field_with_name("category").is_ok());
        assert!(schema.field_with_name("price").is_ok());
        
        // Check metadata field
        assert!(schema.field_with_name("extra_metadata_info").is_ok());
    }
    
    #[tokio::test]
    async fn test_quantization_schema() {
        let manager = ColumnarSchema::new();
        
        let config = QuantizationConfig {
            enable_binary: true,
            enable_int8: true,
            enable_pq: true,
            pq_segments: 16,
            pq_bits: 8,
            ..Default::default()
        };
        
        let schema = manager.create_vector_schema(
            "test_collection",
            768,
            Some(&config),
            &[],
        ).await.unwrap();
        
        // Check quantization fields
        assert!(schema.field_with_name("vector_binary").is_ok());
        assert!(schema.field_with_name("vector_int8").is_ok());
        assert!(schema.field_with_name("int8_scale").is_ok());
        assert!(schema.field_with_name("int8_zero_point").is_ok());
        assert!(schema.field_with_name("vector_pq").is_ok());
        assert!(schema.field_with_name("pq_codebook_id").is_ok());
    }
    
    #[tokio::test]
    async fn test_schema_compatibility() {
        let manager = ColumnarSchema::new();
        
        // Create original schema
        let original_schema = manager.create_vector_schema(
            "test_collection",
            768,
            None,
            &[FilterableColumn {
                name: "category".to_string(),
                // data_type removed -  "string".to_string(),
                nullable: true,
                indexed: true,
            }],
        ).await.unwrap();
        
        // Create evolved schema with additional field
        let evolved_schema = manager.create_vector_schema(
            "test_collection",
            768,
            None,
            &[
                FilterableColumn {
                    name: "category".to_string(),
                    // data_type removed -  "string".to_string(),
                    nullable: true,
                    indexed: true,
                },
                FilterableColumn {
                    name: "price".to_string(),
                    // data_type removed -  "float".to_string(),
                    nullable: true,
                    indexed: false,
                },
            ],
        ).await.unwrap();
        
        let compatibility = manager.validate_schema_compatibility(&original_schema, &evolved_schema).await.unwrap();
        
        assert!(compatibility.is_compatible);
        assert_eq!(compatibility.added_fields.len(), 1);
        assert_eq!(compatibility.added_fields[0], "price");
    }
}
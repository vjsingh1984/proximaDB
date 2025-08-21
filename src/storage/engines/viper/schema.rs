// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Schema Generation and Management
//!
//! This module handles dynamic Parquet schema generation based on collection configuration
//! and provides schema caching and evolution capabilities.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};


use super::types::{FilterableColumn, FilterableDataType};

/// Schema cache and management
#[derive(Debug)]
pub struct SchemaManager {
    /// Schema cache to avoid repeated collection service calls
    schema_cache: Arc<RwLock<HashMap<String, Arc<arrow_schema::Schema>>>>,
}

impl SchemaManager {
    pub fn new() -> Self {
        Self {
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or generate cached schema for a collection with pre-fetched config
    pub async fn get_or_generate_cached_schema(
        &self, 
        collection_id: &str, 
        collection_config: &Option<crate::proto::proximadb::Collection>
    ) -> Result<Arc<arrow_schema::Schema>> {
        // Check cache first
        {
            let cache = self.schema_cache.read().await;
            if let Some(cached_schema) = cache.get(collection_id) {
                debug!("📊 Using cached schema for collection {}", collection_id);
                return Ok(cached_schema.clone());
            }
        }
        
        // Generate new schema using provided config
        info!("🔧 Generating new schema for collection {}", collection_id);
        let schema = self.generate_dynamic_parquet_schema(collection_id, collection_config).await?;
        
        // Cache the schema
        {
            let mut cache = self.schema_cache.write().await;
            cache.insert(collection_id.to_string(), schema.clone());
        }
        
        info!("📊 Cached schema for collection {} ({} fields)", collection_id, schema.fields().len());
        Ok(schema)
    }

    /// Generate dynamic Parquet schema based on collection configuration
    async fn generate_dynamic_parquet_schema(
        &self, 
        collection_id: &str, 
        collection_config: &Option<crate::proto::proximadb::Collection>
    ) -> Result<Arc<arrow_schema::Schema>> {
        use arrow_schema::{DataType, Field, Fields, Schema};
        use std::sync::Arc;
        
        info!("🔧 Generating dynamic Parquet schema for collection {} with pre-fetched config", collection_id);
        
        let mut schema_fields = Vec::new();
        
        // Core fields - id can be null for immutable/append-only vectors
        schema_fields.push(Field::new("id", DataType::Utf8, true));
        // Collection ID is not stored - derived from directory structure
        
        // Vector field - native Parquet List<Float32> with nullable items for sparse vectors
        schema_fields.push(Field::new(
            "vector",
            DataType::List(Arc::new(Field::new("item", DataType::Float32, true))),
            true,  // Vector field itself can be nullable
        ));
        
        // Version field for MVCC support - using Int8 (tinyint) for efficiency
        schema_fields.push(Field::new("version", DataType::Int8, true));
        
        // Audit field - stores creation time initially, updated on modifications
        schema_fields.push(Field::new("updated_at", DataType::Int64, true));
        
        // Only include expires_at if TTL is enabled
        schema_fields.push(Field::new("expires_at", DataType::Int64, true)); // Nullable for TTL
        
        // Quantization fields (optional) - optimized for performance
        if let Some(ref collection) = collection_config {
            if let Some(quant_config) = collection.config.as_ref().and_then(|c| c.quantization.as_ref()) {
                // Use proto quantization config directly
                // Check quantization strategy from new proto structure
                let quant_type = match quant_config.strategy() {
                    crate::proto::proximadb::quantization_config::Strategy::SmartDefaults => "pq", // Default to PQ for smart defaults
                    crate::proto::proximadb::quantization_config::Strategy::CustomLevels => "pq", // Assume PQ for custom levels
                    crate::proto::proximadb::quantization_config::Strategy::Minimal => "sq", // Minimal uses scalar
                    crate::proto::proximadb::quantization_config::Strategy::Aggressive => "binary", // Aggressive uses binary
                };
                
                match quant_type {
                    "pq" | "pq4" | "pq8" => {
                        // Product Quantization - use FixedSizeBinary for better performance
                        // Default to 16 subvectors with 8 bits each for PQ
                        let (subvectors, bits) = (16u32, 8u32);
                        let pq_size = subvectors * (bits / 8);
                        
                        schema_fields.push(Field::new(
                            "vector_pq",
                            DataType::FixedSizeBinary(pq_size),
                            true, // Nullable for progressive rollout
                        ));
                    },
                    "sq" => {
                        // Scalar Quantization
                        schema_fields.push(Field::new(
                            "vector_sq",
                            DataType::List(Arc::new(Field::new("item", DataType::Int8, false))),
                            true,
                        ));
                        schema_fields.push(Field::new("sq_scale", DataType::Float32, true));
                        schema_fields.push(Field::new("sq_offset", DataType::Float32, true));
                    },
                    "binary" => {
                        // Binary Quantization
                        let dimension = collection.config.as_ref().map(|c| c.dimension);
                        let binary_size = (dimension + 7) / 8; // Bits to bytes
                        schema_fields.push(Field::new(
                            "vector_binary",
                            DataType::FixedSizeBinary(binary_size),
                            true,
                        ));
                    },
                    _ => {
                        // Default to PQ8 format for unknown types
                        schema_fields.push(Field::new(
                            "vector_quantized",
                            DataType::Binary,
                            true,
                        ));
                    }
                }
                
                // Quantization metadata stored once per row group, not per row
                // This will be stored in Parquet metadata instead
            }
        }
        
        // Filterable metadata columns as native Parquet columns - use proto definition directly
        if let Some(ref collection) = collection_config {
            if let Some(ref config) = collection.config {
                // Pre-allocate capacity for better performance
                schema_fields.reserve(config.filterable_columns.len());
                
                for filterable_column in &config.filterable_columns {
                    let field_type = self.convert_proto_type_to_arrow(filterable_column.data_type)?;
                    schema_fields.push(Field::new(
                        &filterable_column.name,
                        field_type,
                        true, // Filterable metadata is always nullable
                    ));
                }
            }
        }
        
        // Extra metadata as list of key-value pairs (for non-filterable fields)
        // Each element is a struct with "key" and "value" fields
        let key_value_struct = DataType::Struct(Fields::from(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ]));
        schema_fields.push(Field::new(
            "extra_meta", 
            DataType::List(Arc::new(Field::new("item", key_value_struct, true))),  // Item should be nullable for empty metadata
            true
        ));
        
        let schema = Schema::new(schema_fields);
        
        info!("✅ Generated dynamic Parquet schema for collection {} with {} fields", 
              collection_id, schema.fields().len());
        
        Ok(Arc::new(schema))
    }

    /// Convert proto FilterableDataType to Arrow DataType
    pub fn convert_proto_type_to_arrow(&self, data_type: i32) -> Result<arrow_schema::DataType> {
        use crate::proto::proximadb::FilterableDataType;
        use arrow_schema::DataType;
        
        match FilterableDataType::try_from(data_type) {
            Ok(FilterableDataType::FilterableString) => Ok(DataType::Utf8),
            Ok(FilterableDataType::FilterableInteger) => Ok(DataType::Int64),
            Ok(FilterableDataType::FilterableFloat) => Ok(DataType::Float64),
            Ok(FilterableDataType::FilterableBoolean) => Ok(DataType::Boolean),
            Ok(FilterableDataType::FilterableDatetime) => Ok(DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None)),
            Ok(FilterableDataType::FilterableArrayString) => Ok(DataType::List(Arc::new(arrow_schema::Field::new("item", DataType::Utf8, false)))),
            Ok(FilterableDataType::FilterableArrayInteger) => Ok(DataType::List(Arc::new(arrow_schema::Field::new("item", DataType::Int64, false)))),
            Ok(FilterableDataType::FilterableArrayFloat) => Ok(DataType::List(Arc::new(arrow_schema::Field::new("item", DataType::Float64, false)))),
            _ => Ok(DataType::Utf8), // Default to string
        }
    }
    
    /// Parse filterable columns from collection config JSON
    /// Kept for backward compatibility
    #[allow(dead_code)]
    pub fn parse_filterable_columns(&self, config: &str) -> Result<Vec<FilterableColumn>> {
        let config: serde_json::Value = serde_json::from_str(config)
            .unwrap_or_else(|_| serde_json::json!({}));
        
        let columns = if let Some(cols) = config.get("columns").and_then(|v| v.as_array()) {
            cols.iter()
                .filter_map(|col| {
                    if let (Some(name), Some(data_type_str)) = (
                        col.get("name").and_then(|v| v.as_deref()),
                        col.get("type").and_then(|v| v.as_deref())
                    ) {
                        let data_type = match data_type_str {
                            "string" => FilterableDataType::String,
                            "integer" => FilterableDataType::Integer,
                            "float" => FilterableDataType::Float,
                            "boolean" => FilterableDataType::Boolean,
                            "datetime" => FilterableDataType::DateTime,
                            "array_string" => FilterableDataType::Array(Box::new(FilterableDataType::String)),
                            "array_integer" => FilterableDataType::Array(Box::new(FilterableDataType::Integer)),
                            _ => FilterableDataType::String,
                        };
                        Some(FilterableColumn {
                            name: name.to_string(),
                            data_type,
                            indexed: col.get("indexed").and_then(|v| v.as_bool()),
                            supports_range: col.get("supports_range").and_then(|v| v.as_bool()),
                            estimated_cardinality: col.get("estimated_cardinality").and_then(|v| v.as_u64()).map(|v| v as usize),
                        })
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        
        Ok(columns)
    }

    /// Convert FilterableDataType to Arrow DataType
    fn convert_filterable_type_to_arrow(&self, data_type: &FilterableDataType) -> Result<arrow_schema::DataType> {
        use arrow_schema::{DataType, Field, TimeUnit};
        
        let arrow_type = match data_type {
            FilterableDataType::String => DataType::Utf8,
            FilterableDataType::Integer => DataType::Int64,
            FilterableDataType::Float => DataType::Float64,
            FilterableDataType::Boolean => DataType::Boolean,
            FilterableDataType::DateTime => DataType::Timestamp(TimeUnit::Millisecond, None),
            FilterableDataType::Array(inner_type) => {
                let inner_arrow_type = self.convert_filterable_type_to_arrow(inner_type)?;
                DataType::List(Arc::new(Field::new("item", inner_arrow_type, false)))
            }
        };
        
        Ok(arrow_type)
    }

    /// Clear schema cache for a collection
    pub async fn clear_schema_cache(&self, collection_id: &str) {
        let mut cache = self.schema_cache.write().await;
        cache.remove(collection_id);
        info!("🗑️ Cleared schema cache for collection {}", collection_id);
    }

    /// Clear all schema caches
    pub async fn clear_all_schema_cache(&self) {
        let mut cache = self.schema_cache.write().await;
        cache.clear();
        info!("🗑️ Cleared all schema caches");
    }

    /// Get schema cache statistics
    pub async fn get_cache_stats(&self) -> (usize, Vec<String>) {
        let cache = self.schema_cache.read().await;
        let cached_collections: Vec<String> = cache.keys().cloned().collect();
        (cache.len(), cached_collections)
    }
}

impl Default for SchemaManager {
    fn default() -> Self {
        Self::new()
    }
}
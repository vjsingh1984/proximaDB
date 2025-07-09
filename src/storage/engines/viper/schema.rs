// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Schema Generation and Management
//!
//! This module handles dynamic Parquet schema generation based on collection configuration
//! and provides schema caching and evolution capabilities.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::core::CollectionId;
use super::types::{FilterableColumn, FilterableDataType};

/// Schema cache and management
#[derive(Debug)]
pub struct SchemaManager {
    /// Schema cache to avoid repeated collection service calls
    schema_cache: Arc<RwLock<HashMap<CollectionId, Arc<arrow_schema::Schema>>>>,
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
        collection_id: &CollectionId, 
        collection_config: &Option<crate::core::Collection>
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
            cache.insert(collection_id.clone(), schema.clone());
        }
        
        info!("📊 Cached schema for collection {} ({} fields)", collection_id, schema.fields().len());
        Ok(schema)
    }

    /// Generate dynamic Parquet schema based on collection configuration
    async fn generate_dynamic_parquet_schema(
        &self, 
        collection_id: &CollectionId, 
        collection_config: &Option<crate::core::Collection>
    ) -> Result<Arc<arrow_schema::Schema>> {
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;
        
        info!("🔧 Generating dynamic Parquet schema for collection {} with pre-fetched config", collection_id);
        
        let mut schema_fields = Vec::new();
        
        // Core fields (always present)
        schema_fields.push(Field::new("id", DataType::Utf8, false));
        schema_fields.push(Field::new("collection_id", DataType::Utf8, false));
        
        // Vector field - native Parquet List<Float32>
        schema_fields.push(Field::new(
            "vector",
            DataType::List(Arc::new(Field::new("item", DataType::Float32, false))),
            false,
        ));
        
        // MVCC fields
        schema_fields.push(Field::new("timestamp", DataType::Int64, false));
        schema_fields.push(Field::new("created_at", DataType::Int64, false));
        schema_fields.push(Field::new("updated_at", DataType::Int64, false));
        schema_fields.push(Field::new("version", DataType::Int64, false));
        schema_fields.push(Field::new("expires_at", DataType::Int64, true)); // Nullable for TTL
        
        // Quantization fields (optional)
        if collection_config.as_ref().map_or(false, |c| c.config.contains_key("quantization")) {
            schema_fields.push(Field::new(
                "vector_pq",
                DataType::List(Arc::new(Field::new("item", DataType::UInt8, false))),
                true, // Nullable - may not be present for all records
            ));
            schema_fields.push(Field::new("pq_centroids", DataType::Binary, true));
        }
        
        // Filterable metadata columns as native Parquet columns
        if let Some(ref collection) = collection_config {
            // Convert config HashMap to JSON string for parsing
            let config_json = serde_json::to_string(&collection.config)
                .context("Failed to serialize collection config")?;
            let filterable_columns = self.parse_filterable_columns(&config_json)?;
            for column in filterable_columns {
                let field_type = self.convert_filterable_type_to_arrow(&column.data_type)?;
                schema_fields.push(Field::new(
                    &column.name,
                    field_type,
                    true, // Filterable metadata is always nullable
                ));
            }
        }
        
        // Extra metadata as JSON string (for non-filterable fields)
        schema_fields.push(Field::new("extra_meta", DataType::Utf8, true));
        
        let schema = Schema::new(schema_fields);
        
        info!("✅ Generated dynamic Parquet schema for collection {} with {} fields", 
              collection_id, schema.fields().len());
        
        Ok(Arc::new(schema))
    }

    /// Parse filterable columns from collection config JSON
    pub fn parse_filterable_columns(&self, config: &str) -> Result<Vec<FilterableColumn>> {
        let config: serde_json::Value = serde_json::from_str(config)
            .unwrap_or_else(|_| serde_json::json!({}));
        
        let columns = if let Some(cols) = config.get("filterable_columns").and_then(|v| v.as_array()) {
            cols.iter()
                .filter_map(|col| {
                    if let (Some(name), Some(data_type_str)) = (
                        col.get("name").and_then(|v| v.as_str()),
                        col.get("data_type").and_then(|v| v.as_str())
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
                            indexed: col.get("indexed").and_then(|v| v.as_bool()).unwrap_or(false),
                            supports_range: col.get("supports_range").and_then(|v| v.as_bool()).unwrap_or(false),
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
    pub async fn clear_schema_cache(&self, collection_id: &CollectionId) {
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
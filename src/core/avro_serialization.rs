//! Binary Avro Serialization Helper Module
//! 
//! Provides high-performance binary Avro serialization for ProximaDB responses
//! Replaces JSON serialization for better performance and type safety

use anyhow::{Context, Result};
use apache_avro::{Schema, Writer, to_avro_datum, from_avro_datum};
use serde::{Serialize, Deserialize};
use std::io::Cursor;

use super::avro_unified::*;

/// Binary Avro serializer for ProximaDB responses
pub struct AvroSerializer {
    /// Cached schemas for different response types
    vector_search_schema: Schema,
    collection_response_schema: Schema,
    health_response_schema: Schema,
    metrics_response_schema: Schema,
    operation_response_schema: Schema,
}

impl AvroSerializer {
    /// Create a new Avro serializer with pre-compiled schemas
    pub fn new() -> Result<Self> {
        // Define schemas for common response types
        let vector_search_schema = Schema::parse_str(r#"
        {
            "type": "record",
            "name": "VectorSearchResponse",
            "fields": [
                {"name": "success", "type": "boolean"},
                {"name": "results", "type": {"type": "array", "items": {
                    "type": "record",
                    "name": "SearchResult",
                    "fields": [
                        {"name": "id", "type": "string"},
                        {"name": "vector_id", "type": ["null", "string"], "default": null},
                        {"name": "score", "type": "float"},
                        {"name": "distance", "type": ["null", "float"], "default": null},
                        {"name": "rank", "type": ["null", "int"], "default": null},
                        {"name": "vector", "type": ["null", {"type": "array", "items": "float"}], "default": null},
                        {"name": "metadata", "type": ["null", {"type": "map", "values": "string"}], "default": null},
                        {"name": "collection_id", "type": ["null", "string"], "default": null},
                        {"name": "created_at", "type": ["null", "long"], "default": null},
                        {"name": "algorithm_used", "type": ["null", "string"], "default": null},
                        {"name": "processing_time_us", "type": ["null", "long"], "default": null}
                    ]
                }}},
                {"name": "total_count", "type": "long"},
                {"name": "total_found", "type": "long"},
                {"name": "processing_time_us", "type": "long"},
                {"name": "algorithm_used", "type": "string"},
                {"name": "error_message", "type": ["null", "string"], "default": null}
            ]
        }
        "#)?;

        let collection_response_schema = Schema::parse_str(r#"
        {
            "type": "record",
            "name": "CollectionResponse",
            "fields": [
                {"name": "success", "type": "boolean"},
                {"name": "operation", "type": {"type": "enum", "name": "CollectionOperation", "symbols": ["CREATE", "UPDATE", "GET", "LIST", "DELETE", "MIGRATE"]}},
                {"name": "collection", "type": ["null", {
                    "type": "record",
                    "name": "Collection",
                    "fields": [
                        {"name": "id", "type": "string"},
                        {"name": "name", "type": "string"},
                        {"name": "dimension", "type": "int"},
                        {"name": "distance_metric", "type": "string"},
                        {"name": "indexing_algorithm", "type": "string"},
                        {"name": "storage_engine", "type": "string"},
                        {"name": "vector_count", "type": "long"},
                        {"name": "created_at", "type": "long"},
                        {"name": "updated_at", "type": "long"},
                        {"name": "status", "type": "string"}
                    ]
                }], "default": null},
                {"name": "collections", "type": {"type": "array", "items": "Collection"}},
                {"name": "affected_count", "type": "long"},
                {"name": "total_count", "type": ["null", "long"], "default": null},
                {"name": "metadata", "type": {"type": "map", "values": "string"}},
                {"name": "error_message", "type": ["null", "string"], "default": null},
                {"name": "error_code", "type": ["null", "string"], "default": null},
                {"name": "processing_time_us", "type": "long"}
            ]
        }
        "#)?;

        let health_response_schema = Schema::parse_str(r#"
        {
            "type": "record",
            "name": "HealthResponse",
            "fields": [
                {"name": "status", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "uptime_seconds", "type": "long"},
                {"name": "total_operations", "type": "long"},
                {"name": "successful_operations", "type": "long"},
                {"name": "failed_operations", "type": "long"},
                {"name": "avg_processing_time_us", "type": "double"},
                {"name": "storage_healthy", "type": "boolean"},
                {"name": "wal_healthy", "type": "boolean"},
                {"name": "timestamp", "type": "long"}
            ]
        }
        "#)?;

        let metrics_response_schema = Schema::parse_str(r#"
        {
            "type": "record",
            "name": "MetricsResponse",
            "fields": [
                {"name": "service_metrics", "type": {
                    "type": "record",
                    "name": "ServiceMetrics",
                    "fields": [
                        {"name": "total_operations", "type": "long"},
                        {"name": "successful_operations", "type": "long"},
                        {"name": "failed_operations", "type": "long"},
                        {"name": "avg_processing_time_us", "type": "double"},
                        {"name": "last_operation_time", "type": ["null", "long"], "default": null}
                    ]
                }},
                {"name": "wal_metrics", "type": {
                    "type": "record",
                    "name": "WalMetrics",
                    "fields": [
                        {"name": "total_entries", "type": "long"},
                        {"name": "memory_entries", "type": "long"},
                        {"name": "disk_segments", "type": "long"},
                        {"name": "total_disk_size_bytes", "type": "long"},
                        {"name": "compression_ratio", "type": "double"}
                    ]
                }},
                {"name": "timestamp", "type": "long"}
            ]
        }
        "#)?;

        let operation_response_schema = Schema::parse_str(r#"
        {
            "type": "record",
            "name": "OperationResponse",
            "fields": [
                {"name": "success", "type": "boolean"},
                {"name": "error_message", "type": ["null", "string"], "default": null},
                {"name": "error_code", "type": ["null", "string"], "default": null},
                {"name": "affected_count", "type": "long"},
                {"name": "processing_time_us", "type": "long"},
                {"name": "metadata", "type": {"type": "map", "values": "string"}}
            ]
        }
        "#)?;

        Ok(Self {
            vector_search_schema,
            collection_response_schema,
            health_response_schema,
            metrics_response_schema,
            operation_response_schema,
        })
    }

    /// Serialize VectorSearchResponse to binary Avro (fallback to JSON for now)
    pub fn serialize_search_response(&self, response: &VectorSearchResponse) -> Result<Vec<u8>> {
        // TODO: Implement proper binary Avro serialization once all response types are converted
        // For now, use JSON as intermediate step to maintain compatibility
        serde_json::to_vec(response)
            .context("Failed to serialize search response as JSON (Avro conversion pending)")
    }

    /// Serialize CollectionResponse to binary Avro (fallback to JSON for now)
    pub fn serialize_collection_response(&self, response: &CollectionResponse) -> Result<Vec<u8>> {
        // TODO: Implement proper binary Avro serialization
        serde_json::to_vec(response)
            .context("Failed to serialize collection response as JSON (Avro conversion pending)")
    }

    /// Serialize HealthResponse to binary Avro (fallback to JSON for now)
    pub fn serialize_health_response(&self, response: &HealthResponse) -> Result<Vec<u8>> {
        // TODO: Implement proper binary Avro serialization
        serde_json::to_vec(response)
            .context("Failed to serialize health response as JSON (Avro conversion pending)")
    }

    /// Serialize MetricsResponse to binary Avro (fallback to JSON for now)
    pub fn serialize_metrics_response(&self, response: &MetricsResponse) -> Result<Vec<u8>> {
        // TODO: Implement proper binary Avro serialization
        serde_json::to_vec(response)
            .context("Failed to serialize metrics response as JSON (Avro conversion pending)")
    }

    /// Serialize OperationResponse to binary Avro (fallback to JSON for now)
    pub fn serialize_operation_response(&self, response: &OperationResponse) -> Result<Vec<u8>> {
        // TODO: Implement proper binary Avro serialization
        serde_json::to_vec(response)
            .context("Failed to serialize operation response as JSON (Avro conversion pending)")
    }

    /// Deserialize binary Avro to VectorSearchRequest
    pub fn deserialize_search_request(&self, avro_bytes: &[u8]) -> Result<VectorSearchRequest> {
        // For now, assume JSON format and convert - TODO: implement proper Avro deserialization
        let json_value: serde_json::Value = serde_json::from_slice(avro_bytes)
            .context("Failed to parse search request as JSON")?;
        
        let collection_id = json_value.get("collection_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing collection_id"))?
            .to_string();
            
        let query_vector = json_value.get("vector")
            .and_then(|v| v.as_array())
            .context("Missing or invalid vector field")?
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect();
            
        let k = json_value.get("k")
            .and_then(|v| v.as_i64())
            .unwrap_or(10) as i32;
            
        let metadata_filter = json_value.get("metadata_filter")
            .and_then(|v| v.as_object())
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
            
        let include_vector = json_value.get("include_vector")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
            
        let include_metadata = json_value.get("include_metadata")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        Ok(VectorSearchRequest {
            collection_id,
            query_vector,
            k,
            metadata_filter,
            include_vector,
            include_metadata,
        })
    }
}

/// Global Avro serializer instance
static AVRO_SERIALIZER: once_cell::sync::Lazy<AvroSerializer> = 
    once_cell::sync::Lazy::new(|| AvroSerializer::new().expect("Failed to initialize Avro serializer"));

/// Get the global Avro serializer instance
pub fn get_avro_serializer() -> &'static AvroSerializer {
    &AVRO_SERIALIZER
}
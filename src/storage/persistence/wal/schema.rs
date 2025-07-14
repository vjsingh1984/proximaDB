// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Modern WAL Schema Module - Batch-Oriented Operations Only
//!
//! This module provides schema definitions and serialization functions for
//! modern batch-oriented WAL operations. Legacy individual-entry functions
//! have been removed in favor of batch operations for better performance.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use prost::Message;

use crate::core::VectorRecord;

/// ULTRA-FRUGAL vector batch schema - optimized for minimal memory/disk footprint
/// Uses smaller data types and optional fields to reduce serialization overhead
pub const VECTOR_BATCH_SCHEMA_V1: &str = r#"
{
  "type": "record",
  "name": "WalVectorBatch",
  "namespace": "ai.proximadb.wal",
  "fields": [
    {"name": "vectors", "type": {
      "type": "array", 
      "items": {
        "type": "record",
        "name": "VectorRecord", 
        "fields": [
          {"name": "id", "type": ["null", "string"], "default": null},
          {"name": "collection_id", "type": "string"},
          {"name": "vector", "type": {"type": "array", "items": ["null", "float"]}},
          {"name": "metadata", "type": ["null", {"type": "map", "values": "string"}], "default": null},
          {"name": "timestamp", "type": "int"},
          {"name": "expires_at", "type": ["null", "int"], "default": null},
          {"name": "version", "type": "int"}
        ]
      }
    }}
  ]
}
"#;

/// Avro representation of a single vector - ULTRA-FRUGAL design for minimal footprint
/// Optimized for memory and disk efficiency with optional fields and smaller data types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroVector {
    /// Optional ID - vectors without ID are immutable and similarity-search only
    /// Client/SDK has ownership to populate ID for vectors that need get/delete/upsert operations
    pub id: Option<String>,
    pub collection_id: String,
    pub vector: Vec<f32>,
    /// Optional metadata - no cost when None, but likely present most times
    pub metadata: Option<HashMap<String, String>>,
    /// Coarse timestamp precision (seconds since epoch) - much smaller than microseconds
    pub timestamp: i32,
    /// ESSENTIAL but OPTIONAL: Only serialize when record has TTL/delete - no cost when None
    pub expires_at: Option<i32>,
    /// ESSENTIAL but SMALL: Use i32 for version (Avro int type) - still much smaller than i64
    pub version: i32,
}

/// Avro representation of a vector batch for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroVectorBatch {
    pub vectors: Vec<AvroVector>,
}

/// Convert VectorRecord to AvroVector
impl From<&VectorRecord> for AvroVector {
    fn from(record: &VectorRecord) -> Self {
        // Convert metadata from Vec<MetadataItem> to HashMap<String, String>
        let metadata = if record.metadata.is_empty() {
            None
        } else {
            Some(
                record
                    .metadata
                    .iter()
                    .map(|item| (item.key.clone(), item.value.clone()))
                    .collect(),
            )
        };

        Self {
            id: if record.id.as_deref().unwrap_or("").is_empty() { None } else { Some(record.id.as_deref().unwrap_or("").to_string()) },
            collection_id: record.collection_id.clone(),
            vector: record.vector.clone(),
            metadata,
            // Convert microsecond timestamp to seconds (much smaller)
            timestamp: (record.timestamp / 1_000_000) as i32,
            // Convert microsecond expires_at to seconds if present
            expires_at: record.expires_at.map(|exp| (exp / 1_000_000) as i32),
            version: record.version as i32,
        }
    }
}

/// Convert AvroVector back to VectorRecord
impl TryFrom<&AvroVector> for VectorRecord {
    type Error = anyhow::Error;
    
    fn try_from(avro: &AvroVector) -> Result<Self> {
        // Convert metadata from HashMap<String, String> to HashMap<String, serde_json::Value>
        let metadata = avro
            .metadata
            .as_ref()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect()
            })
            .unwrap_or_default();

        let timestamp_micros = (avro.timestamp as i64) * 1_000_000;
        
        // 🔧 FLEXIBLE: ID is optional - vectors without ID are immutable and similarity-search only
        // Client/SDK has ownership to populate ID for vectors that need get/delete/upsert operations
        let id = avro.id.clone().unwrap_or_default(); // Empty string for immutable vectors
        
        // In proto-first architecture, create proto VectorRecord directly
        use crate::core::proto_metadata_helper::json_metadata_to_proto;
        Ok(crate::core::VectorRecord {
            id: if id.is_empty() { None } else { Some(id) },
            collection_id: avro.collection_id.clone(),
            vector: avro.vector.clone(),
            metadata: json_metadata_to_proto(&metadata),
            // Convert seconds back to microseconds
            timestamp: timestamp_micros,
            created_at: timestamp_micros,
            updated_at: timestamp_micros,
            // Convert seconds back to microseconds if present
            expires_at: avro.expires_at.map(|exp| (exp as i64) * 1_000_000),
            version: avro.version as i64,
            rank: None,
            score: None,
            distance: None,
        })
    }
}

/// Create Avro vector batch from VectorRecord list (used by REST/gRPC handlers)
pub fn create_avro_vector_batch(vector_records: &[VectorRecord]) -> Result<Vec<u8>> {
    use apache_avro::Schema;

    let schema = Schema::parse_str(VECTOR_BATCH_SCHEMA_V1)
        .map_err(|e| anyhow::anyhow!("Failed to parse vector batch schema: {}", e))?;

    // Convert VectorRecord to AvroVector
    let avro_vectors: Vec<AvroVector> = vector_records.iter().map(AvroVector::from).collect();

    let batch = AvroVectorBatch {
        vectors: avro_vectors,
    };

    // Use apache_avro Writer for proper serialization
    use apache_avro::{Writer, types::Value};
    
    let mut writer = Writer::new(&schema, Vec::new());
    
    // Create the batch value with proper field ordering
    let vectors_array = Value::Array(
        batch.vectors.into_iter().map(|v| {
            let mut fields = vec![];
            
            // Add fields in the order they appear in the schema
            fields.push(("id".to_string(), match v.id {
                Some(id) => Value::Union(1, Box::new(Value::String(id))),
                None => Value::Union(0, Box::new(Value::Null)),
            }));
            
            fields.push(("collection_id".to_string(), Value::String(v.collection_id)));
            
            fields.push(("vector".to_string(), Value::Array(
                v.vector.into_iter().map(|f| {
                    // For sparse vectors, we could encode zero as null to save space
                    // For now, always use the float value (Union index 1)
                    Value::Union(1, Box::new(Value::Float(f)))
                }).collect()
            )));
            
            fields.push(("metadata".to_string(), match v.metadata {
                Some(meta) => Value::Union(1, Box::new(Value::Map(
                    meta.into_iter().map(|(k, v)| (k, Value::String(v))).collect()
                ))),
                None => Value::Union(0, Box::new(Value::Null)),
            }));
            
            fields.push(("timestamp".to_string(), Value::Int(v.timestamp)));
            
            fields.push(("expires_at".to_string(), match v.expires_at {
                Some(exp) => Value::Union(1, Box::new(Value::Int(exp))),
                None => Value::Union(0, Box::new(Value::Null)),
            }));
            
            fields.push(("version".to_string(), Value::Int(v.version)));
            
            Value::Record(fields)
        }).collect()
    );
    
    let batch_record = Value::Record(vec![
        ("vectors".to_string(), vectors_array)
    ]);
    
    writer.append(batch_record)
        .map_err(|e| anyhow::anyhow!("Failed to append vector batch: {}", e))?;
    
    let avro_bytes = writer.into_inner()
        .map_err(|e| anyhow::anyhow!("Failed to serialize vector batch: {}", e))?;

    Ok(avro_bytes)
}

/// Deserialize Avro vector batch to VectorRecord list (used by WAL strategies)
pub fn deserialize_vector_batch(avro_payload: &[u8]) -> Result<Vec<VectorRecord>> {
    use apache_avro::{Reader, Schema};

    let schema = Schema::parse_str(VECTOR_BATCH_SCHEMA_V1)
        .map_err(|e| anyhow::anyhow!("Failed to parse vector batch schema: {}", e))?;

    let reader = Reader::with_schema(&schema, avro_payload)
        .map_err(|e| anyhow::anyhow!("Failed to create Avro reader: {}", e))?;
    
    let mut result = Vec::new();
    
    for value in reader {
        let avro_value = value.map_err(|e| anyhow::anyhow!("Failed to read Avro value: {}", e))?;

        // Parse the Avro value into VectorRecord structs
        if let apache_avro::types::Value::Record(record) = avro_value {
            if let Some((_, apache_avro::types::Value::Array(vectors))) = record.iter().find(|(key, _)| *key == "vectors") {
                for vector_value in vectors {
                if let apache_avro::types::Value::Record(vector_record) = vector_value {
                    let id = vector_record
                        .iter()
                        .find(|(key, _)| key == "id")
                        .and_then(|(_, v)| match v {
                            apache_avro::types::Value::Union(_, inner) => {
                                if let apache_avro::types::Value::String(s) = inner.as_ref() {
                                    Some(s.clone())
                                } else {
                                    None
                                }
                            }
                            apache_avro::types::Value::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();

                    let vector = vector_record
                        .iter()
                        .find(|(key, _)| key == "vector")
                        .and_then(|(_, v)| {
                            if let apache_avro::types::Value::Array(arr) = v {
                                Some(
                                    arr.iter()
                                        .map(|f| {
                                            match f {
                                                // Direct float (backward compatibility)
                                                apache_avro::types::Value::Float(f) => *f,
                                                // Union with float (new sparse vector support)
                                                apache_avro::types::Value::Union(idx, inner) => {
                                                    if *idx == 1 {
                                                        if let apache_avro::types::Value::Float(f) = inner.as_ref() {
                                                            *f
                                                        } else {
                                                            0.0 // Default for invalid union value
                                                        }
                                                    } else {
                                                        0.0 // Null value (idx == 0) becomes 0.0 for sparse vectors
                                                    }
                                                }
                                                _ => 0.0, // Default for any other type
                                            }
                                        })
                                        .collect(),
                                )
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();

                    let metadata = vector_record
                        .iter()
                        .find(|(key, _)| key == "metadata")
                        .and_then(|(_, v)| match v {
                            apache_avro::types::Value::Union(_, inner) => {
                                if let apache_avro::types::Value::Map(map) = inner.as_ref() {
                                    Some(
                                        map.iter()
                                            .map(|(k, v)| {
                                                let value_str = match v {
                                                    apache_avro::types::Value::String(s) => s.clone(),
                                                    _ => format!("{:?}", v),
                                                };
                                                crate::proto::proximadb::MetadataItem {
                                                    key: k.clone(),
                                                    value: value_str,
                                                }
                                            })
                                            .collect(),
                                    )
                                } else {
                                    None
                                }
                            }
                            apache_avro::types::Value::Map(map) => {
                                Some(
                                    map.iter()
                                        .map(|(k, v)| {
                                            let value_str = match v {
                                                apache_avro::types::Value::String(s) => s.clone(),
                                                _ => format!("{:?}", v),
                                            };
                                            crate::proto::proximadb::MetadataItem {
                                                key: k.clone(),
                                                value: value_str,
                                            }
                                        })
                                        .collect(),
                                )
                            }
                            _ => None,
                        })
                        .unwrap_or_default();

                    let timestamp = vector_record
                        .iter()
                        .find(|(key, _)| key == "timestamp")
                        .and_then(|(_, v)| match v {
                            apache_avro::types::Value::Int(ts) => Some(*ts as i64),
                            apache_avro::types::Value::Long(ts) => Some(*ts),
                            _ => None,
                        })
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());

                    result.push(VectorRecord {
                        id: Some(id),
                        collection_id: String::new(), // Will be set by caller
                        vector,
                        metadata,
                        timestamp,
                        created_at: timestamp,
                        updated_at: timestamp,
                        expires_at: None,
                        version: 1,
                        rank: None,
                        score: None,
                        distance: None,
                    });
                }
            }
        }
    }
    }
    
    Ok(result)
}

/// Serialize vector batch to Avro binary (convenience function)
pub fn serialize_vector_batch(vector_records: &[VectorRecord]) -> Result<Vec<u8>> {
    create_avro_vector_batch(vector_records)
}

/// Serialize Avro VectorRecord batch to bytes
pub fn serialize_avro_vector_batch(avro_records: &[crate::core::avro_unified::VectorRecord]) -> Result<Vec<u8>> {
    use apache_avro::Schema;

    let schema = Schema::parse_str(VECTOR_BATCH_SCHEMA_V1)
        .map_err(|e| anyhow::anyhow!("Failed to parse vector batch schema: {}", e))?;

    let mut writer = apache_avro::Writer::new(&schema, Vec::new());
    
    for record in avro_records {
        writer
            .append_ser(record)
            .map_err(|e| anyhow::anyhow!("Failed to serialize vector record: {}", e))?;
    }

    writer
        .into_inner()
        .map_err(|e| anyhow::anyhow!("Failed to finalize Avro batch: {}", e))
}

// convert_to_avro_entry removed - use WalVectorBatch for batch operations

// ============================================================================
// PROTO-BASED WAL SERIALIZATION (Phase 2: Migration to Proto)
// ============================================================================

/// Proto-based vector batch wrapper for WAL operations
/// This will replace AvroVectorBatch once migration is complete
#[derive(Clone, Message)]
pub struct ProtoVectorBatch {
    /// Batch of vector records
    #[prost(message, repeated, tag = "1")]
    pub vectors: Vec<crate::proto::proximadb::VectorRecord>,
    
    /// Batch metadata
    #[prost(string, optional, tag = "2")]
    pub batch_id: Option<String>,
    
    /// Batch timestamp (microseconds since epoch)
    #[prost(int64, tag = "3")]
    pub timestamp: i64,
    
    /// Collection ID for this batch
    #[prost(string, tag = "4")]
    pub collection_id: String,
}

/// Create Proto vector batch from VectorRecord list (Phase 2 implementation)
/// Create proto vector batch - VectorRecord is already proto type
pub fn create_proto_vector_batch(vector_records: &[VectorRecord], _collection_id: &str) -> Result<Vec<u8>> {
    // No conversion needed - VectorRecord is already proto type
    let proto_vectors = vector_records.to_vec();
    
    let batch = ProtoVectorBatch {
        vectors: proto_vectors,
        batch_id: Some(format!("batch_{}", chrono::Utc::now().timestamp_micros())),
        timestamp: chrono::Utc::now().timestamp_micros(),
        collection_id: _collection_id.to_string(),
    };
    
    // Serialize using protobuf
    let mut buf = Vec::new();
    batch.encode(&mut buf)
        .map_err(|e| anyhow::anyhow!("Failed to encode proto vector batch: {}", e))?;
    
    Ok(buf)
}

/// Deserialize Proto vector batch to VectorRecord list (Phase 2 implementation)
/// This function deserializes protobuf and converts back to Avro VectorRecords for compatibility
pub fn deserialize_proto_vector_batch(proto_payload: &[u8]) -> Result<Vec<VectorRecord>> {
    use crate::core::proto_to_avro;
    
    // Deserialize protobuf
    let batch = ProtoVectorBatch::decode(proto_payload)
        .map_err(|e| anyhow::anyhow!("Failed to decode proto vector batch: {}", e))?;
    
    // Direct return - VectorRecord is already proto type in proto-first architecture
    Ok(batch.vectors)
}

/// Create Proto vector batch from Proto VectorRecords directly (Future Phase 3)
/// This will be used once we fully migrate to Proto VectorRecord throughout
pub fn create_proto_vector_batch_native(
    proto_vectors: &[crate::proto::proximadb::VectorRecord], 
    collection_id: &str
) -> Result<Vec<u8>> {
    let batch = ProtoVectorBatch {
        vectors: proto_vectors.to_vec(),
        batch_id: Some(format!("batch_{}", chrono::Utc::now().timestamp_micros())),
        timestamp: chrono::Utc::now().timestamp_micros(),
        collection_id: collection_id.to_string(),
    };
    
    let mut buf = Vec::new();
    batch.encode(&mut buf)
        .map_err(|e| anyhow::anyhow!("Failed to encode proto vector batch: {}", e))?;
    
    Ok(buf)
}

/// Deserialize Proto vector batch to Proto VectorRecords directly (Future Phase 3)
/// This will be used once we fully migrate to Proto VectorRecord throughout
pub fn deserialize_proto_vector_batch_native(proto_payload: &[u8]) -> Result<(Vec<crate::proto::proximadb::VectorRecord>, String)> {
    let batch = ProtoVectorBatch::decode(proto_payload)
        .map_err(|e| anyhow::anyhow!("Failed to decode proto vector batch: {}", e))?;
    
    Ok((batch.vectors, batch.collection_id))
}

/// Unified deserialization function that handles both Avro and Proto formats
/// This provides backward compatibility during the migration period
pub fn deserialize_vector_batch_unified(payload: &[u8]) -> Result<Vec<VectorRecord>> {
    // Try proto first (new format), fall back to Avro (legacy format)
    match deserialize_proto_vector_batch(payload) {
        Ok(vectors) => Ok(vectors),
        Err(_) => {
            // Fallback to Avro deserialization for backward compatibility
            deserialize_vector_batch(payload)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proto_vector_batch_serialization() {
        use crate::core::VectorRecord;
        
        let records = vec![
            VectorRecord {
                id: Some("vec-1".to_string()),
                collection_id: "test-collection".to_string(),
                vector: vec![1.0, 2.0, 3.0, 4.0],
                metadata: vec![
                    crate::proto::proximadb::MetadataItem {
                        key: "category".to_string(),
                        value: "test".to_string(),
                    },
                    crate::proto::proximadb::MetadataItem {
                        key: "score".to_string(),
                        value: "42".to_string(),
                    },
                ],
                timestamp: 1640995200000000,
                created_at: 1640995200000,
                updated_at: 1640995200000,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
            VectorRecord {
                id: Some("vec-2".to_string()),
                collection_id: "test-collection".to_string(),
                vector: vec![5.0, 6.0, 7.0, 8.0],
                metadata: vec![
                    crate::proto::proximadb::MetadataItem {
                        key: "category".to_string(),
                        value: "example".to_string(),
                    },
                    crate::proto::proximadb::MetadataItem {
                        key: "active".to_string(),
                        value: "true".to_string(),
                    },
                ],
                timestamp: 1640995201000000,
                created_at: 1640995201000,
                updated_at: 1640995201000,
                expires_at: Some(1640995300000),
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
        ];
        
        // Test proto serialization
        let proto_payload = create_proto_vector_batch(&records, "test-collection")
            .expect("Failed to create proto batch");
        assert!(!proto_payload.is_empty());
        
        // Test proto deserialization
        let deserialized = deserialize_proto_vector_batch(&proto_payload)
            .expect("Failed to deserialize proto batch");
        
        assert_eq!(records.len(), deserialized.len());
        assert_eq!(records[0].id, deserialized[0].id);
        assert_eq!(records[0].vector, deserialized[0].vector);
        assert_eq!(records[1].id, deserialized[1].id);
        assert_eq!(records[1].vector, deserialized[1].vector);
        
        // Verify metadata was preserved
        let original_category = records[0].metadata.iter()
            .find(|item| item.key == "category")
            .map(|item| &item.value);
        let deserialized_category = deserialized[0].metadata.iter()
            .find(|item| item.key == "category")
            .map(|item| &item.value);
        assert_eq!(original_category, deserialized_category);
        let original_score = records[0].metadata.iter()
            .find(|item| item.key == "score")
            .map(|item| &item.value);
        let deserialized_score = deserialized[0].metadata.iter()
            .find(|item| item.key == "score")
            .map(|item| &item.value);
        assert_eq!(original_score, deserialized_score);
    }
    
    #[test]
    fn test_unified_deserialization() {
        use crate::core::VectorRecord;
        
        let records = vec![
            VectorRecord {
                id: Some("test-vec".to_string()),
                collection_id: "test-collection".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: vec![],
                timestamp: 1640995200000000,
                created_at: 1640995200000,
                updated_at: 1640995200000,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
        ];
        
        // Test that unified function can handle proto format
        let proto_payload = create_proto_vector_batch(&records, "test-collection")
            .expect("Failed to create proto batch");
        let deserialized_proto = deserialize_vector_batch_unified(&proto_payload)
            .expect("Failed to deserialize proto via unified");
        assert_eq!(records[0].id, deserialized_proto[0].id);
        
        // Test that unified function can handle avro format
        let avro_payload = create_avro_vector_batch(&records)
            .expect("Failed to create avro batch");
        let deserialized_avro = deserialize_vector_batch_unified(&avro_payload)
            .expect("Failed to deserialize avro via unified");
        assert_eq!(records[0].id, deserialized_avro[0].id);
    }
    
    #[test]
    fn test_proto_native_functions() {
        use crate::proto::proximadb::{VectorRecord as ProtoVectorRecord, MetadataMap, MetadataValue, metadata_value};
        
        let mut metadata_fields = HashMap::new();
        metadata_fields.insert("test".to_string(), MetadataValue {
            value: Some(metadata_value::Value::StringValue("value".to_string()))
        });
        
        let proto_vectors = vec![
            ProtoVectorRecord {
                id: Some("proto-vec-1".to_string()),
                collection_id: "test-collection".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: vec![
                    crate::proto::proximadb::MetadataItem {
                        key: "category".to_string(),
                        value: "test".to_string(),
                    },
                ],
                timestamp: 1640995200000000,
                created_at: 1640995200000,
                updated_at: 1640995200000,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
        ];
        
        // Test native proto serialization
        let payload = create_proto_vector_batch_native(&proto_vectors, "test-collection")
            .expect("Failed to create native proto batch");
        assert!(!payload.is_empty());
        
        // Test native proto deserialization  
        let (deserialized, collection_id) = deserialize_proto_vector_batch_native(&payload)
            .expect("Failed to deserialize native proto batch");
        
        assert_eq!(collection_id, "test-collection");
        assert_eq!(proto_vectors.len(), deserialized.len());
        assert_eq!(proto_vectors[0].id, deserialized[0].id);
        assert_eq!(proto_vectors[0].vector, deserialized[0].vector);
    }
    
    #[test]
    fn test_vector_record_avro_conversion() {
        let metadata = vec![
            crate::proto::proximadb::MetadataItem {
                key: "key1".to_string(),
                value: "value1".to_string(),
            },
            crate::proto::proximadb::MetadataItem {
                key: "key2".to_string(),
                value: "42".to_string(),
            },
        ];

        let record = VectorRecord {
            id: Some("test_vector".to_string()),
            collection_id: "test_collection".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata,
            timestamp: 1234000000, // Use timestamp that doesn't lose precision in Avro conversion
            created_at: 1234000000,
            updated_at: 1234000000,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
            };

        // Test VectorRecord -> AvroVector -> VectorRecord
        let avro_vector = AvroVector::from(&record);
        let restored_record = VectorRecord::try_from(&avro_vector).expect("Failed to convert back to VectorRecord");

        assert_eq!(record.id.as_deref().unwrap_or(""), restored_record.id.as_deref().unwrap_or(""));
        assert_eq!(record.vector, restored_record.vector);
        assert_eq!(record.timestamp, restored_record.timestamp);
        // Metadata comparison (Vec<MetadataItem> format)
        assert!(restored_record.metadata.iter().any(|item| item.key == "key1"));
        assert!(restored_record.metadata.iter().any(|item| item.key == "key2"));
    }

    #[test]
    fn test_vector_batch_serialization() {
        let records = vec![
            VectorRecord {
                id: Some("vector1".to_string()),
                collection_id: "test".to_string(),
                vector: vec![1.0, 2.0],
                metadata: vec![],
                timestamp: 1234567890,
                created_at: 1234567890,
                updated_at: 1234567890,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
            VectorRecord {
                id: Some("vector2".to_string()),
                collection_id: "test".to_string(),
                vector: vec![3.0, 4.0],
                metadata: vec![],
                timestamp: 1234567891,
                created_at: 1234567891,
                updated_at: 1234567891,
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            },
        ];

        // Test serialization and deserialization
        let serialized = serialize_vector_batch(&records).expect("Failed to serialize");
        let deserialized = deserialize_vector_batch(&serialized).expect("Failed to deserialize");

        assert_eq!(records.len(), deserialized.len());
        assert_eq!(records[0].id, deserialized[0].id);
        assert_eq!(records[0].vector, deserialized[0].vector);
        assert_eq!(records[1].id, deserialized[1].id);
        assert_eq!(records[1].vector, deserialized[1].vector);
    }
}
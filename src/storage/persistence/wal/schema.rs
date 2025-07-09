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
          {"name": "vector", "type": {"type": "array", "items": "float"}},
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
        // Convert metadata from HashMap<String, serde_json::Value> to HashMap<String, String>
        let metadata = if record.metadata.is_empty() {
            None
        } else {
            Some(
                record
                    .metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_string()))
                    .collect(),
            )
        };

        Self {
            id: if record.id.is_empty() { None } else { Some(record.id.clone()) },
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
        let id = avro.id.as_ref()
            .cloned()
            .unwrap_or_default(); // Empty string for immutable vectors
        
        Ok(Self {
            id,
            collection_id: avro.collection_id.clone(),
            vector: avro.vector.clone(),
            metadata,
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
    use apache_avro::{to_avro_datum, Schema};

    let schema = Schema::parse_str(VECTOR_BATCH_SCHEMA_V1)
        .map_err(|e| anyhow::anyhow!("Failed to parse vector batch schema: {}", e))?;

    // Convert VectorRecord to AvroVector
    let avro_vectors: Vec<AvroVector> = vector_records.iter().map(AvroVector::from).collect();

    let batch = AvroVectorBatch {
        vectors: avro_vectors,
    };

    // Convert to Avro Value and serialize to binary format
    use apache_avro::types::Value;
    
    let batch_value = Value::Record(vec![
        ("vectors".to_string(), Value::Array(
            batch.vectors.into_iter().map(|v| Value::Record(vec![
                ("id".to_string(), match v.id {
                    Some(id) => Value::String(id),
                    None => Value::Null,
                }),
                ("collection_id".to_string(), Value::String(v.collection_id)),
                ("vector".to_string(), Value::Array(v.vector.into_iter().map(Value::Float).collect())),
                ("metadata".to_string(), match v.metadata {
                    Some(meta) => Value::Map(meta.into_iter().map(|(k, v)| (k, Value::String(v))).collect()),
                    None => Value::Null,
                }),
                ("timestamp".to_string(), Value::Int(v.timestamp)),
                ("expires_at".to_string(), match v.expires_at {
                    Some(exp) => Value::Int(exp),
                    None => Value::Null,
                }),
                ("version".to_string(), Value::Int(v.version)),
            ])).collect()
        ))
    ]);
    
    let avro_bytes = to_avro_datum(&schema, batch_value)
        .map_err(|e| anyhow::anyhow!("Failed to serialize vector batch: {}", e))?;

    Ok(avro_bytes)
}

/// Deserialize Avro vector batch to VectorRecord list (used by WAL strategies)
pub fn deserialize_vector_batch(avro_payload: &[u8]) -> Result<Vec<VectorRecord>> {
    use apache_avro::{from_avro_datum, Schema};

    let schema = Schema::parse_str(VECTOR_BATCH_SCHEMA_V1)
        .map_err(|e| anyhow::anyhow!("Failed to parse vector batch schema: {}", e))?;

    let mut reader = std::io::Cursor::new(avro_payload);
    let avro_value = from_avro_datum(&schema, &mut reader, None)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize Avro payload: {}", e))?;

    // Parse the Avro value into VectorRecord structs
    if let apache_avro::types::Value::Record(record) = avro_value {
        if let Some((_, apache_avro::types::Value::Array(vectors))) = record.iter().find(|(key, _)| *key == "vectors") {
            let mut result = Vec::new();
            for vector_value in vectors {
                if let apache_avro::types::Value::Record(vector_record) = vector_value {
                    let id = vector_record
                        .iter()
                        .find(|(key, _)| key == "id")
                        .and_then(|(_, v)| {
                            if let apache_avro::types::Value::String(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();

                    let vector = vector_record
                        .iter()
                        .find(|(key, _)| key == "vector")
                        .and_then(|(_, v)| {
                            if let apache_avro::types::Value::Array(arr) = v {
                                Some(
                                    arr.iter()
                                        .filter_map(|f| {
                                            if let apache_avro::types::Value::Float(f) = f {
                                                Some(*f)
                                            } else {
                                                None
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
                        .and_then(|(_, v)| {
                            if let apache_avro::types::Value::Map(map) = v {
                                Some(
                                    map.iter()
                                        .map(|(k, v)| {
                                            let value = match v {
                                                apache_avro::types::Value::String(s) => {
                                                    serde_json::Value::String(s.clone())
                                                }
                                                _ => serde_json::Value::String(format!("{:?}", v)),
                                            };
                                            (k.clone(), value)
                                        })
                                        .collect(),
                                )
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();

                    let timestamp = vector_record
                        .iter()
                        .find(|(key, _)| key == "timestamp")
                        .and_then(|(_, v)| {
                            if let apache_avro::types::Value::Long(ts) = v {
                                Some(*ts)
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());

                    result.push(VectorRecord {
                        id,
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
            return Ok(result);
        }
    }

    anyhow::bail!("Invalid Avro payload structure")
}

/// Serialize vector batch to Avro binary (convenience function)
pub fn serialize_vector_batch(vector_records: &[VectorRecord]) -> Result<Vec<u8>> {
    create_avro_vector_batch(vector_records)
}

/// Convert WalEntry to AvroVector for disk writing (compatibility function)
/// This is a temporary function to support legacy disk.rs until we implement direct payload writing
pub fn convert_to_avro_entry(entry: &crate::storage::persistence::wal::WalEntry) -> Result<AvroVector> {
    // Handle different payload formats
    let vectors = match entry.operation.payload_format.as_str() {
        "avro" => {
            // Deserialize Avro payload
            deserialize_vector_batch(&entry.operation.payload_data)?
        }
        "bincode" => {
            // For bincode, we'd need a bincode deserializer
            // For now, return error as disk.rs should use direct payload writing for bincode
            anyhow::bail!("Bincode payload format not supported in legacy conversion function - use direct payload writing")
        }
        format => {
            anyhow::bail!("Unknown payload format: {}", format)
        }
    };
    
    if vectors.is_empty() {
        anyhow::bail!("WalOperation contains empty vector batch")
    }
    
    // Return first vector in the batch (for disk.rs compatibility)
    Ok(AvroVector::from(&vectors[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_record_avro_conversion() {
        let mut metadata = HashMap::new();
        metadata.insert("key1".to_string(), serde_json::Value::String("value1".to_string()));
        metadata.insert("key2".to_string(), serde_json::Value::Number(serde_json::Number::from(42)));

        let record = VectorRecord {
            id: "test_vector".to_string(),
            collection_id: "test_collection".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata,
            timestamp: 1234567890,
            created_at: 1234567890,
            updated_at: 1234567890,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };

        // Test VectorRecord -> AvroVector -> VectorRecord
        let avro_vector = AvroVector::from(&record);
        let restored_record = VectorRecord::try_from(&avro_vector).expect("Failed to convert back to VectorRecord");

        assert_eq!(record.id, restored_record.id);
        assert_eq!(record.vector, restored_record.vector);
        assert_eq!(record.timestamp, restored_record.timestamp);
        // Metadata comparison (string conversion expected)
        assert!(restored_record.metadata.contains_key("key1"));
        assert!(restored_record.metadata.contains_key("key2"));
    }

    #[test]
    fn test_vector_batch_serialization() {
        let records = vec![
            VectorRecord {
                id: "vector1".to_string(),
                collection_id: "test".to_string(),
                vector: vec![1.0, 2.0],
                metadata: HashMap::new(),
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
                id: "vector2".to_string(),
                collection_id: "test".to_string(),
                vector: vec![3.0, 4.0],
                metadata: HashMap::new(),
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
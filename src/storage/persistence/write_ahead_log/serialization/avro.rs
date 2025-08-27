//! Avro serialization for WAL
//! 
//! Provides schema evolution support for backward compatibility.

use anyhow::Result;
use crate::core::VectorRecord;
use apache_avro::{Schema, Writer, Reader, types::Value};
use std::collections::HashMap;

/// ULTRA-FRUGAL vector batch schema - optimized for minimal memory/disk footprint
/// Uses smaller data types and optional fields to reduce serialization overhead
const VECTOR_BATCH_SCHEMA_V1: &str = r#"
{
  "type": "record",
  "name": "WALVectorBatch",
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
          {"name": "metadata_info", "type": ["null", {"type": "map", "values": "string"}], "default": null},
          {"name": "timestamp", "type": "int"},
          {"name": "expires_at", "type": ["null", "int"], "default": null},
          {"name": "version", "type": "int"}
        ]
      }
    }}
  ]
}
"#;

/// Avro serializer - for schema evolution support
#[derive(Debug, Clone, Default)]
pub struct AvroSerializer;

impl AvroSerializer {
    /// Create a new Avro serializer
    pub fn new() -> Self {
        Self::default()
    }
}

impl super::VectorBatchSerializer for AvroSerializer {
    fn serialize_batch(&self, vectors: &[VectorRecord]) -> Result<Vec<u8>> {
        let schema = Schema::parse_str(VECTOR_BATCH_SCHEMA_V1)
            .map_err(|e| anyhow::anyhow!("Failed to parse vector batch schema: {}", e))?;

        // Use apache_avro Writer for proper serialization
        let mut writer = Writer::new(&schema, Vec::new());
        
        // Create the batch value with proper field ordering
        let vectors_array = Value::Array(
            vectors.iter().map(|v| {
                let mut fields = vec![];
                
                // Add fields in the order they appear in the schema
                fields.push(("id".to_string(), match &v.id {
                    Some(id) => Value::Union(1, Box::new(Value::String(id.clone()))),
                    None => Value::Union(0, Box::new(Value::Null)),
                }));
                
                // Collection ID is managed externally, use empty string
                fields.push(("collection_id".to_string(), Value::String(String::new())));
                
                fields.push(("vector".to_string(), Value::Array(
                    v.vector.iter().map(|&f| {
                        // For sparse vectors, we could encode zero as null to save space
                        // For now, always use the float value (Union index 1)
                        Value::Union(1, Box::new(Value::Float(f)))
                    }).collect()
                )));
                
                // Convert metadata from Vec<MetadataItem> to Map for Avro
                let metadata_map: HashMap<String, String> = v.metadata.iter()
                    .map(|item| {
                        let value_str = match &item.value {
                            Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) => s.clone(),
                            Some(crate::proto::proximadb::metadata_item::Value::NumberValue(n)) => n.to_string(),
                            Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)) => b.to_string(),
                            None => String::new(),
                        };
                        (item.key.clone(), value_str)
                    })
                    .collect();
                    
                fields.push(("metadata_info".to_string(), if metadata_map.is_empty() {
                    Value::Union(0, Box::new(Value::Null))
                } else {
                    Value::Union(1, Box::new(Value::Map(
                        metadata_map.into_iter().map(|(k, v)| (k, Value::String(v))).collect()
                    )))
                }));
                
                // Convert microsecond timestamp to seconds (much smaller)
                fields.push(("timestamp".to_string(), Value::Int((v.timestamp / 1_000_000) as i32)));
                
                fields.push(("expires_at".to_string(), match v.expires_at {
                    Some(exp) => Value::Union(1, Box::new(Value::Int((exp / 1_000_000) as i32))),
                    None => Value::Union(0, Box::new(Value::Null)),
                }));
                
                fields.push(("version".to_string(), Value::Int(v.version as i32)));
                
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
    
    fn deserialize_batch(&self, data: &[u8]) -> Result<Vec<VectorRecord>> {
        let schema = Schema::parse_str(VECTOR_BATCH_SCHEMA_V1)
            .map_err(|e| anyhow::anyhow!("Failed to parse vector batch schema: {}", e))?;

        let reader = Reader::with_schema(&schema, data)
            .map_err(|e| anyhow::anyhow!("Failed to create Avro reader: {}", e))?;
        
        let mut result = Vec::new();
        
        for value in reader {
            let avro_value = value.map_err(|e| anyhow::anyhow!("Failed to read Avro value: {}", e))?;

            // Parse the Avro value into VectorRecord structs
            if let Value::Record(record) = avro_value {
                if let Some((_, Value::Array(vectors))) = record.iter().find(|(key, _)| *key == "vectors") {
                    for vector_value in vectors {
                        if let Value::Record(vector_record) = vector_value {
                            let id = vector_record
                                .iter()
                                .find(|(key, _)| key == "id")
                                .and_then(|(_, v)| match v {
                                    Value::Union(_, inner) => {
                                        if let Value::String(s) = inner.as_ref() {
                                            Some(s.clone())
                                        } else {
                                            None
                                        }
                                    }
                                    Value::String(s) => Some(s.clone()),
                                    _ => None,
                                })
                                .filter(|s| !s.is_empty());

                            let vector = vector_record
                                .iter()
                                .find(|(key, _)| key == "vector")
                                .and_then(|(_, v)| {
                                    if let Value::Array(arr) = v {
                                        Some(
                                            arr.iter()
                                                .map(|f| {
                                                    match f {
                                                        // Direct float (backward compatibility)
                                                        Value::Float(f) => *f,
                                                        // Union with float (new sparse vector support)
                                                        Value::Union(idx, inner) => {
                                                            if *idx == 1 {
                                                                if let Value::Float(f) = inner.as_ref() {
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
                                .clone();

                            let metadata = vector_record
                                .iter()
                                .find(|(key, _)| key == "metadata_info")
                                .and_then(|(_, v)| match v {
                                    Value::Union(_, inner) => {
                                        if let Value::Map(map) = inner.as_ref() {
                                            Some(
                                                map.iter()
                                                    .map(|(k, v)| {
                                                        let metadata_value = match v {
                                                            Value::String(s) => Some(crate::proto::proximadb::metadata_item::Value::StringValue(s.clone())),
                                                            Value::Float(f) => Some(crate::proto::proximadb::metadata_item::Value::NumberValue(*f as f64)),
                                                            Value::Double(f) => Some(crate::proto::proximadb::metadata_item::Value::NumberValue(*f)),
                                                            Value::Int(i) => Some(crate::proto::proximadb::metadata_item::Value::NumberValue(*i as f64)),
                                                            Value::Long(i) => Some(crate::proto::proximadb::metadata_item::Value::NumberValue(*i as f64)),
                                                            Value::Boolean(b) => Some(crate::proto::proximadb::metadata_item::Value::BoolValue(*b)),
                                                            _ => Some(crate::proto::proximadb::metadata_item::Value::StringValue(format!("{:?}", v))),
                                                        };
                                                        crate::proto::proximadb::MetadataItem {
                                                            key: k.clone(),
                                                            value: metadata_value,
                                                        }
                                                    })
                                                    .collect(),
                                            )
                                        } else {
                                            None
                                        }
                                    }
                                    Value::Map(map) => {
                                        Some(
                                            map.iter()
                                                .map(|(k, v)| {
                                                    let metadata_value = match v {
                                                        Value::String(s) => Some(crate::proto::proximadb::metadata_item::Value::StringValue(s.clone())),
                                                        Value::Float(f) => Some(crate::proto::proximadb::metadata_item::Value::NumberValue(*f as f64)),
                                                        Value::Double(f) => Some(crate::proto::proximadb::metadata_item::Value::NumberValue(*f)),
                                                        Value::Int(i) => Some(crate::proto::proximadb::metadata_item::Value::NumberValue(*i as f64)),
                                                        Value::Long(i) => Some(crate::proto::proximadb::metadata_item::Value::NumberValue(*i as f64)),
                                                        Value::Boolean(b) => Some(crate::proto::proximadb::metadata_item::Value::BoolValue(*b)),
                                                        _ => Some(crate::proto::proximadb::metadata_item::Value::StringValue(format!("{:?}", v))),
                                                    };
                                                    crate::proto::proximadb::MetadataItem {
                                                        key: k.clone(),
                                                        value: metadata_value,
                                                    }
                                                })
                                                .collect(),
                                        )
                                    }
                                    _ => None,
                                })
                                .clone();

                            let timestamp_seconds = vector_record
                                .iter()
                                .find(|(key, _)| key == "timestamp")
                                .and_then(|(_, v)| match v {
                                    Value::Int(ts) => Some(*ts as i64),
                                    Value::Long(ts) => Some(*ts),
                                    _ => None,
                                })
                                ;
                            
                            // Convert seconds back to microseconds
                            let timestamp_micros = timestamp_seconds * 1_000_000;

                            result.push(VectorRecord {
                                id,
                                vector,
                                metadata,
                                timestamp: (timestamp_micros / 1_000_000) as u32,
                                updated_at: Some((timestamp_micros / 1_000_000) as u32),
                                expires_at: vector_record
                                    .iter()
                                    .find(|(key, _)| key == "expires_at")
                                    .and_then(|(_, v)| match v {
                                        Value::Union(idx, inner) if *idx == 1 => {
                                            if let Value::Int(exp) = inner.as_ref() {
                                                Some(*exp as u32)
                                            } else {
                                                None
                                            }
                                        }
                                        _ => None,
                                    }),
                                version: vector_record
                                    .iter()
                                    .find(|(key, _)| key == "version")
                                    .and_then(|(_, v)| match v {
                                        Value::Int(ver) => Some(*ver as u32),
                                        _ => None,
                                    }),
                                quantized_vector: None,
                            });
                        }
                    }
                }
            }
        }
        
        Ok(result)
    }
    
    fn format(&self) -> super::SerializationFormat {
        super::SerializationFormat::Avro
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb::MetadataItem;
    use crate::storage::persistence::write_ahead_log::serialization::VectorBatchSerializer;

    fn create_test_vector() -> VectorRecord {
        VectorRecord {
            id: Some("test_vector_1".to_string()),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("test".to_string())),
                },
            ],
            timestamp: 1234567890,
            updated_at: Some(1234567890),
            expires_at: None,
            version: Some(1),
            quantized_vector: None,
        }
    }

    #[test]
    fn test_avro_round_trip() {
        let serializer = AvroSerializer::new();
        let vectors = vec![create_test_vector()];
        
        // Serialize
        let serialized = serializer.serialize_batch(&vectors)
            .expect("Failed to serialize batch");
        assert!(!serialized.is_empty());
        
        // Deserialize
        let deserialized = serializer.deserialize_batch(&serialized)
            .expect("Failed to deserialize batch");
        assert_eq!(deserialized.len(), 1);
        assert_eq!(deserialized[0].id, vectors[0].id);
        assert_eq!(deserialized[0].vector, vectors[0].vector);
    }

    #[test]
    fn test_metadata_preservation() {
        let serializer = AvroSerializer::new();
        let mut vector = create_test_vector();
        vector.metadata = vec![
            MetadataItem {
                key: "key1".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("value1".to_string())),
            },
            MetadataItem {
                key: "key2".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("value2".to_string())),
            },
        ];
        
        let vectors = vec![vector];
        let serialized = serializer.serialize_batch(&vectors)
            .expect("Failed to serialize batch");
        let deserialized = serializer.deserialize_batch(&serialized)
            .expect("Failed to deserialize batch");
        
        assert_eq!(deserialized[0].metadata.len(), 2);
        
        // Check that both keys are present (order doesn't matter due to HashMap)
        let keys: std::collections::HashSet<String> = deserialized[0].metadata.iter()
            .map(|item| item.key.clone())
            .collect();
        assert!(keys.contains_hash("key1"));
        assert!(keys.contains_hash("key2"));
        
        // Find and verify each key-value pair
        let key1_item = deserialized[0].metadata.iter().find(|item| item.key == "key1").unwrap();
        let key2_item = deserialized[0].metadata.iter().find(|item| item.key == "key2").unwrap();
        assert!(matches!(&key1_item.value, Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) if s == "value1"));
        assert!(matches!(&key2_item.value, Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) if s == "value2"));
    }
    
    #[test]
    fn test_format_identifier() {
        let serializer = AvroSerializer::new();
        assert_eq!(serializer.format(), super::super::SerializationFormat::Avro);
    }
}
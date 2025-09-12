//! Serialization helpers for proto types
//!
//! This module provides helper functions to serialize proto types
//! that don't implement Serialize trait directly.

use crate::proto::proximadb_v1;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use anyhow::Result;

/// Serializable wrapper for VectorRecord for bincode/other serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableVectorRecord {
    pub id: String,
    pub collection_id: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: u64,
    pub version: u64,
}

impl From<&proximadb_v1::VectorRecord> for SerializableVectorRecord {
    fn from(record: &proximadb_v1::VectorRecord) -> Self {
        Self {
            id: record.id.clone(),
            collection_id: record.collection_id.clone(),
            vector: record.vector.clone(),
            metadata: record.metadata.iter().map(|(k, v)| {
                let json_value = sql_value_to_json(v);
                (k.clone(), json_value)
            }).collect(),
            timestamp: record.timestamp,
            version: record.version,
        }
    }
}

impl From<SerializableVectorRecord> for proximadb_v1::VectorRecord {
    fn from(record: SerializableVectorRecord) -> Self {
        Self {
            id: record.id,
            collection_id: record.collection_id,
            vector: record.vector,
            metadata: record.metadata.into_iter().map(|(k, v)| {
                let sql_value = json_to_sql_value(v);
                (k, sql_value)
            }).collect(),
            timestamp: record.timestamp,
            version: record.version,
        }
    }
}

/// Serialize a VectorRecord using bincode
pub fn serialize_vector_record(record: &proximadb_v1::VectorRecord) -> Result<Vec<u8>> {
    let serializable = SerializableVectorRecord::from(record);
    Ok(bincode::serialize(&serializable)?)
}

/// Deserialize a VectorRecord using bincode
pub fn deserialize_vector_record(data: &[u8]) -> Result<proximadb_v1::VectorRecord> {
    let serializable: SerializableVectorRecord = bincode::deserialize(data)?;
    Ok(proximadb_v1::VectorRecord::from(serializable))
}

/// Serialize multiple VectorRecords using bincode
pub fn serialize_vector_records(records: &[proximadb_v1::VectorRecord]) -> Result<Vec<u8>> {
    let serializables: Vec<SerializableVectorRecord> = records.iter().map(SerializableVectorRecord::from).collect();
    Ok(bincode::serialize(&serializables)?)
}

/// Deserialize multiple VectorRecords using bincode  
pub fn deserialize_vector_records(data: &[u8]) -> Result<Vec<proximadb_v1::VectorRecord>> {
    let serializables: Vec<SerializableVectorRecord> = bincode::deserialize(data)?;
    Ok(serializables.into_iter().map(proximadb_v1::VectorRecord::from).collect())
}

/// Convert SqlValue to serde_json::Value
fn sql_value_to_json(value: &proximadb_v1::SqlValue) -> serde_json::Value {
    use proximadb_v1::sql_value::Value;
    match value.value.as_ref() {
        Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(Value::NumberValue(n)) => serde_json::Value::Number(
            serde_json::Number::from_f64(*n).unwrap_or(serde_json::Number::from(0))
        ),
        Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(Value::Int64Value(i)) => serde_json::Value::Number((*i).into()),
        Some(Value::BytesValue(b)) => {
            serde_json::Value::Array(
                b.iter().map(|x| serde_json::Value::Number((*x as u64).into())).collect()
            )
        },
        Some(Value::NullValue(_)) => serde_json::Value::Null,
        Some(Value::ArrayValue(arr)) => {
            serde_json::Value::Array(arr.values.iter().map(sql_value_to_json).collect())
        },
        Some(Value::ObjectValue(obj)) => {
            let mut map = serde_json::Map::new();
            for (k, sv) in &obj.fields {
                map.insert(k.clone(), sql_value_to_json(sv));
            }
            serde_json::Value::Object(map)
        },
        None => serde_json::Value::Null,
    }
}

/// Convert serde_json::Value to SqlValue
fn json_to_sql_value(value: serde_json::Value) -> proximadb_v1::SqlValue {
    use proximadb_v1::sql_value::Value;
    let inner_value = match value {
        serde_json::Value::String(s) => Some(Value::StringValue(s)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Value::Int64Value(i))
            } else if let Some(f) = n.as_f64() {
                Some(Value::NumberValue(f))
            } else {
                Some(Value::NumberValue(0.0))
            }
        },
        serde_json::Value::Bool(b) => Some(Value::BoolValue(b)),
        serde_json::Value::Null => Some(Value::NullValue(0)),
        serde_json::Value::Array(arr) => {
            let values = arr.into_iter().map(json_to_sql_value).collect();
            Some(Value::ArrayValue(proximadb_v1::SqlArray { values }))
        },
        serde_json::Value::Object(obj) => {
            let fields = obj.into_iter().map(|(k, v)| (k, json_to_sql_value(v))).collect();
            Some(Value::ObjectValue(proximadb_v1::SqlObject { fields }))
        },
    };
    
    proximadb_v1::SqlValue { value: inner_value }
}

/// Serializable wrapper for Collection for Avro/other serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableCollection {
    pub id: String,
    pub name: String,
    pub dimension: u32,
    pub distance_metric: i32,
    pub created_at: String,
    pub updated_at: String,
    pub metadata: HashMap<String, serde_json::Value>,
    pub vector_count: u64,
    pub index_type: String,
    pub storage_config: HashMap<String, serde_json::Value>,
    pub quantization_config: Option<SerializableQuantizationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableQuantizationConfig {
    pub quantization_type: i32,
    pub bits: u32,
    pub training_sample_size: u64,
}

impl From<&proximadb_v1::Collection> for SerializableCollection {
    fn from(collection: &proximadb_v1::Collection) -> Self {
        Self {
            id: collection.id.clone(),
            name: collection.name.clone(),
            dimension: collection.dimension,
            distance_metric: collection.distance_metric,
            created_at: collection.created_at.clone(),
            updated_at: collection.updated_at.clone(),
            metadata: collection.metadata.iter().map(|(k, v)| {
                (k.clone(), sql_value_to_json(v))
            }).collect(),
            vector_count: collection.vector_count,
            index_type: collection.index_type.clone(),
            storage_config: collection.storage_config.iter().map(|(k, v)| {
                (k.clone(), sql_value_to_json(v))
            }).collect(),
            quantization_config: collection.quantization_config.as_ref().map(|qc| SerializableQuantizationConfig {
                quantization_type: qc.quantization_type,
                bits: qc.bits,
                training_sample_size: qc.training_sample_size,
            }),
        }
    }
}

impl From<SerializableCollection> for proximadb_v1::Collection {
    fn from(collection: SerializableCollection) -> Self {
        Self {
            id: collection.id,
            name: collection.name,
            dimension: collection.dimension,
            distance_metric: collection.distance_metric,
            created_at: collection.created_at,
            updated_at: collection.updated_at,
            metadata: collection.metadata.into_iter().map(|(k, v)| {
                (k, json_to_sql_value(v))
            }).collect(),
            vector_count: collection.vector_count,
            index_type: collection.index_type,
            storage_config: collection.storage_config.into_iter().map(|(k, v)| {
                (k, json_to_sql_value(v))
            }).collect(),
            quantization_config: collection.quantization_config.map(|qc| proximadb_v1::QuantizationConfig {
                quantization_type: qc.quantization_type,
                bits: qc.bits,
                training_sample_size: qc.training_sample_size,
            }),
        }
    }
}
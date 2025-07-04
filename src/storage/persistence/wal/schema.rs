// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Common Avro schema and conversion functions for WAL operations
//! This module consolidates all schema definitions to avoid inconsistency issues

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{WalEntry, WalOperation};
use super::avro::{AvroWalEntry, AvroWalOperation, AvroOpType};
use crate::core::{CollectionId, VectorId, VectorRecord};

/// Canonical Avro schema for WAL entries (Version 1)
/// This schema MUST be kept in sync with the AvroWalEntry struct definition
pub const AVRO_SCHEMA_V1: &str = r#"
{
  "type": "record",
  "name": "WalEntry",
  "namespace": "ai.proximadb.wal",
  "fields": [
    {"name": "entry_id", "type": "string"},
    {"name": "collection_id", "type": "string"},
    {"name": "operation", "type": {
      "type": "record",
      "name": "WalOperation",
      "fields": [
        {"name": "op_type", "type": {"type": "enum", "name": "OpType", "symbols": ["INSERT", "UPDATE", "DELETE", "CREATE_COLLECTION", "DROP_COLLECTION"]}},
        {"name": "vector_id", "type": ["null", "string"], "default": null},
        {"name": "vector_data", "type": ["null", "bytes"], "default": null},
        {"name": "metadata", "type": ["null", "string"], "default": null},
        {"name": "config", "type": ["null", "string"], "default": null},
        {"name": "expires_at", "type": ["null", "long"], "default": null}
      ]
    }},
    {"name": "timestamp", "type": "long"},
    {"name": "sequence", "type": "long"},
    {"name": "global_sequence", "type": "long"},
    {"name": "expires_at", "type": ["null", "long"], "default": null},
    {"name": "version", "type": "long", "default": 1}
  ]
}
"#;

/// Avro schema for vector batch insert payload (gRPC zero-copy)
/// This schema defines the structure of vectors_avro_payload in VectorInsertRequest
pub const VECTOR_BATCH_SCHEMA_V1: &str = r#"
{
  "type": "record",
  "name": "VectorBatch",
  "namespace": "ai.proximadb.vectors",
  "fields": [
    {"name": "vectors", "type": {
      "type": "array",
      "items": {
        "type": "record",
        "name": "Vector",
        "fields": [
          {"name": "id", "type": "string"},
          {"name": "vector", "type": {"type": "array", "items": "float"}},
          {"name": "metadata", "type": ["null", {"type": "map", "values": "string"}], "default": null},
          {"name": "timestamp", "type": ["null", "long"], "default": null}
        ]
      }
    }}
  ]
}
"#;

/// Avro structures for zero-copy vector batch deserialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroVectorBatch {
    pub vectors: Vec<AvroVector>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroVector {
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: Option<HashMap<String, String>>,
    pub timestamp: Option<i64>,
}

/// Deserialize Avro binary vector batch payload from gRPC (zero-copy)
pub fn deserialize_vector_batch(avro_payload: &[u8]) -> Result<Vec<VectorRecord>> {
    use apache_avro::Schema;
    
    // Parse the vector batch schema
    let schema = Schema::parse_str(VECTOR_BATCH_SCHEMA_V1)
        .context("Failed to parse vector batch Avro schema")?;
    
    // Deserialize from Avro binary datum (schema-less, matches to_avro_datum)
    let mut reader = std::io::Cursor::new(avro_payload);
    let avro_value = apache_avro::from_avro_datum(&schema, &mut reader, None)
        .context("Failed to deserialize Avro vector batch datum")?;
    
    // Convert Avro Value to our struct
    let avro_batch: AvroVectorBatch = apache_avro::from_value::<AvroVectorBatch>(&avro_value)
        .context("Failed to convert Avro value to AvroVectorBatch")?;
    
    let mut vector_records = Vec::new();
    
    // Process each vector in the batch
    for avro_vector in avro_batch.vectors {
        let timestamp_ms = avro_vector.timestamp.unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        
        // Convert metadata from Option<HashMap<String, String>> to HashMap<String, serde_json::Value>
        let metadata: HashMap<String, serde_json::Value> = avro_vector.metadata
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
            .collect();
        
        let record = VectorRecord {
            id: avro_vector.id.clone(),
            collection_id: String::new(), // Will be set by caller
            vector: avro_vector.vector,
            metadata,
            timestamp: timestamp_ms,
            created_at: timestamp_ms,
            updated_at: timestamp_ms,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        
        vector_records.push(record);
    }
    
    Ok(vector_records)
}

/// Convert WAL entry to Avro format using the canonical schema
/// This is the single source of truth for WAL entry conversion
pub fn convert_to_avro_entry(entry: &WalEntry) -> Result<AvroWalEntry> {
    let operation = match &entry.operation {
        WalOperation::Insert {
            vector_id,
            record,
            expires_at,
        } => AvroWalOperation {
            op_type: AvroOpType::Insert,
            vector_id: Some(vector_id.to_string()),
            vector_data: Some(serialize_vector_record(record)?),
            metadata: None,
            config: None,
            expires_at: expires_at.map(|dt| dt.timestamp_millis()),
        },
        WalOperation::Update {
            vector_id,
            record,
            expires_at,
        } => AvroWalOperation {
            op_type: AvroOpType::Update,
            vector_id: Some(vector_id.to_string()),
            vector_data: Some(serialize_vector_record(record)?),
            metadata: None,
            config: None,
            expires_at: expires_at.map(|dt| dt.timestamp_millis()),
        },
        WalOperation::Delete {
            vector_id,
            expires_at,
        } => AvroWalOperation {
            op_type: AvroOpType::Delete,
            vector_id: Some(vector_id.to_string()),
            vector_data: None,
            metadata: None,
            config: None,
            expires_at: expires_at.map(|dt| dt.timestamp_millis()),
        },
        WalOperation::AvroPayload {
            operation_type: _,
            avro_data,
        } => AvroWalOperation {
            op_type: AvroOpType::Insert, // Use Insert as default for binary Avro data
            vector_id: None,
            vector_data: Some(avro_data.clone()),
            metadata: None,
            config: None,
            expires_at: None,
        },
        WalOperation::Flush => AvroWalOperation {
            op_type: AvroOpType::Insert, // Use Insert as default for system operations
            vector_id: None,
            vector_data: None,
            metadata: Some("FLUSH".to_string()),
            config: None,
            expires_at: None,
        },
        WalOperation::Checkpoint => AvroWalOperation {
            op_type: AvroOpType::Insert, // Use Insert as default for system operations
            vector_id: None,
            vector_data: None,
            metadata: Some("CHECKPOINT".to_string()),
            config: None,
            expires_at: None,
        },
    };

    Ok(AvroWalEntry {
        entry_id: entry.entry_id.to_string(),
        collection_id: entry.collection_id.to_string(),
        operation,
        timestamp: entry.timestamp.timestamp_millis(),
        sequence: entry.sequence as i64,
        global_sequence: entry.global_sequence as i64,
        expires_at: entry.expires_at.map(|dt| dt.timestamp_millis()),
        version: entry.version as i64,
    })
}

/// Convert from Avro format to WAL entry using the canonical schema
pub fn convert_from_avro_entry(avro_entry: AvroWalEntry) -> Result<WalEntry> {
    let operation = match avro_entry.operation.op_type {
        AvroOpType::Insert => {
            let vector_id = VectorId::from(
                avro_entry
                    .operation
                    .vector_id
                    .context("Missing vector_id for Insert")?,
            );
            let record = deserialize_vector_record(
                avro_entry
                    .operation
                    .vector_data
                    .context("Missing vector_data for Insert")?,
            )?;
            let expires_at = avro_entry
                .operation
                .expires_at
                .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now()));

            WalOperation::Insert {
                vector_id,
                record,
                expires_at,
            }
        }
        AvroOpType::Update => {
            let vector_id = VectorId::from(
                avro_entry
                    .operation
                    .vector_id
                    .context("Missing vector_id for Update")?,
            );
            let record = deserialize_vector_record(
                avro_entry
                    .operation
                    .vector_data
                    .context("Missing vector_data for Update")?,
            )?;
            let expires_at = avro_entry
                .operation
                .expires_at
                .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now()));

            WalOperation::Update {
                vector_id,
                record,
                expires_at,
            }
        }
        AvroOpType::Delete => {
            let vector_id = VectorId::from(
                avro_entry
                    .operation
                    .vector_id
                    .context("Missing vector_id for Delete")?,
            );
            let expires_at = avro_entry
                .operation
                .expires_at
                .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now()));

            WalOperation::Delete {
                vector_id,
                expires_at,
            }
        }
    };

    Ok(WalEntry {
        entry_id: avro_entry.entry_id,
        collection_id: CollectionId::from(avro_entry.collection_id),
        operation,
        timestamp: DateTime::from_timestamp_millis(avro_entry.timestamp)
            .unwrap_or_else(|| Utc::now()),
        sequence: avro_entry.sequence as u64,
        global_sequence: avro_entry.global_sequence as u64,
        expires_at: avro_entry
            .expires_at
            .map(|ts| DateTime::from_timestamp_millis(ts).unwrap_or_else(|| Utc::now())),
        version: avro_entry.version as u64,
    })
}

/// Serialize vector record to bytes using bincode
pub fn serialize_vector_record(record: &VectorRecord) -> Result<Vec<u8>> {
    bincode::serialize(record).context("Failed to serialize VectorRecord")
}

/// Deserialize vector record from bytes using bincode
pub fn deserialize_vector_record(data: Vec<u8>) -> Result<VectorRecord> {
    bincode::deserialize(&data).context("Failed to deserialize VectorRecord")
}
/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! VectorRecord Migration Utilities
//!
//! This module provides utilities for migrating from Avro VectorRecord to Proto VectorRecord.
//! It serves as a compatibility layer during the transition period.

use crate::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;

/// Avro VectorRecord type alias for compatibility
pub type ServiceVectorRecord = crate::core::service_types::VectorRecord;

/// Proto VectorRecord type alias for convenience
pub type ProtoVectorRecord = VectorRecord;

/// Convert Service VectorRecord to Proto VectorRecord
pub fn service_to_proto(
    service_record: &ServiceVectorRecord,
    _collection_id: &str,
) -> ProtoVectorRecord {
    // Convert metadata from HashMap<String, serde_json::Value> to HashMap<String, SqlValue>
    let metadata: HashMap<String, crate::proto::proximadb_v1::SqlValue> = service_record
        .metadata
        .iter()
        .map(|(key, value)| {
            let sql_value = match value {
                serde_json::Value::String(s) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s.clone())),
                },
                serde_json::Value::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(f)),
                        }
                    } else {
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(n.to_string())),
                        }
                    }
                }
                serde_json::Value::Bool(b) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(*b)),
                },
                _ => crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(value.to_string())),
                },
            };
            (key.clone(), sql_value)
        })
        .collect();

    ProtoVectorRecord {
        id: service_record.id.clone(),
        vector: service_record.vector.clone(),
        metadata,
        timestamp: (service_record.timestamp / 1_000_000) as i64, // Convert microseconds to seconds
        updated_at: service_record.updated_at.map(|v| (v / 1_000_000) as i64),
        expires_at: service_record.expires_at.map(|v| (v / 1_000_000) as i64),
        version: service_record.version.map(|v| v as i64),
        quantized_vector: Vec::new(),
        source: None,
    }
}

/// Convert Proto VectorRecord to Service VectorRecord
pub fn proto_to_service(
    proto_record: &ProtoVectorRecord,
    collection_id: &str,
) -> ServiceVectorRecord {
    // Convert metadata from Vec<MetadataItem> to HashMap<String, serde_json::Value>
    let metadata =
        crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(&proto_record.metadata);

    ServiceVectorRecord {
        id: proto_record.id.clone(),
        collection_id: collection_id.to_string(),
        vector: proto_record.vector.clone(),
        metadata,
        timestamp: (proto_record.timestamp as i64) * 1_000_000, // Convert seconds to microseconds
        updated_at: Some(
            proto_record
                .updated_at
                .map(|v| (v as i64) * 1_000_000)
                .unwrap_or_else(|| chrono::Utc::now().timestamp_micros()),
        ),
        expires_at: proto_record.expires_at.map(|v| (v as i64) * 1_000_000),
        version: proto_record.version.map(|v| v as i64),
        // Note: similarity field removed - only exists on SearchVectorRecord
    }
}

/// Convert a batch of Service VectorRecords to Proto VectorRecords
pub fn service_batch_to_proto(
    service_records: &[ServiceVectorRecord],
    collection_id: &str,
) -> Vec<ProtoVectorRecord> {
    service_records
        .iter()
        .map(|r| service_to_proto(r, collection_id))
        .collect()
}

/// Convert a batch of Proto VectorRecords to Service VectorRecords
pub fn proto_batch_to_service(
    proto_records: &[ProtoVectorRecord],
    collection_id: &str,
) -> Vec<ServiceVectorRecord> {
    proto_records
        .iter()
        .map(|r| proto_to_service(r, collection_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_avro_to_proto_conversion() {
        let mut metadata = HashMap::new();
        metadata.insert(
            "category".to_string(),
            serde_json::Value::String("test".to_string()),
        );
        metadata.insert(
            "score".to_string(),
            serde_json::Value::Number(serde_json::Number::from(42)),
        );
        metadata.insert("active".to_string(), serde_json::Value::Bool(true));

        let service_record = ServiceVectorRecord {
            id: "test-vector-1".to_string(),
            collection_id: "test-collection".to_string(),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata,
            timestamp: 1640995200000000, // 2022-01-01 00:00:00 UTC in microseconds
            updated_at: Some(1640995200000000),
            expires_at: None,
            version: Some(1),
        };

        let proto_record = service_to_proto(&service_record, "test-collection");

        assert_eq!(proto_record.id, "test-vector-1".to_string());
        assert_eq!(proto_record.vector, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(proto_record.timestamp, 1640995200); // Converted from microseconds to seconds
        assert_eq!(proto_record.version, Some(1));
        assert_eq!(proto_record.metadata.len(), 3);

        // Check metadata items
        let metadata_items = &proto_record.metadata;
        assert!(metadata_items.iter().any(|(key, sql_value)| key == "category" &&
            matches!(&sql_value.value, Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) if s == "test")));
        assert!(metadata_items.iter().any(|(key, sql_value)| key == "score" &&
            matches!(&sql_value.value, Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) if *n == 42.0)));
        assert!(metadata_items.iter().any(|(key, sql_value)| key == "active" &&
            matches!(&sql_value.value, Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) if *b)));
    }

    #[test]
    fn test_proto_to_avro_conversion() {
        use crate::proto::proximadb_v1::MetadataItem;

        let metadata = vec![
            MetadataItem {
                key: "category".to_string(),
                value: Some(
                    crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                        "test".to_string(),
                    ),
                ),
            },
            MetadataItem {
                key: "score".to_string(),
                value: Some(
                    crate::proto::proximadb_v1::metadata_item::Value::StringValue("42".to_string()),
                ),
            },
        ];

        let proto_record = ProtoVectorRecord {
            id: "test-vector-1".to_string(),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata: std::collections::HashMap::new(), // Use HashMap for new format
            timestamp: 1640995200, // Converted from microseconds to seconds
            updated_at: Some(1640995200),
            expires_at: None,
            version: Some(1),
            quantized_vector: Vec::new(),
            source: Some("test".to_string()),
        };

        let service_record = proto_to_service(&proto_record, "test-collection");

        assert_eq!(service_record.id, "test-vector-1");
        assert_eq!(service_record.collection_id, "test-collection");
        assert_eq!(service_record.vector, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(service_record.timestamp, 1640995200000000);
        assert_eq!(service_record.version, Some(1));
        assert_eq!(service_record.metadata.len(), 2);
        assert!(service_record.metadata.contains_key("category"));
        assert!(service_record.metadata.contains_key("score"));
    }
}

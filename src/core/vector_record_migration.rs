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

use std::collections::HashMap;
use crate::proto::proximadb::VectorRecord;

/// Legacy Avro VectorRecord type alias for compatibility
pub type AvroVectorRecord = crate::core::avro_unified::VectorRecord;

/// Proto VectorRecord type alias for convenience
pub type ProtoVectorRecord = VectorRecord;

/// Convert Avro VectorRecord to Proto VectorRecord
pub fn avro_to_proto(avro_record: &AvroVectorRecord, _collection_id: &str) -> ProtoVectorRecord {
    // Convert metadata from HashMap<String, serde_json::Value> to Vec<MetadataItem>
    let metadata: Vec<crate::proto::proximadb::MetadataItem> = avro_record.metadata.iter()
        .map(|(key, value)| {
            let string_value = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => value.to_string()
            };
            crate::proto::proximadb::MetadataItem {
                key: key.clone(),
                value: string_value,
            }
        })
        .collect();

    ProtoVectorRecord {
        id: if avro_record.id.is_empty() { None } else { Some(avro_record.id.clone()) },
        vector: avro_record.vector.clone(),
        metadata,
        timestamp: avro_record.timestamp,
        created_at: avro_record.timestamp,
        updated_at: avro_record.timestamp,
        expires_at: avro_record.expires_at,
        version: avro_record.version,
        rank: None,
        score: None,
        distance: None,
    }
}

/// Convert Proto VectorRecord to Avro VectorRecord
pub fn proto_to_avro(proto_record: &ProtoVectorRecord, collection_id: &str) -> AvroVectorRecord {
    // Convert metadata from Vec<MetadataItem> to HashMap<String, serde_json::Value>
    let metadata: HashMap<String, serde_json::Value> = proto_record.metadata.iter()
        .map(|item| {
            (item.key.clone(), serde_json::Value::String(item.value.clone()))
        })
        .collect();

    AvroVectorRecord {
        id: proto_record.id.clone().unwrap_or_default(),
        collection_id: collection_id.to_string(),
        vector: proto_record.vector.clone(),
        metadata,
        timestamp: proto_record.timestamp,
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: proto_record.expires_at,
        version: proto_record.version,
        rank: None,
        score: None,
        distance: None,
    }
}

/// Convert a batch of Avro VectorRecords to Proto VectorRecords
pub fn avro_batch_to_proto(avro_records: &[AvroVectorRecord], collection_id: &str) -> Vec<ProtoVectorRecord> {
    avro_records.iter().map(|r| avro_to_proto(r, collection_id)).collect()
}

/// Convert a batch of Proto VectorRecords to Avro VectorRecords
pub fn proto_batch_to_avro(proto_records: &[ProtoVectorRecord], collection_id: &str) -> Vec<AvroVectorRecord> {
    proto_records.iter().map(|r| proto_to_avro(r, collection_id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_avro_to_proto_conversion() {
        let mut metadata = HashMap::new();
        metadata.insert("category".to_string(), serde_json::Value::String("test".to_string()));
        metadata.insert("score".to_string(), serde_json::Value::Number(serde_json::Number::from(42)));
        metadata.insert("active".to_string(), serde_json::Value::Bool(true));

        let avro_record = AvroVectorRecord {
            id: "test-vector-1".to_string(),
            collection_id: "test-collection".to_string(),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata,
            timestamp: 1640995200000000, // 2022-01-01 00:00:00 UTC in microseconds
            created_at: 1640995200000,   // 2022-01-01 00:00:00 UTC in milliseconds
            updated_at: 1640995200000,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };

        let proto_record = avro_to_proto(&avro_record, "test-collection");

        assert_eq!(proto_record.id, Some("test-vector-1".to_string()));
        assert_eq!(proto_record.vector, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(proto_record.timestamp, 1640995200000000);
        assert_eq!(proto_record.version, 1);
        assert_eq!(proto_record.metadata.len(), 3);
        
        // Check metadata items
        let metadata_items = &proto_record.metadata;
        assert!(metadata_items.iter().any(|item| item.key == "category" && item.value == "test"));
        assert!(metadata_items.iter().any(|item| item.key == "score" && item.value == "42"));
        assert!(metadata_items.iter().any(|item| item.key == "active" && item.value == "true"));
    }

    #[test]
    fn test_proto_to_avro_conversion() {
        use crate::proto::proximadb::MetadataItem;

        let metadata = vec![
            MetadataItem {
                key: "category".to_string(),
                value: "test".to_string(),
            },
            MetadataItem {
                key: "score".to_string(),
                value: "42".to_string(),
            },
        ];

        let proto_record = ProtoVectorRecord {
            id: Some("test-vector-1".to_string()),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata,
            timestamp: 1640995200000000,
            created_at: 1640995200000,
            updated_at: 1640995200000,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };

        let avro_record = proto_to_avro(&proto_record, "test-collection");

        assert_eq!(avro_record.id, "test-vector-1");
        assert_eq!(avro_record.collection_id, "test-collection");
        assert_eq!(avro_record.vector, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(avro_record.timestamp, 1640995200000000);
        assert_eq!(avro_record.version, 1);
        assert_eq!(avro_record.metadata.len(), 2);
        assert!(avro_record.metadata.contains_key("category"));
        assert!(avro_record.metadata.contains_key("score"));
    }
}
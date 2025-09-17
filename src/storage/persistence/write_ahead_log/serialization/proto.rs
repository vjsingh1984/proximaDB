//! Protocol Buffers serialization for WAL
//!
//! This is the default and recommended format for ProximaDB's proto-first architecture.

use crate::core::VectorRecord;
use anyhow::Result;
use prost::Message;

/// Protocol Buffers serializer - the default for proto-first architecture
#[derive(Debug, Clone, Default)]
pub struct ProtocolBuffersSerializer;

impl ProtocolBuffersSerializer {
    /// Create a new Protocol Buffers serializer
    pub fn new() -> Self {
        Self::default()
    }
}

/// Proto-based vector batch wrapper for WAL operations
#[derive(Clone, Message)]
struct ProtoVectorBatch {
    /// Batch of vector records
    #[prost(message, repeated, tag = "1")]
    pub vectors: Vec<crate::proto::proximadb_v1::VectorRecord>,

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

impl super::VectorBatchSerializer for ProtocolBuffersSerializer {
    fn serialize_batch(&self, vectors: &[VectorRecord]) -> Result<Vec<u8>> {
        // Create proto batch wrapper
        let batch = ProtoVectorBatch {
            vectors: vectors.to_vec(),
            batch_id: Some(format!("batch_{}", chrono::Utc::now().timestamp_micros())),
            timestamp: chrono::Utc::now().timestamp_micros(),
            collection_id: String::new(), // Collection ID is managed externally
        };

        // Serialize using protobuf
        let mut buf = Vec::new();
        batch
            .encode(&mut buf)
            .map_err(|e| anyhow::anyhow!("Failed to encode proto vector batch: {}", e))?;

        Ok(buf)
    }

    fn deserialize_batch(&self, data: &[u8]) -> Result<Vec<VectorRecord>> {
        // Deserialize protobuf
        let batch = ProtoVectorBatch::decode(data)
            .map_err(|e| anyhow::anyhow!("Failed to decode proto vector batch: {}", e))?;

        // Direct return - VectorRecord is already proto type in proto-first architecture
        Ok(batch.vectors)
    }

    fn format(&self) -> super::SerializationFormat {
        super::SerializationFormat::ProtocolBuffers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::write_ahead_log::serialization::VectorBatchSerializer;

    fn create_test_vector() -> VectorRecord {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            "category".to_string(),
            crate::proto::proximadb_v1::SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    "test".to_string(),
                )),
            },
        );

        VectorRecord {
            id: "test_vector_1".to_string(),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            metadata,
            timestamp: 1234567890,
            updated_at: Some(1234567890),
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        }
    }

    #[test]
    fn test_protocol_buffers_round_trip() {
        let serializer = ProtocolBuffersSerializer::new();
        let vectors = vec![create_test_vector()];

        // Serialize
        let serialized = serializer
            .serialize_batch(&vectors)
            .expect("Failed to serialize batch");
        assert!(!serialized.is_empty());

        // Deserialize
        let deserialized = serializer
            .deserialize_batch(&serialized)
            .expect("Failed to deserialize batch");
        assert_eq!(deserialized.len(), 1);
        assert_eq!(deserialized[0].id, vectors[0].id);
        assert_eq!(deserialized[0].vector, vectors[0].vector);
    }

    #[test]
    fn test_empty_batch_handling() {
        let serializer = ProtocolBuffersSerializer::new();
        let vectors = vec![];

        let serialized = serializer
            .serialize_batch(&vectors)
            .expect("Failed to serialize empty batch");
        let deserialized = serializer
            .deserialize_batch(&serialized)
            .expect("Failed to deserialize empty batch");

        assert!(deserialized.is_empty());
    }

    #[test]
    fn test_format_identifier() {
        let serializer = ProtocolBuffersSerializer::new();
        assert_eq!(
            serializer.format(),
            super::super::SerializationFormat::ProtocolBuffers
        );
    }
}

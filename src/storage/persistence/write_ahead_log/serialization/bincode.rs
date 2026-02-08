//! Bincode serialization for WAL
//!
//! Provides maximum performance for native Rust serialization.

use crate::proto::proximadb_v1::VectorRecord;
use anyhow::{Context, Result};

/// Bincode serializer - optimized for performance
#[derive(Debug, Clone, Default)]
pub struct BincodeSerializer;

impl BincodeSerializer {
    /// Create a new Bincode serializer
    pub fn new() -> Self {
        Self::default()
    }
}

impl super::VectorBatchSerializer for BincodeSerializer {
    fn serialize_batch(&self, vectors: &[VectorRecord]) -> Result<Vec<u8>> {
        bincode::serialize(vectors).context("Failed to serialize vectors to Bincode format")
    }

    fn deserialize_batch(&self, data: &[u8]) -> Result<Vec<VectorRecord>> {
        bincode::deserialize(data).context("Failed to deserialize Bincode vectors")
    }

    fn format(&self) -> super::SerializationFormat {
        super::SerializationFormat::Bincode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_vector() -> VectorRecord {
        VectorRecord {
            id: "test_vector_1".to_string(),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(1234567890),
            updated_at: Some(1234567890),
            expires_at: None,
            version: Some(1),
            source: Some("test".to_string()),
        }
    }

    #[test]
    fn test_bincode_round_trip() {
        let serializer = BincodeSerializer::new();
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
    fn test_multiple_vectors_batch() {
        let serializer = BincodeSerializer::new();
        let vectors = vec![
            create_test_vector(),
            create_test_vector(),
            create_test_vector(),
        ];

        let serialized = serializer
            .serialize_batch(&vectors)
            .expect("Failed to serialize batch");
        let deserialized = serializer
            .deserialize_batch(&serialized)
            .expect("Failed to deserialize batch");

        assert_eq!(deserialized.len(), 3);
    }

    #[test]
    fn test_high_dimensional_vector() {
        let serializer = BincodeSerializer::new();
        let mut vector = create_test_vector();
        vector.vector = vec![0.1; 1024]; // 1024-dimensional vector

        let vectors = vec![vector];
        let serialized = serializer
            .serialize_batch(&vectors)
            .expect("Failed to serialize high-dimensional vector");
        let deserialized = serializer
            .deserialize_batch(&serialized)
            .expect("Failed to deserialize high-dimensional vector");

        assert_eq!(deserialized[0].vector.len(), 1024);
    }

    #[test]
    fn test_format_identifier() {
        let serializer = BincodeSerializer::new();
        assert_eq!(
            serializer.format(),
            super::super::SerializationFormat::Bincode
        );
    }
}

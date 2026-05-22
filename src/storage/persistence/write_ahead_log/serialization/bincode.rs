//! Bincode serialization for WAL
//!
//! Provides maximum performance for native Rust serialization.

use anyhow::{Context, Result};
use proximadb_records::ProximaRecord;

/// Bincode serializer - optimized for performance
#[derive(Debug, Clone, Default)]
pub struct BincodeSerializer;

impl BincodeSerializer {
    /// Create a new Bincode serializer
    pub fn new() -> Self {
        Self
    }
}

impl super::VectorBatchSerializer for BincodeSerializer {
    fn serialize_batch(&self, records: &[ProximaRecord]) -> Result<Vec<u8>> {
        bincode::serialize(records).context("Failed to serialize ProximaRecords to Bincode format")
    }

    fn deserialize_batch(&self, data: &[u8]) -> Result<Vec<ProximaRecord>> {
        bincode::deserialize(data).context("Failed to deserialize Bincode ProximaRecords")
    }

    fn format(&self) -> super::SerializationFormat {
        super::SerializationFormat::Bincode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistence::write_ahead_log::serialization::VectorBatchSerializer;
    use proximadb_records::{EmbeddingCell, ProximaRecord};

    fn create_test_vector() -> ProximaRecord {
        ProximaRecord {
            oid: "test_vector_1".to_string(),
            embeddings: vec![EmbeddingCell {
                model_id: "default".to_string(),
                modality: "vector".to_string(),
                values: vec![0.1, 0.2, 0.3, 0.4],
                dim: 4,
                ..Default::default()
            }],
            origin: Some("test".to_string()),
            record_version: 1,
            ..Default::default()
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
        assert_eq!(deserialized[0].oid, vectors[0].oid);
        assert_eq!(
            deserialized[0].embeddings.first().map(|e| &e.values),
            vectors[0].embeddings.first().map(|e| &e.values)
        );
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
        vector.embeddings = vec![EmbeddingCell {
            model_id: "default".to_string(),
            modality: "vector".to_string(),
            values: vec![0.1; 1024],
            dim: 1024,
            ..Default::default()
        }];

        let vectors = vec![vector];
        let serialized = serializer
            .serialize_batch(&vectors)
            .expect("Failed to serialize high-dimensional vector");
        let deserialized = serializer
            .deserialize_batch(&serialized)
            .expect("Failed to deserialize high-dimensional vector");

        assert_eq!(
            deserialized[0].embeddings.first().map(|e| e.values.len()),
            Some(1024)
        );
    }

    #[test]
    fn test_format_identifier() {
        let serializer = BincodeSerializer::new();
        assert_eq!(
            serializer.format(),
            super::super::SerializationFormat::Bincode
        );
    }

    // === PR 2: schema_version dispatch (LLD §schema-version-dispatch) ===

    #[test]
    fn deserialize_batch_default_stamps_v1() {
        let serializer = BincodeSerializer::new();
        let bytes = serializer.serialize_batch(&[create_test_vector()]).unwrap();
        let records = serializer.deserialize_batch(&bytes).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].schema_version,
            proximadb_records::schema_version::V1,
            "PR 2 WAL frames must read as V1 by default"
        );
    }

    #[test]
    fn deserialize_batch_with_v1_hint_stamps_v1() {
        let serializer = BincodeSerializer::new();
        let bytes = serializer.serialize_batch(&[create_test_vector()]).unwrap();
        let records = serializer
            .deserialize_batch_with_schema_version(
                &bytes,
                proximadb_records::schema_version::V1,
            )
            .unwrap();
        assert_eq!(records[0].schema_version, proximadb_records::schema_version::V1);
    }

    #[test]
    fn deserialize_batch_with_v2_hint_stamps_v2() {
        // PR 2: on-disk format is unchanged (writer is still V1). The hint is
        // what the caller (segment header in PR 4) declares the segment to
        // be. Stamping V2 here is the dispatch site future PRs will use to
        // pick a precision-aware decoder.
        let serializer = BincodeSerializer::new();
        let bytes = serializer.serialize_batch(&[create_test_vector()]).unwrap();
        let records = serializer
            .deserialize_batch_with_schema_version(
                &bytes,
                proximadb_records::schema_version::V2,
            )
            .unwrap();
        assert_eq!(records[0].schema_version, proximadb_records::schema_version::V2);
    }

    #[test]
    fn mixed_v1_v2_segments_read_independently() {
        // Simulates PR 4's mixed-segment-reader contract: two batches written
        // identically (PR 2 keeps the writer on V1), but each read dispatch
        // can label them with their declared schema version.
        let serializer = BincodeSerializer::new();
        let bytes_a = serializer.serialize_batch(&[create_test_vector()]).unwrap();
        let bytes_b = serializer.serialize_batch(&[create_test_vector()]).unwrap();

        let v1_records = serializer
            .deserialize_batch_with_schema_version(
                &bytes_a,
                proximadb_records::schema_version::V1,
            )
            .unwrap();
        let v2_records = serializer
            .deserialize_batch_with_schema_version(
                &bytes_b,
                proximadb_records::schema_version::V2,
            )
            .unwrap();
        assert_eq!(v1_records[0].schema_version, proximadb_records::schema_version::V1);
        assert_eq!(v2_records[0].schema_version, proximadb_records::schema_version::V2);
        // Payload is structurally identical — PR 4 will change this when the
        // v2 path swaps in the EmbeddingValues decoder.
        assert_eq!(
            v1_records[0].embeddings[0].values,
            v2_records[0].embeddings[0].values
        );
    }
}

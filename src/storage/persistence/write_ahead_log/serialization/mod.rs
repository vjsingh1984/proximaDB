//! Pure serialization layer for WAL operations
//!
//! This module provides clean serialization/deserialization interfaces
//! without any I/O operations, memtable management, or other concerns.

use anyhow::Result;
use proximadb_records::ProximaRecord;

/// Trait for vector batch serialization
///
/// Implementations should ONLY handle data format conversion,
/// not I/O, memtable operations, or any other concerns.
pub trait VectorBatchSerializer: Send + Sync {
    /// Convert a batch of canonical records to serialized bytes.
    ///
    /// PR 2 of the embedding-precision rollout: writers ignore
    /// `schema_version` (the field is `serde(skip)` on ProximaRecord). PR 3
    /// adds a feature-flag-gated v2 writer that prepends a schema byte.
    fn serialize_batch(&self, records: &[ProximaRecord]) -> Result<Vec<u8>>;

    /// Convert serialized bytes back to canonical records.
    ///
    /// Default behavior stamps every record with `schema_version::V1` because
    /// PR 2 WAL frames are bytewise-identical to PR 0. Use
    /// [`Self::deserialize_batch_with_schema_version`] when dispatching from
    /// a v2 segment header (PR 4) or an explicit version-aware reader.
    fn deserialize_batch(&self, data: &[u8]) -> Result<Vec<ProximaRecord>>;

    /// Get the serialization format identifier.
    fn format(&self) -> SerializationFormat;

    /// PR 2 §schema-version-dispatch: deserialize a batch and stamp every
    /// returned record with `schema_version`.
    ///
    /// * `schema_version::V1` — legacy fp32 records. Behavior identical to
    ///   [`Self::deserialize_batch`] because PR 2 storage is still
    ///   `Vec<f32>`.
    /// * `schema_version::V2` — precision-aware records. PR 2 returns
    ///   structurally-identical records (no on-disk format change); PR 4+
    ///   will wire this branch to the `EmbeddingValues` decoder once the
    ///   v2 segment header lands.
    fn deserialize_batch_with_schema_version(
        &self,
        data: &[u8],
        schema_version: u8,
    ) -> Result<Vec<ProximaRecord>> {
        let mut records = self.deserialize_batch(data)?;
        for r in &mut records {
            r.schema_version = schema_version;
        }
        Ok(records)
    }
}

/// Supported serialization formats
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SerializationFormat {
    /// Protocol Buffers - default for proto-first architecture
    ProtocolBuffers,
    /// Bincode - optimized for performance
    Bincode,
    /// Apache Avro - for schema evolution
    Avro,
}

impl SerializationFormat {
    /// Get string representation for logging and storage
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProtocolBuffers => "proto",
            Self::Bincode => "bincode",
            Self::Avro => "avro",
        }
    }

    /// Parse from string
    pub fn parse_format(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "proto" | "protobuf" | "protocol-buffers" => Ok(Self::ProtocolBuffers),
            "bincode" => Ok(Self::Bincode),
            "avro" => Ok(Self::Avro),
            _ => Err(anyhow::anyhow!("Unknown serialization format: {}", s)),
        }
    }
}

// Module exports
mod avro;
mod bincode;
mod proto;
pub use avro::AvroSerializer;
pub use bincode::BincodeSerializer;
pub use proto::ProtocolBuffersSerializer;

/// Factory to create serializers by format
pub struct SerializerFactory;

impl SerializerFactory {
    /// Create a new serializer for the specified format
    pub fn create(format: SerializationFormat) -> Box<dyn VectorBatchSerializer> {
        match format {
            SerializationFormat::ProtocolBuffers => Box::new(ProtocolBuffersSerializer::new()),
            SerializationFormat::Bincode => Box::new(BincodeSerializer::new()),
            SerializationFormat::Avro => Box::new(AvroSerializer::new()),
        }
    }

    /// Create a serializer from a format string
    pub fn from_string(format: &str) -> Result<Box<dyn VectorBatchSerializer>> {
        let format = SerializationFormat::parse_format(format)?;
        Ok(Self::create(format))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialization_format_conversion() {
        assert_eq!(
            SerializationFormat::parse_format("proto").unwrap(),
            SerializationFormat::ProtocolBuffers
        );
        assert_eq!(
            SerializationFormat::parse_format("bincode").unwrap(),
            SerializationFormat::Bincode
        );
        assert_eq!(
            SerializationFormat::parse_format("avro").unwrap(),
            SerializationFormat::Avro
        );
        assert!(SerializationFormat::parse_format("unknown").is_err());
    }

    #[test]
    fn test_format_string_representation() {
        assert_eq!(SerializationFormat::ProtocolBuffers.as_str(), "proto");
        assert_eq!(SerializationFormat::Bincode.as_str(), "bincode");
        assert_eq!(SerializationFormat::Avro.as_str(), "avro");
    }

    #[test]
    fn test_serializer_factory() {
        let proto_serializer = SerializerFactory::create(SerializationFormat::ProtocolBuffers);
        assert_eq!(
            proto_serializer.format(),
            SerializationFormat::ProtocolBuffers
        );

        let bincode_serializer = SerializerFactory::from_string("bincode").unwrap();
        assert_eq!(bincode_serializer.format(), SerializationFormat::Bincode);
    }
}

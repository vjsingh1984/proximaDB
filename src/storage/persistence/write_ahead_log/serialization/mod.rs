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

    /// v2 wire path serialize. Default delegates to v1 so serializers
    /// that haven't migrated keep working — but they'll re-trigger the
    /// v1 fp32-only refuse-error on non-Fp32 records. Bincode (and any
    /// other format that wants fp16 ingest) overrides this with a
    /// natural enum-aware encoding via
    /// `proximadb_records::wire_v2::ProximaRecordV2`.
    fn serialize_batch_v2(&self, records: &[ProximaRecord]) -> Result<Vec<u8>> {
        self.serialize_batch(records)
    }

    /// v2 wire path deserialize. Default delegates to v1.
    fn deserialize_batch_v2(&self, data: &[u8]) -> Result<Vec<ProximaRecord>> {
        self.deserialize_batch(data)
    }

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

    /// INT-2a (mini-phase EMBEDDING_PRECISION_INTEGRATION_PLAN): prepend
    /// the PR 4 v2 segment header to a serialized batch.
    ///
    /// Returns `header_bytes || serialize_batch(records)`. The caller
    /// (a disk-manager write path gated on `schema_v2_enabled`) writes
    /// the resulting blob to disk as one file. Readers use
    /// [`Self::deserialize_batch_auto`] to magic-peek + dispatch.
    ///
    /// Default impl is generic across serializers — it doesn't depend on
    /// the bincode shape. The header bytes only live in front of the
    /// payload; no inline framing changes.
    fn serialize_batch_with_v2_segment_header(
        &self,
        records: &[ProximaRecord],
        header: &crate::storage::persistence::write_ahead_log::v2_segment_header::V2SegmentHeader,
    ) -> Result<Vec<u8>> {
        let mut out = header.encode();
        // v2 segment header gates the v2 wire encoding — the per-record
        // shape (`ProximaRecordV2` with natural enum-aware embeddings
        // serde) is what makes fp16 / bf16 / int8 records durable; the
        // header alone just declares the schema bytes ahead.
        out.extend(self.serialize_batch_v2(records)?);
        Ok(out)
    }

    /// INT-2a: deserialize a batch with automatic v1/v2 dispatch.
    ///
    /// * If the bytes start with the PR 4 `PWAL` magic and declare
    ///   `version = 2`, the header is parsed, records are decoded from
    ///   the bytes after the header, and every record is stamped
    ///   `schema_version = V2`.
    /// * Otherwise the bytes are treated as legacy v1 (no header, raw
    ///   bincode batch) and records are stamped `schema_version = V1`.
    ///
    /// Returns `(records, parsed_header)` so the caller can use the
    /// header's `canonical_default_precision`, `policy_id`, and
    /// `precision_epoch` for downstream routing (INT-3+ PAX dispatch,
    /// recall-report tagging, etc.).
    fn deserialize_batch_auto(
        &self,
        data: &[u8],
    ) -> Result<(
        Vec<ProximaRecord>,
        Option<crate::storage::persistence::write_ahead_log::v2_segment_header::V2SegmentHeader>,
    )> {
        use crate::storage::persistence::write_ahead_log::v2_segment_header::{
            PWAL_MAGIC, PWAL_PEEK_LEN, PeekedSegmentVersion, V2SegmentHeader,
            peek_segment_version,
        };
        // Magic check is bounded: any blob shorter than 4 bytes or that
        // doesn't start with PWAL is treated as legacy v1 and handed to
        // the existing path. This keeps the dispatch safe for all
        // pre-INT-2a writers.
        if data.len() < PWAL_PEEK_LEN || &data[..4] != PWAL_MAGIC {
            let records = self.deserialize_batch_with_schema_version(
                data,
                proximadb_records::schema_version::V1,
            )?;
            return Ok((records, None));
        }
        match peek_segment_version(data)? {
            PeekedSegmentVersion::V1 => {
                // PWAL magic + version=1 is a future-reserved layout
                // (today's v1 writer never prepends magic). Treat as
                // legacy: pass the full blob through. Once that layout
                // ships we'll either route it here or evolve the magic.
                let records = self.deserialize_batch_with_schema_version(
                    data,
                    proximadb_records::schema_version::V1,
                )?;
                Ok((records, None))
            }
            PeekedSegmentVersion::V2 => {
                let (header, consumed) = V2SegmentHeader::decode(data)?;
                let payload = &data[consumed..];
                // v2 payload uses the v2 wire shape (natural enum-aware
                // embeddings). Decode through the typed path, then
                // stamp the schema_version field (serde-skip on
                // ProximaRecord, so the decoder always defaults it).
                let mut records = self.deserialize_batch_v2(payload)?;
                for r in &mut records {
                    r.schema_version = proximadb_records::schema_version::V2;
                }
                Ok((records, Some(header)))
            }
        }
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

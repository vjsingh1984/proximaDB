//! Pure serialization layer for WAL operations
//! 
//! This module provides clean serialization/deserialization interfaces
//! without any I/O operations, memtable management, or other concerns.

use anyhow::Result;
use crate::core::VectorRecord;

/// Trait for vector batch serialization
/// 
/// Implementations should ONLY handle data format conversion,
/// not I/O, memtable operations, or any other concerns.
pub trait VectorBatchSerializer: Send + Sync {
    /// Convert a batch of vector records to serialized bytes
    fn serialize_batch(&self, vectors: &[VectorRecord]) -> Result<Vec<u8>>;
    
    /// Convert serialized bytes back to vector records
    fn deserialize_batch(&self, data: &[u8]) -> Result<Vec<VectorRecord>>;
    
    /// Get the serialization format identifier
    fn format(&self) -> SerializationFormat;
}

/// Supported serialization formats
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_deref() {
            "proto" | "protobuf" | "protocol-buffers" => Ok(Self::ProtocolBuffers),
            "bincode" => Ok(Self::Bincode),
            "avro" => Ok(Self::Avro),
            _ => Err(anyhow::anyhow!("Unknown serialization format: {}", s)),
        }
    }
}

// Module exports
mod proto;
mod bincode;
mod avro;
pub use proto::ProtocolBuffersSerializer;
pub use bincode::BincodeSerializer;
pub use avro::AvroSerializer;

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
        let format = SerializationFormat::from_str(format)?;
        Ok(Self::create(format))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialization_format_conversion() {
        assert_eq!(SerializationFormat::from_str("proto").unwrap(), SerializationFormat::ProtocolBuffers);
        assert_eq!(SerializationFormat::from_str("bincode").unwrap(), SerializationFormat::Bincode);
        assert_eq!(SerializationFormat::from_str("avro").unwrap(), SerializationFormat::Avro);
        assert!(SerializationFormat::from_str("unknown").is_err());
    }

    #[test]
    fn test_format_string_representation() {
        assert_eq!(SerializationFormat::ProtocolBuffers.as_deref(), "proto");
        assert_eq!(SerializationFormat::Bincode.as_deref(), "bincode");
        assert_eq!(SerializationFormat::Avro.as_deref(), "avro");
    }

    #[test]
    fn test_serializer_factory() {
        let proto_serializer = SerializerFactory::create(SerializationFormat::ProtocolBuffers);
        assert_eq!(proto_serializer.format(), SerializationFormat::ProtocolBuffers);
        
        let bincode_serializer = SerializerFactory::from_string("bincode").unwrap();
        assert_eq!(bincode_serializer.format(), SerializationFormat::Bincode);
    }
}
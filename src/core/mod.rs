//! # Core Module - Foundation Types and Utilities
//!
//! This module provides the foundational types, configurations, and utilities that are used
//! throughout ProximaDB. It serves as the common vocabulary for all other modules, ensuring
//! consistency and type safety across the codebase.
//!
//! ## Role in ProximaDB Architecture
//!
//! The core module sits at the base of the dependency hierarchy:
//! ```text
//! All Modules
//!      ↓
//! Core Module (types, config, errors, utilities)
//! ```
//!
//! ## Key Components
//!
//! - **Types & Service Types**: Common type definitions (VectorRecord, Collection, etc.)
//! - **Configuration**: System configuration and loading
//! - **Error Handling**: Unified error types with `thiserror`
//! - **Search**: Core search functionality and result types
//! - **Compression**: Unified compression algorithms (LZ4, Snappy, Zstd, etc.)
//! - **Bloom Filters**: Probabilistic data structures for fast lookups
//! - **Hardware Capabilities**: SIMD/GPU detection and optimization
//! - **Memory Management**: Memory pools and allocation strategies

/// Core service types for vector operations (VectorRecord, Collection, etc.)
pub mod service_types;

/// Base62 encoding utilities for compact ID generation
pub mod base62;

/// System configuration structures and defaults
pub mod config;

/// Configuration loading from files and environment
pub mod config_loader;

/// Type conversions and proto-to-native mappings
pub mod conversions;

/// Migration utilities for vector record formats
pub mod vector_record_migration;

/// Legacy error module (being replaced by errors module)
pub mod error;

/// gRPC metadata parsing utilities
pub mod grpc_metadata_parser;

/// Index-related core types and traits
pub mod index;

/// Indexing configuration and strategies
pub mod indexing;

/// Metadata query parsing and execution
pub mod metadata_query;

/// Core search functionality, filters, and result types
pub mod search;

/// Serialization utilities for various formats
pub mod serialization;

/// Unified compression module with 13 algorithm support
pub mod compression;

/// Storage-related core types and traits
pub mod storage;

/// Foundation traits and base implementations
pub mod foundation;

/// Storage layout and path management
pub mod storage_layout;

/// Memory management, pools, and allocation strategies
pub mod memory;

/// Protocol buffer metadata helpers
pub mod proto_metadata_helper;

/// Bloom filter implementations for fast lookups
pub mod bloom;

/// Unified error types with thiserror
pub mod errors;

/// Hardware capability detection (SIMD, GPU, etc.)
pub mod hardware_capabilities;

/// Ultra-efficient enum packing for 75% storage savings
pub mod enum_packing;

/// Common utility functions (metadata conversion, vector ops, validation)
pub mod utils;

#[cfg(test)]
mod config_tests;

#[cfg(test)]
mod error_tests;

#[cfg(test)]
mod config_loader_tests;

#[cfg(test)]
mod flush_config_tests;

pub use config::*;
pub use config_loader::*;
pub use error::*;
// Core service types for vector operations
pub use service_types::{
    BatchSearchRequest, CollectionConfig, CollectionOperation, CollectionRequest,
    CollectionResponse, CompactionConfig, CompactionStrategy, CompressionAlgorithm, DistanceMetric,
    FieldCondition, HealthResponse, IndexStats, IndexingAlgorithm, MetadataFilter, MetricsResponse,
    NodeId, OperationResponse, SearchDebugInfo, SearchMetadata, SearchRequest, SearchStrategy,
    ServiceMetrics, StorageEngine, String, Vector, VectorId, VectorInsertRequest,
    VectorInsertResponse, VectorOperation, VectorOperationMetrics, VectorSearchRequest,
    VectorSearchResponse, WriteBufferMetrics,
};

// PROTO-FIRST ARCHITECTURE: Direct proto usage for zero overhead
// No conversions, no adapters, no memory duplication

/// VectorRecord is now a direct type alias to ProtoVectorRecord
/// This achieves true proto-first architecture with:
/// - Zero overhead (no enum dispatch, no conversions)
/// - Direct field access
/// - No memory duplication
/// - Single source of truth
///
/// The proto definition has been aligned to include all fields
/// previously in VectorRecord, making this a complete replacement.
pub type VectorRecord = crate::proto::proximadb::VectorRecord;

/// Type alias for cleaner imports
pub type ProtoVectorRecord = crate::proto::proximadb::VectorRecord;

/// Helper struct for bincode serialization of non-vector VectorRecord fields
/// This avoids the overhead of protobuf for the remaining fields
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct VectorRecordOtherFields {
    pub id: Option<String>,
    pub metadata: Vec<crate::proto::proximadb::MetadataItem>,
    pub timestamp: u32,
    pub updated_at: Option<u32>,
    pub expires_at: Option<u32>,
    pub version: Option<u32>,
    pub quantized_vector: Option<Vec<u8>>,
}

/// Extension trait for VectorRecord to add optimized serialization methods
/// This provides the fastest possible serialization by avoiding double compression
pub trait VectorRecordSerialization {
    /// Serialize VectorRecord with zero-copy bytemuck for vectors and bincode for other fields
    /// No compression at record level - DataBlock handles compression for optimal ratios
    fn serialize_with_config(
        &self,
        config: &crate::core::serialization::VectorSerializationConfig,
    ) -> anyhow::Result<Vec<u8>>;

    /// Deserialize VectorRecord with zero-copy bytemuck and fast bincode
    /// No decompression at record level - DataBlock handles decompression
    fn deserialize_with_config(
        data: &[u8],
        config: &crate::core::serialization::VectorSerializationConfig,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
}

impl VectorRecordSerialization for VectorRecord {
    /// Serialize using bytemuck for vectors (no compression) and bincode for other fields
    /// This avoids double compression - DataBlock will handle compression at the block level
    fn serialize_with_config(
        &self,
        _config: &crate::core::serialization::VectorSerializationConfig,
    ) -> anyhow::Result<Vec<u8>> {
        use bytemuck::cast_slice;
        use std::io::Write;

        let mut buffer = Vec::new();

        // 1. Serialize vector using raw bytemuck (zero-copy, no compression)
        // This avoids double compression - DataBlock compression will handle it later
        let vector_bytes = cast_slice(&self.vector);

        // 2. Serialize remaining fields using fast bincode (faster than protobuf)
        let other_fields = VectorRecordOtherFields {
            id: Some(self.id.clone()),
            metadata: self.metadata.clone(),
            timestamp: self.timestamp,
            updated_at: self.updated_at,
            expires_at: self.expires_at,
            version: self.version,
            quantized_vector: self.quantized_vector.clone(),
        };
        let bincode_data = bincode::serialize(&other_fields)?;

        // 3. Combine with length prefixes for efficient parsing
        // Format: [vector_len:4][vector_data][bincode_len:4][bincode_data]
        buffer.write_all(&(vector_bytes.len() as u32).to_le_bytes())?;
        buffer.write_all(vector_bytes)?;

        buffer.write_all(&(bincode_data.len()).to_le_bytes())?;
        buffer.write_all(&bincode_data)?;

        Ok(buffer)
    }

    /// Deserialize using raw bytemuck (no decompression) and fast bincode
    /// This avoids double decompression - DataBlock handles decompression at block level
    fn deserialize_with_config(
        data: &[u8],
        _config: &crate::core::serialization::VectorSerializationConfig,
    ) -> anyhow::Result<Self> {
        use bytemuck::try_cast_slice;

        if data.len() < 8 {
            // Need at least 2 length prefixes
            return Err(anyhow::anyhow!("Invalid VectorRecord data: too short"));
        }

        let mut offset = 0;

        // 1. Read vector data using raw bytemuck (zero-copy, no decompression)
        let vector_byte_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        offset += 4;

        if offset + vector_byte_len > data.len() {
            return Err(anyhow::anyhow!("Invalid vector data length"));
        }

        let vector_bytes = &data[offset..offset + vector_byte_len];
        let vector_slice: &[f32] = try_cast_slice(vector_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to cast bytes to f32 slice: {}", e))?;
        let vector = vector_slice.to_vec();
        offset += vector_byte_len;

        // 2. Read other fields using fast bincode
        if offset + 4 > data.len() {
            return Err(anyhow::anyhow!("Missing bincode length"));
        }

        let bincode_len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;

        if offset + bincode_len > data.len() {
            return Err(anyhow::anyhow!("Invalid bincode data length"));
        }

        let bincode_data = &data[offset..offset + bincode_len];
        let other_fields: VectorRecordOtherFields = bincode::deserialize(bincode_data)?;

        // 3. Combine zero-copy vector with fast-deserialized fields
        Ok(VectorRecord {
            id: other_fields.id.unwrap_or_default(),
            vector,
            metadata: other_fields.metadata,
            timestamp: other_fields.timestamp,
            updated_at: other_fields.updated_at,
            expires_at: other_fields.expires_at,
            version: other_fields.version,
            quantized_vector: other_fields.quantized_vector,
            source: None,
        })
    }
}
pub use grpc_metadata_parser::*;
pub use metadata_query::*;
pub use vector_record_migration::{
    proto_batch_to_service, proto_to_service, service_batch_to_proto, service_to_proto,
};

// The VectorRecord optimization trait is already in scope
// No need to re-export it since it's defined in this module

// Note: VectorRecord, VectorRecord, and ProtoVectorRecord are already public
// No need to re-export them as they're defined in this module

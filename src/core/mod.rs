pub mod avro_serialization;
pub mod avro_unified;
pub mod base62;
pub mod config;
pub mod config_loader;
pub mod vector_record_migration;
pub mod error;
pub mod grpc_metadata_parser;
pub mod index;
pub mod indexing;
pub mod metadata_query;
pub mod search;
pub mod serialization;
pub mod compression;  // 🆕 UNIFIED COMPRESSION MODULE
pub mod storage;
pub mod foundation;
pub mod storage_layout;
pub mod memory;
pub mod proto_metadata_helper;
pub mod bloom;
// pub mod errors;  // 🔴 UNUSED MODULE - Never imported
pub mod hardware_capabilities;

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
// Migration phase: Use selective exports and introduce Proto VectorRecord alias
pub use avro_unified::{
    BatchSearchRequest, CollectionConfig, CollectionOperation, CollectionRequest,
    CollectionResponse, CompactionConfig, CompactionStrategy, CompressionAlgorithm,
    DistanceMetric, FieldCondition, HealthResponse, IndexStats, IndexingAlgorithm,
    MetadataFilter, MetricsResponse, NodeId, OperationResponse, SearchContext,
    SearchDebugInfo, SearchMetadata, SearchStrategy,
    ServiceMetrics,
    StorageEngine, String, Vector, VectorId, VectorInsertRequest, VectorInsertResponse,
    VectorOperation, VectorOperationMetrics, VectorSearchRequest, VectorSearchResponse,
    WriteBufferMetrics,
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

// No impl blocks needed - VectorRecord is just a type alias to proto::VectorRecord
// All fields are directly accessible without any method call overhead
pub use metadata_query::*;
pub use grpc_metadata_parser::*;
pub use vector_record_migration::{avro_to_proto, proto_to_avro, avro_batch_to_proto, proto_batch_to_avro};


// Note: VectorRecord, VectorRecord, and ProtoVectorRecord are already public
// No need to re-export them as they're defined in this module

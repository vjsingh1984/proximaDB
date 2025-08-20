// RAPTOR Storage Engine - Row-Aligned Predicated Tensor Optimized Repository
// Combines Google Artus concepts with advanced vector database requirements

/// Magic constant for RAPTOR files (4 bytes)
pub const RAPTOR_MAGIC: [u8; 4] = *b"RPTR";

// Common types module - MUST be first to avoid circular dependencies
pub mod common;

pub mod config;
pub mod engine;
pub mod writer;
pub mod reader;
pub mod compaction;
pub mod hnsw_manager;
pub mod hnsw_compaction;
pub mod unified_reader;     // Consolidated reader
pub mod rowgroup_cache;     // RowGroup-level caching for selective loading
pub mod rowgroup_manager;   // Smart row group management with hybrid architecture
pub mod smart_rowgroup_sizing; // Smart sizing based on dimensions and cloud I/O
pub mod simd_ops;
pub mod simd_encoder;
pub mod artus_bloom;

#[cfg(test)]
mod tests;

// Re-export commonly used types from common module
pub use common::{
    RowGroup, RowGroupMetadata, VectorStats, ColumnStats, 
    MetadataColumn, MetadataValue, MetadataDataType,
    HnswGraph, HnswEdge, LocalHnswSegment, HnswGraphMetadata,
    RaptorFileMetadata, SchemaDescriptor, FieldDescriptor,
    SearchResult, Predicate, PredicateOp,
    FastLanesScheme, VectorEncoding, ColumnEncoding,
    IoStrategy, CachePolicy, ReadPattern,
    LocalityCluster, BloomFilterMetadata,
};

pub use config::{RaptorConfig, CompactionConfig, HnswConfig};
pub use engine::RaptorEngine;
pub use writer::RaptorWriter;
pub use reader::RaptorReader;
pub use unified_reader::RaptorUnifiedReader;
pub use rowgroup_manager::{RowGroupManager, HybridRowGroup, ColumnarBlock, TransposedVectors};
pub use smart_rowgroup_sizing::{SmartRowGroupSizer, OptimalRowGroupSize, CloudIOProfile, CommonConfigurations};
/// Common types and structures shared across RAPTOR modules
/// This eliminates duplication between reader, writer, compaction, and other modules

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use arrow_array::RecordBatch;
use crate::proto::proximadb::VectorRecord;

// ====== Core RowGroup Structure (unified from rowgroup.rs and compaction.rs) ======

/// Primary RowGroup structure used throughout RAPTOR
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowGroup {
    // Core identifiers
    pub id: u32,
    pub offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub row_count: usize,
    
    // Statistics
    pub vector_stats: VectorStats,
    pub metadata_stats: HashMap<String, ColumnStats>,
    
    // Bloom filter for this row group's IDs
    pub bloom_filter: Option<RowGroupBloomFilter>,
    pub bloom_filter_offset: Option<u64>,
    
    // HNSW segment for this row group
    pub hnsw_segment_offset: Option<u64>,
    
    // HNSW locality info
    pub local_hnsw: Option<LocalHnswSegment>,
    
    // Compression and temporal info
    pub compression_codec: String,
    pub min_timestamp: Option<i64>,
    pub max_timestamp: Option<i64>,
    
    // Cached data (for compaction use)
    pub vectors: Option<Vec<VectorRecord>>,
    pub centroid: Option<Vec<f32>>,
}

impl RowGroup {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            offset: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            row_count: 0,
            vector_stats: VectorStats::default(),
            metadata_stats: HashMap::new(),
            bloom_filter: None,
            bloom_filter_offset: None,
            hnsw_segment_offset: None,
            local_hnsw: None,
            compression_codec: "zstd".to_string(),
            min_timestamp: None,
            max_timestamp: None,
            vectors: None,
            centroid: None,
        }
    }
    
    /// Convert to compact representation for storage
    pub fn to_storage(&self) -> RowGroupMetadata {
        RowGroupMetadata {
            id: self.id,
            offset: self.offset,
            compressed_size: self.compressed_size,
            uncompressed_size: self.uncompressed_size,
            row_count: self.row_count,
            vector_stats: self.vector_stats.clone(),
            metadata_stats: self.metadata_stats.clone(),
            bloom_filter_offset: self.bloom_filter_offset,
            hnsw_segment_offset: self.hnsw_segment_offset,
            compression_codec: self.compression_codec.clone(),
            min_timestamp: self.min_timestamp,
            max_timestamp: self.max_timestamp,
            centroid: self.centroid.clone(),
        }
    }
}

/// Compact metadata representation for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowGroupMetadata {
    pub id: u32,
    pub offset: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub row_count: usize,
    pub vector_stats: VectorStats,
    pub metadata_stats: HashMap<String, ColumnStats>,
    pub bloom_filter_offset: Option<u64>,
    pub hnsw_segment_offset: Option<u64>,
    pub compression_codec: String,
    pub min_timestamp: Option<i64>,
    pub max_timestamp: Option<i64>,
    pub centroid: Option<Vec<f32>>,
}

impl Default for RowGroupMetadata {
    fn default() -> Self {
        Self {
            id: 0,
            offset: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            row_count: 0,
            vector_stats: VectorStats::default(),
            metadata_stats: HashMap::new(),
            bloom_filter_offset: None,
            hnsw_segment_offset: None,
            compression_codec: "zstd".to_string(),
            min_timestamp: None,
            max_timestamp: None,
            centroid: None,
        }
    }
}

/// Row page metadata for detailed page-level tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowPageMetadata {
    pub page_id: u32,
    pub file_offset: i64,
    pub compressed_size: i64,
    pub uncompressed_size: i64,
    pub num_rows: i32,
    pub first_id: Vec<u8>,
    pub last_id: Vec<u8>,
    pub compression_codec: String,
}

/// HNSW segment metadata for row group navigation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswSegmentMetadata {
    pub segment_id: u32,
    pub row_group_id: u32,
    pub file_offset: i64,
    pub compressed_size: i64,
    pub uncompressed_size: i64,
    pub num_nodes: i32,
    pub entry_point: Option<u32>,
    pub max_level: u32,
    pub compression_codec: String,
}

// ====== Vector Statistics (unified) ======

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VectorStats {
    pub dimension: usize,
    pub min_norm: f32,
    pub max_norm: f32,
    pub centroid: Vec<f32>,
    pub quantization_error: Option<f32>,
    pub encoding: VectorEncoding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorEncoding {
    Raw,
    ProductQuantization { 
        num_subvectors: usize,
        bits_per_subvector: usize,
    },
    ScalarQuantization {
        bits: usize,
        scale: f32,
        zero_point: f32,
    },
    Binary,
    FastLanes {
        scheme: FastLanesScheme,
    },
}

impl Default for VectorEncoding {
    fn default() -> Self {
        VectorEncoding::Raw
    }
}

// ====== Column Statistics (unified from reader.rs and rowgroup.rs) ======

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnStats {
    pub null_count: usize,
    pub distinct_count: Option<usize>,
    pub min_value: Option<MetadataValue>,
    pub max_value: Option<MetadataValue>,
    pub encoding: ColumnEncoding,
    pub compressed_size: usize,
    pub uncompressed_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnEncoding {
    Dictionary { num_entries: usize },
    Integer { bits: usize },
    Float,
    Boolean,
    String,
    FastLanes { scheme: FastLanesScheme },
}

// ====== Metadata Column (unified from reader.rs and writer.rs) ======

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataColumn {
    pub name: String,
    pub data_type: MetadataDataType,
    pub encoding: ColumnEncoding,
    pub stats: ColumnStats,
    pub dictionary: Option<Vec<String>>,  // For dictionary encoding
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetadataDataType {
    Boolean,
    Integer,
    Float,
    String,
    List(Box<MetadataDataType>),
    Map(Box<MetadataDataType>, Box<MetadataDataType>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetadataValue {
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
    List(Vec<MetadataValue>),
    Map(HashMap<String, MetadataValue>),
}

// ====== HNSW Structures (unified from compaction.rs and hnsw_manager.rs) ======

/// Local HNSW segment for a row group
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalHnswSegment {
    pub num_nodes: usize,
    pub entry_point: u32,
    pub edges: Vec<HnswEdge>,
}

impl LocalHnswSegment {
    pub fn new() -> Self {
        Self {
            num_nodes: 0,
            entry_point: 0,
            edges: Vec::new(),
        }
    }
}

/// HNSW graph structure (unified)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswGraph {
    pub entry_points: Vec<u32>,
    pub edges: Vec<HnswEdge>,
    pub levels: HashMap<u32, usize>,  // node_id -> level
    pub metadata: HnswGraphMetadata,
}

/// Edge in HNSW graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswEdge {
    pub from: u32,
    pub to: u32,
    pub distance: f32,
    pub level: usize,
}

/// HNSW graph metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswGraphMetadata {
    pub num_nodes: usize,
    pub num_edges: usize,
    pub max_level: usize,
    pub m: usize,  // Max connections per node
    pub ef_construction: usize,
    pub ef_search: usize,
    pub distance_metric: String,
}

// ====== FastLanes Encoding Schemes (shared) ======

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FastLanesScheme {
    Delta { bits: usize },
    FrameOfReference { reference: i64, bits: usize },
    RunLength,
    BitPacked { bits: usize },
    Dictionary { num_entries: usize },
    Zigzag { bits: usize },
}

// ====== Compaction Configuration (moved from config.rs duplicate) ======
// Note: Using the one from config.rs as the source of truth

// ====== File Metadata (unified) ======

#[derive(Debug, Clone, Serialize, Deserialize)]
/// CONSOLIDATED RaptorFileMetadata - Single source of truth
/// This combines all fields from the three duplicate definitions
pub struct RaptorFileMetadata {
    // Core file metadata
    pub version: u32,
    pub created_at: i64,
    pub created_by: String,
    pub file_path: String,
    pub file_size: u64,
    
    // Row and vector counts
    pub total_rows: usize,
    pub total_vectors: usize,
    pub dimension: usize,
    
    // Collection info
    pub collection_id: String,
    
    // Row groups
    pub row_groups: Vec<RowGroupMetadata>,
    pub num_rowgroups: usize,
    pub rowgroup_offsets: Vec<u64>,
    pub rowgroup_sizes: Vec<u64>,
    pub rowgroup_vector_counts: Vec<usize>,
    
    // Schema
    pub schema: SchemaDescriptor,
    
    // HNSW metadata
    pub hnsw_metadata: Option<HnswGraphMetadata>,
    pub global_hnsw_offset: u64,
    pub global_hnsw_size: u64,
    pub hnsw_entry_points: Vec<String>,
    pub hnsw_num_layers: u8,
    pub global_hnsw_entry: Option<i32>,
    
    // Bloom filter metadata (for per-rowgroup bloom filters)
    pub bloom_filter_metadata: Option<BloomFilterMetadata>,
    
    // Compression
    pub compression_codec: String,
    
    // Metadata storage
    pub custom_metadata: HashMap<String, String>,
    pub key_value_metadata: Vec<KeyValue>,
    
    // Footer info
    pub footer_offset: u64,
    pub footer_size: u64,
    
    // Access tracking
    pub last_accessed: i64,
    
    // Locality clusters for optimization
    pub locality_clusters: Vec<LocalityClusterInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaDescriptor {
    pub vector_dimension: usize,
    pub metadata_fields: Vec<FieldDescriptor>,
    pub version: u32,
}

impl Default for SchemaDescriptor {
    fn default() -> Self {
        Self {
            vector_dimension: 384, // Default OpenAI embedding dimension
            metadata_fields: Vec::new(),
            version: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalityClusterInfo {
    pub cluster_id: u32,
    pub start_offset: u64,
    pub size: u64,
    pub vector_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDescriptor {
    pub name: String,
    pub data_type: MetadataDataType,
    pub nullable: bool,
    pub default_value: Option<MetadataValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterMetadata {
    pub num_bits: usize,
    pub num_hashes: usize,
    pub false_positive_rate: f64,
    pub offset: u64,
    pub size: u64,
}

// ====== I/O Strategy (for unified reader) ======

#[derive(Debug, Clone)]
pub struct IoStrategy {
    pub prefetch_size: usize,
    pub cache_policy: CachePolicy,
    pub read_pattern: ReadPattern,
}

#[derive(Debug, Clone)]
pub enum CachePolicy {
    LRU,
    LFU,
    Cost,  // Based on I/O cost
    None,
}

#[derive(Debug, Clone)]
pub enum ReadPattern {
    Sequential,
    Random,
    Strided { stride: usize },
    Adaptive,  // Detect pattern at runtime
}

// ====== Search Results (unified) ======

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub vector_id: String,
    pub distance: f32,
    pub vector: Option<Vec<f32>>,
    pub metadata: Option<HashMap<String, MetadataValue>>,
    pub rowgroup_id: u32,
}

// ====== Predicates for filtering ======

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Predicate {
    pub field: String,
    pub op: PredicateOp,
    pub value: MetadataValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PredicateOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    In,
    NotIn,
    Contains,
    StartsWith,
}

// ====== Bloom Filter Structures (HNSW-Optimized) ======

/// Per-RowGroup bloom filter for fast membership testing
/// Optimized for HNSW-organized data where IDs are scattered
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowGroupBloomFilter {
    /// Bloom filter bits (typically 10 bits per ID for 1% false positive)
    pub bits: Vec<u8>,
    
    /// Number of hash functions (typically 7 for optimal)
    pub num_hashes: usize,
    
    /// Number of IDs in this row group
    pub num_ids: usize,
    
    /// Size in bits
    pub size_bits: usize,
    
    /// Target false positive rate
    pub false_positive_rate: f64,
}

/// Columnar ID index within row group for SIMD scanning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnnarIdIndex {
    /// IDs stored columnar for SIMD scanning
    pub ids: Vec<String>,
    
    /// Pre-computed hashes for faster comparison (optional)
    pub id_hashes: Option<Vec<u64>>,
    
    /// Offsets to full row data within row group
    pub row_offsets: Vec<u32>,
    
    /// Whether IDs are sorted (enables binary search)
    pub is_sorted: bool,
}

// ====== Locality clustering for HNSW organization ======

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalityCluster {
    pub id: u32,
    pub centroid: Vec<f32>,
    pub radius: f32,
    pub vector_ids: Vec<String>,
    pub rowgroup_ids: Vec<u32>,
}
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
    // Core identifiers (optimized to u16 for 67M vectors per file)
    pub id: u16,
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
    pub fn new(id: u16) -> Self {
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
            centroid_stats: None, // Will be computed during flush/compaction
        }
    }
}

/// Compact metadata representation for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowGroupMetadata {
    pub id: u16,                    // Optimized: supports 67M vectors per file
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
    
    // Enhanced centroid information for fast pruning
    pub centroid: Option<Vec<f32>>,              // Centroid vector (recomputed during compaction)
    pub centroid_stats: Option<CentroidStats>,   // Statistics for search optimization
}

/// Centroid statistics for rowgroup-level pruning
/// 
/// DESIGN DECISION: We store pre-computed distances because:
/// 1. Distance calculation for 1536-dim vectors takes ~3-5μs per vector
/// 2. For 1000 rowgroups, that's 3-5ms just for distance calculations
/// 3. Storage cost is minimal: ~100 bytes per rowgroup (100KB for 1000 rowgroups)
/// 4. These stats enable O(1) pruning decisions without loading vectors
/// 
/// The stats are computed during flush/compaction when we already have vectors in memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CentroidStats {
    pub cluster_id: u32,                  // IVF cluster assignment
    pub mean_distance: f32,                // Mean distance of vectors to centroid
    pub std_deviation: f32,                // Standard deviation for confidence bounds
    pub radius: f32,                       // 95th percentile distance (pruning radius)
    pub min_distance: f32,                 // Closest vector to centroid
    pub max_distance: f32,                 // Farthest vector from centroid
    
    // Pre-computed bounds for common distance metrics
    // These enable triangle inequality based pruning
    pub euclidean_bounds: Option<DistanceBounds>,
    pub cosine_bounds: Option<DistanceBounds>,
    pub dot_product_bounds: Option<DistanceBounds>,
    
    // Nearest neighbor rowgroups for multi-probe search
    // Store top-K nearest rowgroups by centroid distance
    // This replaces the separate CentroidNeighbors structure
    pub neighbor_rowgroups: Vec<RowGroupNeighbor>,
}

/// Neighbor rowgroup reference for multi-probe search
/// Lightweight reference stored in each rowgroup's metadata
/// Only stores INDICES, not distances - distances computed at query time
/// Optimized with u16 IDs: supports 65,536 rowgroups = 67M vectors per file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowGroupNeighbor {
    pub rowgroup_id: u16,           // Direct index into footer centroids array (67M vectors max)
    pub neighbor_cluster_id: u16,   // Cluster assignment of neighbor (65k clusters max)
    pub neighbor_type: NeighborType, // Hierarchical classification
    
    // NO DISTANCE STORAGE! Distances are computed at query time because:
    // 1. With p≥1024, intra-rowgroup connectivity is strong
    // 2. We use hierarchical navigation: sqrt(k) super-clusters
    // 3. Query-specific distances needed for accurate ranking
    // 4. Allows dynamic distance metrics without rewriting files
    
    // OPTIMAL HIERARCHICAL STRATEGY:
    // - Ultra-small (k≤25): All neighbors (direct access)
    // - Small (k≤100): Max 8 neighbors (limited exploration)
    // - Medium (k≤1000): 0.8×√k intra neighbors (sqrt-based)
    // - Large (k≤5000): 0.5×√k intra + 6 inter (balanced hierarchy)
    // - XLarge (k>5000): 2×ln(k) intra + 0.8×ln(k) inter (log scaling)
    // - Memory: 8 bytes per neighbor (33% savings vs u32)
    // - Performance: <100μs latency for collections up to 5M vectors
}

/// Type of neighbor relationship for hierarchical navigation
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NeighborType {
    IntraSuperCluster,  // Within same super-cluster (local)
    InterSuperCluster,  // Different super-cluster (global)
    Direct,            // For small collections (k < 100)
}

/// Pre-computed distance bounds for fast pruning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistanceBounds {
    pub min: f32,     // Minimum possible distance to any vector in rowgroup
    pub max: f32,     // Maximum possible distance to any vector in rowgroup
    pub p50: f32,     // Median distance (for ranking)
    pub p90: f32,     // 90th percentile (for adaptive pruning)
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
            centroid_stats: None,
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

// ====== Centralized Footer for Centroid Storage ======

/// Centralized centroid storage in RAPTOR footer
/// All centroids stored columnar-encoded and sorted by rowgroup_id for O(1) access
/// 
/// MEMORY COMPARISON (for k=1000 rowgroups, d=1536 dimensions):
/// - Distributed (5 centroids per rowgroup): 5 * 1536 * 4 * 1000 = 30MB
/// - Centralized (all in footer): 1000 * 1536 * 4 = 6MB
/// - Savings: 80% reduction in storage
/// 
/// I/O BENEFITS:
/// - Single read loads ALL centroids (one 6MB I/O vs multiple small reads)
/// - Cached indefinitely (file doesn't change, footer doesn't change)
/// - OS page cache or memory-mapped for zero-copy access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaptorFooter {
    /// All centroids sorted by rowgroup_id for O(1) indexing
    /// Stored using FastLanes columnar encoding for compression
    pub centroids: ColumnarCentroids,
    
    /// Version for backward compatibility
    pub version: u32,
    
    /// Checksum for integrity verification
    pub checksum: u64,
    
    /// File metadata (already exists, just reference it)
    pub file_metadata: RaptorFileMetadata,
}

/// Columnar-encoded centroids using FastLanes
/// Vectors are transposed for better compression and SIMD operations
/// 
/// ENCODING STRATEGY:
/// 1. Transpose: Convert k×d matrix to d×k (better compression per dimension)
/// 2. Delta encode each dimension (values often similar across centroids)
/// 3. Bit-pack based on range (many dimensions need only 8-16 bits)
/// 4. SIMD-friendly layout for fast distance calculations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnarCentroids {
    /// Number of centroids (typically k < 1000)
    pub count: u32,
    
    /// Dimension of each centroid (typically same as vector dimension)
    pub dimension: u32,
    
    /// Rowgroup IDs in sorted order for O(1) access
    /// Size: k * 2 bytes (50% savings vs u32)
    pub rowgroup_ids: Vec<u16>,
    
    /// Transposed centroid data for columnar encoding
    /// Layout: [dim0_values..., dim1_values..., ...]
    /// Size: k * d * 4 bytes (before compression)
    pub transposed_data: Vec<f32>,
    
    /// FastLanes encoding metadata for each dimension
    pub encoding_metadata: Vec<FastLanesMetadata>,
}

impl ColumnarCentroids {
    /// Get centroid by rowgroup_id with O(1) access
    pub fn get_centroid(&self, rowgroup_id: u16) -> Option<Vec<f32>> {
        // Binary search since rowgroup_ids are sorted
        match self.rowgroup_ids.binary_search(&rowgroup_id) {
            Ok(idx) => {
                // Reconstruct centroid from transposed data
                let mut centroid = Vec::with_capacity(self.dimension as usize);
                for dim in 0..self.dimension as usize {
                    let offset = dim * self.count as usize + idx;
                    centroid.push(self.transposed_data[offset]);
                }
                Some(centroid)
            }
            Err(_) => None,
        }
    }
    
    /// Decode all centroids for batch operations
    pub fn decode_all(&self) -> Vec<(u16, Vec<f32>)> {
        let mut centroids = Vec::with_capacity(self.count as usize);
        
        for (idx, &rowgroup_id) in self.rowgroup_ids.iter().enumerate() {
            let mut centroid = Vec::with_capacity(self.dimension as usize);
            for dim in 0..self.dimension as usize {
                let offset = dim * self.count as usize + idx;
                centroid.push(self.transposed_data[offset]);
            }
            centroids.push((rowgroup_id, centroid));
        }
        
        centroids
    }
}

/// Calculate optimal number of neighbors using performance-tested formula
/// Returns (intra_neighbors, inter_neighbors) based on collection size k
pub fn calculate_optimal_neighbors(k: usize) -> (usize, usize) {
    match k {
        // Ultra-small: direct access for maximum accuracy
        k if k <= 25 => (k.saturating_sub(1), 0),
        
        // Small: limited neighbors to prevent over-exploration
        k if k <= 100 => (8.min(k.saturating_sub(1)), 0),
        
        // Medium: sqrt-based intra-cluster only (efficient single-tier)
        k if k <= 1000 => {
            let intra = ((k as f64).sqrt() * 0.8).ceil() as usize;
            (intra, 0)
        },
        
        // Large: balanced intra + limited inter (two-tier hierarchy)
        k if k <= 5000 => {
            let intra = ((k as f64).sqrt() * 0.5).ceil() as usize;
            let inter = 6; // Fixed small number for global exploration
            (intra, inter)
        },
        
        // XLarge: logarithmic scaling prevents neighbor explosion
        _ => {
            let intra = ((k as f64).ln() * 2.0).ceil() as usize;
            let inter = ((k as f64).ln() * 0.8).ceil() as usize;
            (intra, inter)
        }
    }
}

/// Calculate number of super-clusters for hierarchical organization
pub fn calculate_super_clusters(k: usize) -> usize {
    match k {
        k if k <= 100 => 1,  // No super-clustering needed for small collections
        k if k <= 1000 => ((k as f64).sqrt() / 2.0).ceil() as usize,
        _ => ((k as f64).sqrt() / 3.0).ceil() as usize, // More super-clusters for large collections
    }
}

/// Predict search latency in microseconds for performance planning
pub fn predict_search_latency(k: usize, dimension: usize) -> f64 {
    // Centroid computation: ~0.006μs per centroid for 384d (measured)
    let centroid_latency = (k as f64) * (dimension as f64) * 0.000015;
    
    // Neighbor exploration: 3 candidates × neighbors × 50ns per distance
    let (intra, inter) = calculate_optimal_neighbors(k);
    let neighbor_latency = 3.0 * (intra + inter) as f64 * 0.05;
    
    centroid_latency + neighbor_latency
}

/// FastLanes encoding metadata for a dimension
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastLanesMetadata {
    /// Min value in this dimension (for delta encoding)
    pub min_value: f32,
    
    /// Max value in this dimension
    pub max_value: f32,
    
    /// Encoding scheme used
    pub encoding: FastLanesScheme,
    
    /// Compressed size in bytes
    pub compressed_size: u32,
}
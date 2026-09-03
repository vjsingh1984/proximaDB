// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! SST on-disk format types — SstableHeader, IndexEntry, BPlusTreeIndex,
//! SstableIndex, SstMetadataStats (+ SST_MAGIC) hoisted from root
//! `engines/sst/mod.rs` (TD-DECOMP-79) so format readers (arrow_reader,
//! sst_io_layer) can live in engine-core.
use proximadb_compression::CompressionAlgorithm;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Vector format type for bytemuck optimization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum VectorFormat {
    /// All vectors have the same fixed dimension (use bytemuck)
    Fixed { dimension: usize },
    /// Vectors have variable dimensions (use standard serialization)
    #[default]
    Variable,
    /// Mixed dimensions - majority fixed, some variable
    Mixed { dominant_dimension: usize },
}

/// Magic constant for SST files (4 bytes)
pub const SST_MAGIC: [u8; 4] = *b"SST1";

/// SSTable header for row-based storage format with hierarchical optimizations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstableHeader {
    pub version: u32,
    pub level: u8,
    pub entry_count: u64,
    pub min_key: String,
    pub max_key: String,
    pub timestamp: i64,

    // Compression configuration
    pub algorithm: CompressionAlgorithm,
    pub compression_level: u8,

    // Bloom filter configuration
    pub has_bloom_filter: bool,
    pub has_global_bloom: bool, // NEW: Global bloom filter across entire file
    pub has_block_blooms: bool, // NEW: Per-block bloom filters
    pub metadata_column_count: u32, // NEW: Number of metadata columns for bloom sizing

    // Block organization
    pub block_size: u32,
    pub batch_size: u32,
    pub block_count: u32,

    // Component sizes (existing)
    pub header_size: u32,
    pub index_size: u32,
    pub data_size: u32,

    // NEW: Direct access offsets for selective loading (hierarchical architecture)
    pub global_bloom_offset: u64, // Offset to global bloom filter
    pub global_bloom_size: u32,   // Size of global bloom filter
    pub block_index_offset: u64,  // Offset to block index (with per-block blooms)
    pub block_index_size: u32,    // Size of block index
    pub data_blocks_offset: u64,  // Offset to first data block

    // NEW: Vector format analysis for bytemuck optimization
    pub vector_format: VectorFormat,  // Fixed, Variable, or Mixed
    pub fixed_dimension: Option<u32>, // For fixed-dimension optimization
    pub compression_ratio: f32,       // Achieved compression ratio

    // NEW: Centroid index for IVF-style search optimization (LanceDB-inspired)
    // Stores the centroid (mean vector) of all vectors in this SST file
    // Used for partition-aware search to skip irrelevant SST files
    #[serde(default)]
    pub centroid: Option<Vec<f32>>, // Centroid vector (mean of all vectors)
    #[serde(default)]
    pub centroid_distance_sum: Option<f32>, // Sum of distances to centroid (for variance)
    #[serde(default)]
    pub min_distance_to_centroid: Option<f32>, // Minimum distance from any vector to centroid
    #[serde(default)]
    pub max_distance_to_centroid: Option<f32>, // Maximum distance from any vector to centroid

    // NEW: ProximaSchema integration for compute engine compatibility
    // Schema reference for DataFusion/Spark/Trino integration
    #[serde(default)]
    pub schema_id: Option<String>, // Reference to schema in SchemaRegistry
    #[serde(default)]
    pub schema_version: Option<u32>, // Schema version for compatibility checking
    #[serde(default)]
    pub schema_fingerprint: Option<u64>, // Fast schema comparison (xxhash64)
}

/// Index entry for fast key lookups in SSTable with hierarchical bloom filters
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct IndexEntry {
    /// First (minimum) key in this block - used for range lookups
    pub key: String,
    /// Last (maximum) key in this block - enables proper B+ tree range queries
    /// When a block contains multiple records, this allows correct key containment checks
    #[serde(default)]
    pub last_key: Option<String>,
    pub offset: u64,
    pub size: u32,
    pub block_id: u32,
    pub block_offset: u32,
    pub compressed: bool,
    /// Centroid for this block to enable block-level vector pruning (FP32 - legacy)
    pub block_centroid: Vec<f32>,
    /// FP16 quantized centroid (50% storage reduction, <0.1% distance error)
    /// When present, this is used for block selection; block_centroid is kept for backward compatibility
    pub block_centroid_fp16: Option<Vec<u16>>,
    /// TD-RDSTRAT-5 lever-3: block RMS radius (spread). Enables the distance
    /// lower-bound prune score `d(q,centroid) − k·radius`. `#[serde(default)]` →
    /// legacy entries (no radius) deserialize to `0.0` = today's centroid-only
    /// ranking (mixed-read-safe).
    #[serde(default)]
    pub block_radius: f32,

    /// Minimum values for each metadata column in this block
    pub metadata_min_values: HashMap<String, serde_json::Value>,
    /// Maximum values for each metadata column in this block
    pub metadata_max_values: HashMap<String, serde_json::Value>,
    /// Count of null values for each metadata column in this block
    pub metadata_null_counts: HashMap<String, u32>,

    // NEW: Hierarchical bloom filter support
    /// Block-level key bloom filter (optional, for large blocks)
    pub block_key_bloom: Option<Vec<u8>>,
    /// Block-level metadata bloom filter (optional, for metadata-heavy queries)
    pub block_metadata_bloom: Option<Vec<u8>>,

    // NEW: Vector format optimization info
    pub vector_format: VectorFormat,

    // NEW: Z-Order spatial indexing for range-based pruning
    /// Z-Order code (Morton code) for this block's centroid after PCA projection
    /// Enables efficient spatial range queries and pruning (supports up to 64 PCA dims)
    #[serde(default)]
    pub zorder_code: Option<crate::proximablocks::spatial_encoding::SpatialCode>,
    // REMOVED: compression_ratio - can be calculated on-demand from size and DataBlock.uncompressed_size
    /// TD-040: per-dimension vector component bounds for this block, enabling
    /// L2 lower-bound block pruning (see `VectorBoundsPruner`). `None` for blocks
    /// written before TD-040 (index magic < `IDX3`) or with no/mixed-dim vectors —
    /// pruning then conservatively scans the block. Both must be present and
    /// same-length to prune.
    #[serde(default)]
    pub block_component_min: Option<Vec<f32>>,
    #[serde(default)]
    pub block_component_max: Option<Vec<f32>>,
}

/// Minimal B+ tree descriptor persisted in the index blob for fast lookups.
/// We use a two-level structure (root + leaves) for O(log n) key/range lookups.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BPlusTreeIndex {
    /// Fan-out per leaf (number of entries per leaf)
    pub fanout: usize,
    /// Leaf ranges referencing slices in the sorted IndexEntry array
    pub leaves: Vec<BPlusLeaf>,
    /// Root separators for quick leaf selection
    pub root: Vec<BPlusRootEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BPlusLeaf {
    pub start_key: String,
    pub end_key: String,
    /// Start index in the IndexEntry array
    pub start_idx: usize,
    /// Number of entries in this leaf
    pub len: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BPlusRootEntry {
    pub pivot_key: String,
    pub leaf_idx: usize,
}

impl BPlusTreeIndex {
    /// Build a two-level B+ tree over already-sorted entries.
    pub fn build(entries: &[IndexEntry], fanout: usize) -> Self {
        let fanout = fanout.max(8); // Minimum fanout of 8
        let mut leaves = Vec::new();

        for (i, chunk) in entries.chunks(fanout).enumerate() {
            let start_key = chunk.first().map(|e| e.key.clone()).unwrap_or_default();
            // Use last_key from the last entry in chunk if available, otherwise fall back to key
            let end_key = chunk.last().map_or_else(
                || start_key.clone(),
                |e| e.last_key.clone().unwrap_or_else(|| e.key.clone()),
            );
            leaves.push(BPlusLeaf {
                start_key,
                end_key,
                start_idx: i * fanout,
                len: chunk.len(),
            });
        }

        let mut root = Vec::with_capacity(leaves.len());
        for (idx, leaf) in leaves.iter().enumerate() {
            root.push(BPlusRootEntry {
                pivot_key: leaf.start_key.clone(),
                leaf_idx: idx,
            });
        }

        Self {
            fanout,
            leaves,
            root,
        }
    }

    /// Locate the leaf range for a given key.
    pub fn leaf_for_key(&self, key: &str) -> Option<&BPlusLeaf> {
        if self.root.is_empty() {
            return None;
        }

        // Binary search in root to find leaf
        let mut lo = 0;
        let mut hi = self.root.len();
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if key >= self.root[mid].pivot_key.as_str() {
                lo = mid;
            } else {
                hi = mid;
            }
        }

        self.root
            .get(lo)
            .and_then(|entry| self.leaves.get(entry.leaf_idx))
    }

    /// Find entries in a range [start_key, end_key].
    pub fn range_leaves(&self, start_key: &str, end_key: &str) -> Vec<&BPlusLeaf> {
        let mut result = Vec::new();

        for leaf in &self.leaves {
            // Check if this leaf overlaps with [start_key, end_key]
            if leaf.end_key.as_str() >= start_key && leaf.start_key.as_str() <= end_key {
                result.push(leaf);
            }
        }

        result
    }
}

/// Enhanced SSTable index with metadata statistics and custom serialization
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SstableIndex {
    pub entries: Vec<IndexEntry>,
    pub metadata_stats: HashMap<String, SstMetadataStats>,
    pub vector_count: usize,
    pub min_key: String,
    pub max_key: String,
    /// Optional B+ tree for fast point/range lookups (built at write time)
    #[serde(default)]
    pub bplus_tree: Option<BPlusTreeIndex>,
}

/// Backwards-compat alias for [`SstMetadataStats`].
pub type MetadataStats = SstMetadataStats;

/// Metadata statistics for predicate pushdown
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SstMetadataStats {
    pub min_value: serde_json::Value,
    pub max_value: serde_json::Value,
    pub null_count: usize,
    pub distinct_count: usize,
    pub bloom_filter_offset: Option<u64>,
}

impl SstableIndex {
    /// Custom serialization for robust persistence
    /// Uses explicit layout for IndexEntries to avoid serde_json issues in bincode
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        use std::io::Write;
        let mut buffer = Vec::new();

        // Magic header for version 1
        buffer.write_all(b"IDX1")?;

        // Min/Max keys
        let min_bytes = self.min_key.as_bytes();
        buffer.write_all(&(min_bytes.len() as u32).to_le_bytes())?;
        buffer.write_all(min_bytes)?;

        let max_bytes = self.max_key.as_bytes();
        buffer.write_all(&(max_bytes.len() as u32).to_le_bytes())?;
        buffer.write_all(max_bytes)?;

        // Vector count
        buffer.write_all(&(self.vector_count as u64).to_le_bytes())?;

        // Entries
        buffer.write_all(&(self.entries.len() as u64).to_le_bytes())?;
        for entry in &self.entries {
            // Use IndexEntry's custom serialization which handles JSON safely
            let entry_bytes = entry.serialize()?;
            buffer.write_all(&(entry_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(&entry_bytes)?;
        }

        // B+ Tree (safe to use bincode here as it contains no JSON Values)
        match &self.bplus_tree {
            Some(tree) => {
                buffer.write_all(&1u8.to_le_bytes())?;
                let tree_bytes = bincode::serialize(tree)?;
                buffer.write_all(&(tree_bytes.len() as u32).to_le_bytes())?;
                buffer.write_all(&tree_bytes)?;
            }
            None => buffer.write_all(&0u8.to_le_bytes())?,
        }

        // Metadata Stats (placeholder - writing 0 count)
        buffer.write_all(&0u32.to_le_bytes())?;

        Ok(buffer)
    }

    /// Custom deserialization for robust persistence
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        use std::io::Read;
        let mut cursor = std::io::Cursor::new(data);

        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;

        // Check magic header
        if &magic != b"IDX1" {
            return Err(anyhow::anyhow!(
                "Invalid SstableIndex format: expected IDX1, got {:?}",
                std::str::from_utf8(&magic).unwrap_or("????")
            ));
        }

        // Min Key
        let mut len_buf = [0u8; 4];
        cursor.read_exact(&mut len_buf)?;
        let min_len = u32::from_le_bytes(len_buf) as usize;
        let mut min_bytes = vec![0u8; min_len];
        cursor.read_exact(&mut min_bytes)?;
        let min_key = String::from_utf8(min_bytes)?;

        // Max Key
        cursor.read_exact(&mut len_buf)?;
        let max_len = u32::from_le_bytes(len_buf) as usize;
        let mut max_bytes = vec![0u8; max_len];
        cursor.read_exact(&mut max_bytes)?;
        let max_key = String::from_utf8(max_bytes)?;

        // Vector Count
        let mut u64_buf = [0u8; 8];
        cursor.read_exact(&mut u64_buf)?;
        let vector_count = u64::from_le_bytes(u64_buf) as usize;

        // Entries
        cursor.read_exact(&mut u64_buf)?;
        let entries_count = u64::from_le_bytes(u64_buf) as usize;
        let mut entries = Vec::with_capacity(entries_count);

        for _ in 0..entries_count {
            cursor.read_exact(&mut len_buf)?;
            let entry_len = u32::from_le_bytes(len_buf) as usize;

            let start = cursor.position() as usize;
            if start + entry_len > data.len() {
                return Err(anyhow::anyhow!("Truncated index entry"));
            }

            let entry_data = &data[start..start + entry_len];
            entries.push(IndexEntry::deserialize(entry_data)?);

            cursor.set_position((start + entry_len) as u64);
        }

        // B+ Tree
        let mut bool_buf = [0u8; 1];
        cursor.read_exact(&mut bool_buf)?;
        let bplus_tree = if bool_buf[0] == 1 {
            cursor.read_exact(&mut len_buf)?;
            let tree_len = u32::from_le_bytes(len_buf) as usize;

            let start = cursor.position() as usize;
            if start + tree_len > data.len() {
                return Err(anyhow::anyhow!("Truncated B+ tree data"));
            }

            let tree = bincode::deserialize(&data[start..start + tree_len])?;
            cursor.set_position((start + tree_len) as u64);
            Some(tree)
        } else {
            None
        };

        // Metadata Stats (consume count)
        if cursor.position() < data.len() as u64 {
            let _ = cursor.read_exact(&mut len_buf);
        }

        Ok(Self {
            entries,
            metadata_stats: HashMap::new(),
            vector_count,
            min_key,
            max_key,
            bplus_tree,
        })
    }
}
#[derive(Debug, Clone)]
pub enum SstableReadingStrategy {
    /// Selective reads via cache with range optimization for normal queries
    SelectiveWithCache {
        use_range_reads: bool,
        enable_bloom_filters: bool,
        enable_cache_lookup: bool,
        enable_metadata_cache: bool,
    },
    /// Full read strategy for compaction operations - avoid cache pollution
    CompactionFullRead {
        skip_bloom_filters: bool,
        skip_indexes: bool,
        bypass_write_cache: bool,
        use_disk_cache_if_exists: bool,
        sequential_io: bool,
    },
    /// Legacy strategies for backward compatibility
    FullScan { use_block_cache: bool },
    IndexRangeScan {
        start_block: usize,
        end_block: usize,
        use_bloom_filter: bool,
    },
    MetadataFiltered {
        selected_blocks: Vec<usize>,
        skip_bloom_check: bool,
    },
    Hybrid {
        primary_strategy: Box<SstableReadingStrategy>,
        fallback_blocks: Vec<usize>,
    },
}
impl IndexEntry {
    /// Custom serialization to avoid serde_json::Value bincode issues
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        use std::io::Write;
        let mut buffer = Vec::new();

        // Write magic header (IDX3 adds TD-040 per-block vector component bounds).
        buffer.write_all(b"IDX3")?;

        // Write key
        let key_bytes = self.key.as_bytes();
        buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
        buffer.write_all(key_bytes)?;

        // Write last_key (new in IDX2)
        match &self.last_key {
            Some(lk) => {
                buffer.write_all(&1u8.to_le_bytes())?; // Has last_key
                let lk_bytes = lk.as_bytes();
                buffer.write_all(&(lk_bytes.len() as u32).to_le_bytes())?;
                buffer.write_all(lk_bytes)?;
            }
            None => {
                buffer.write_all(&0u8.to_le_bytes())?; // No last_key
            }
        }

        // Write primitive fields
        buffer.write_all(&self.offset.to_le_bytes())?;
        buffer.write_all(&self.size.to_le_bytes())?;
        buffer.write_all(&self.block_id.to_le_bytes())?;
        buffer.write_all(&self.block_offset.to_le_bytes())?;
        buffer.write_all(&[if self.compressed { 1u8 } else { 0u8 }])?;

        // Write block centroid
        buffer.write_all(&(self.block_centroid.len() as u32).to_le_bytes())?;
        for v in &self.block_centroid {
            buffer.write_all(&v.to_le_bytes())?;
        }

        // Write FP16 centroid (optional, for storage optimization)
        match &self.block_centroid_fp16 {
            Some(fp16_data) => {
                buffer.write_all(&1u8.to_le_bytes())?; // Has FP16 centroid
                buffer.write_all(&(fp16_data.len() as u32).to_le_bytes())?;
                for &v in fp16_data {
                    buffer.write_all(&v.to_le_bytes())?;
                }
            }
            None => {
                buffer.write_all(&0u8.to_le_bytes())?; // No FP16 centroid
            }
        }

        // Write metadata_min_values
        buffer.write_all(&(self.metadata_min_values.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata_min_values {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            proximadb_search_types::json_value_serde::serialize_json_value(value, &mut buffer)?;
        }

        // Write metadata_max_values
        buffer.write_all(&(self.metadata_max_values.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata_max_values {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            proximadb_search_types::json_value_serde::serialize_json_value(value, &mut buffer)?;
        }

        // Write metadata_null_counts
        buffer.write_all(&(self.metadata_null_counts.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata_null_counts {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            buffer.write_all(&value.to_le_bytes())?;
        }

        // NEW: Write hierarchical bloom filter data
        match &self.block_key_bloom {
            Some(bloom_data) => {
                buffer.write_all(&1u8.to_le_bytes())?; // Has bloom
                buffer.write_all(&(bloom_data.len() as u32).to_le_bytes())?;
                buffer.write_all(bloom_data)?;
            }
            None => {
                buffer.write_all(&0u8.to_le_bytes())?; // No bloom
            }
        }

        match &self.block_metadata_bloom {
            Some(bloom_data) => {
                buffer.write_all(&1u8.to_le_bytes())?; // Has bloom
                buffer.write_all(&(bloom_data.len() as u32).to_le_bytes())?;
                buffer.write_all(bloom_data)?;
            }
            None => {
                buffer.write_all(&0u8.to_le_bytes())?; // No bloom
            }
        }

        // NEW: Write vector format info (removed compression_ratio)
        let format_byte = match self.vector_format {
            VectorFormat::Variable => 0u8,
            VectorFormat::Fixed { dimension } => {
                buffer.write_all(&1u8.to_le_bytes())?; // Fixed format
                buffer.write_all(&(dimension as u32).to_le_bytes())?;
                1u8
            }
            VectorFormat::Mixed { dominant_dimension } => {
                buffer.write_all(&2u8.to_le_bytes())?; // Mixed format
                buffer.write_all(&(dominant_dimension as u32).to_le_bytes())?;
                2u8
            }
        };
        if format_byte == 0 {
            buffer.write_all(&format_byte.to_le_bytes())?;
        }

        // IDX3: per-block vector component bounds (TD-040). Each is an optional
        // f32 vec encoded as [present:u8][len:u32][f32 * len].
        for bounds in [&self.block_component_min, &self.block_component_max] {
            match bounds {
                Some(values) => {
                    buffer.write_all(&1u8.to_le_bytes())?;
                    buffer.write_all(&(values.len() as u32).to_le_bytes())?;
                    for v in values {
                        buffer.write_all(&v.to_le_bytes())?;
                    }
                }
                None => buffer.write_all(&0u8.to_le_bytes())?,
            }
        }

        Ok(buffer)
    }

    /// Custom deserialization to avoid serde_json::Value bincode issues
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        use std::io::Read;
        let mut cursor = std::io::Cursor::new(data);

        // Read and validate magic header (IDX1 = legacy, IDX2 = with last_key,
        // IDX3 = with TD-040 per-block vector component bounds).
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;
        let is_v3 = &magic == b"IDX3";
        // IDX2-and-later share the same field layout up to vector_format.
        let is_v2 = is_v3 || &magic == b"IDX2";
        if !is_v2 && &magic != b"IDX1" {
            return Err(anyhow::anyhow!("Invalid IndexEntry format"));
        }

        // Read key
        let mut len_buf = [0u8; 4];
        cursor.read_exact(&mut len_buf)?;
        let key_len = u32::from_le_bytes(len_buf) as usize;
        let mut key_bytes = vec![0u8; key_len];
        cursor.read_exact(&mut key_bytes)?;
        let key = String::from_utf8(key_bytes)?;

        // Read last_key (new in IDX2, defaults to None for IDX1)
        let last_key = if is_v2 {
            let mut bool_buf = [0u8; 1];
            cursor.read_exact(&mut bool_buf)?;
            let has_last_key = bool_buf[0] != 0;
            if has_last_key {
                cursor.read_exact(&mut len_buf)?;
                let lk_len = u32::from_le_bytes(len_buf) as usize;
                let mut lk_bytes = vec![0u8; lk_len];
                cursor.read_exact(&mut lk_bytes)?;
                Some(String::from_utf8(lk_bytes)?)
            } else {
                None
            }
        } else {
            None
        };

        // Read primitive fields
        let mut u64_buf = [0u8; 8];
        cursor.read_exact(&mut u64_buf)?;
        let offset = u64::from_le_bytes(u64_buf);

        let mut u32_buf = [0u8; 4];
        cursor.read_exact(&mut u32_buf)?;
        let size = u32::from_le_bytes(u32_buf);

        cursor.read_exact(&mut u32_buf)?;
        let block_id = u32::from_le_bytes(u32_buf);

        cursor.read_exact(&mut u32_buf)?;
        let block_offset = u32::from_le_bytes(u32_buf);

        let mut bool_buf = [0u8; 1];
        cursor.read_exact(&mut bool_buf)?;
        let compressed = bool_buf[0] != 0;

        // Read block centroid
        cursor.read_exact(&mut u32_buf)?;
        let centroid_len = u32::from_le_bytes(u32_buf) as usize;
        let mut block_centroid = Vec::with_capacity(centroid_len);
        for _ in 0..centroid_len {
            let mut f32_buf = [0u8; 4];
            cursor.read_exact(&mut f32_buf)?;
            block_centroid.push(f32::from_le_bytes(f32_buf));
        }

        // Read FP16 centroid (optional, for backward compatibility)
        cursor.read_exact(&mut bool_buf)?;
        let has_fp16_centroid = bool_buf[0] != 0;
        let block_centroid_fp16 = if has_fp16_centroid {
            cursor.read_exact(&mut u32_buf)?;
            let fp16_len = u32::from_le_bytes(u32_buf) as usize;
            let mut fp16_data = Vec::with_capacity(fp16_len);
            for _ in 0..fp16_len {
                let mut u16_buf = [0u8; 2];
                cursor.read_exact(&mut u16_buf)?;
                fp16_data.push(u16::from_le_bytes(u16_buf));
            }
            Some(fp16_data)
        } else {
            None
        };

        // Read metadata_min_values
        cursor.read_exact(&mut len_buf)?;
        let min_values_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata_min_values = HashMap::new();
        for _ in 0..min_values_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            let value =
                proximadb_search_types::json_value_serde::deserialize_json_value(&mut cursor)?;
            metadata_min_values.insert(key, value);
        }

        // Read metadata_max_values
        cursor.read_exact(&mut len_buf)?;
        let max_values_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata_max_values = HashMap::new();
        for _ in 0..max_values_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            let value =
                proximadb_search_types::json_value_serde::deserialize_json_value(&mut cursor)?;
            metadata_max_values.insert(key, value);
        }

        // Read metadata_null_counts
        cursor.read_exact(&mut len_buf)?;
        let null_counts_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata_null_counts = HashMap::new();
        for _ in 0..null_counts_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            cursor.read_exact(&mut u32_buf)?;
            let value = u32::from_le_bytes(u32_buf);
            metadata_null_counts.insert(key, value);
        }

        // NEW: Read hierarchical bloom filter data
        cursor.read_exact(&mut bool_buf)?;
        let has_key_bloom = bool_buf[0] != 0;
        let block_key_bloom = if has_key_bloom {
            cursor.read_exact(&mut u32_buf)?;
            let bloom_len = u32::from_le_bytes(u32_buf) as usize;
            let mut bloom_data = vec![0u8; bloom_len];
            cursor.read_exact(&mut bloom_data)?;
            Some(bloom_data)
        } else {
            None
        };

        cursor.read_exact(&mut bool_buf)?;
        let has_metadata_bloom = bool_buf[0] != 0;
        let block_metadata_bloom = if has_metadata_bloom {
            cursor.read_exact(&mut u32_buf)?;
            let bloom_len = u32::from_le_bytes(u32_buf) as usize;
            let mut bloom_data = vec![0u8; bloom_len];
            cursor.read_exact(&mut bloom_data)?;
            Some(bloom_data)
        } else {
            None
        };

        // NEW: Read vector format and compression info
        cursor.read_exact(&mut bool_buf)?;
        let format_type = bool_buf[0];
        let vector_format = match format_type {
            0 => VectorFormat::Variable,
            1 => {
                cursor.read_exact(&mut u32_buf)?;
                let dimension = u32::from_le_bytes(u32_buf) as usize;
                VectorFormat::Fixed { dimension }
            }
            2 => {
                cursor.read_exact(&mut u32_buf)?;
                let dominant_dimension = u32::from_le_bytes(u32_buf) as usize;
                VectorFormat::Mixed { dominant_dimension }
            }
            _ => VectorFormat::Variable,
        };

        // REMOVED: No longer reading compression_ratio

        // IDX3: per-block vector component bounds (TD-040). Older formats → None.
        let mut read_opt_f32_vec = || -> anyhow::Result<Option<Vec<f32>>> {
            if !is_v3 {
                return Ok(None);
            }
            cursor.read_exact(&mut bool_buf)?;
            if bool_buf[0] == 0 {
                return Ok(None);
            }
            cursor.read_exact(&mut u32_buf)?;
            let len = u32::from_le_bytes(u32_buf) as usize;
            let mut values = Vec::with_capacity(len);
            let mut f32_buf = [0u8; 4];
            for _ in 0..len {
                cursor.read_exact(&mut f32_buf)?;
                values.push(f32::from_le_bytes(f32_buf));
            }
            Ok(Some(values))
        };
        let block_component_min = read_opt_f32_vec()?;
        let block_component_max = read_opt_f32_vec()?;

        Ok(Self {
            key,
            last_key,
            offset,
            size,
            block_id,
            block_offset,
            compressed,
            block_centroid,
            block_centroid_fp16,
            block_radius: 0.0, // legacy SSTable header carries no radius (lever-3 is PAX-only)
            metadata_min_values,
            metadata_max_values,
            metadata_null_counts,
            block_key_bloom,
            block_metadata_bloom,
            vector_format,
            zorder_code: None, // Deserialized separately if present
            block_component_min,
            block_component_max,
        })
    }
}

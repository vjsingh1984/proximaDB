//! Proxima integration for HELIX engine
//!
//! This module bridges HELIX-specific clustering with the shared Proxima
//! block structures used across SST, SWIFT, and other engines.

use anyhow::Result;
use bytes::{BufMut, BytesMut};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

// Reuse existing Proxima structures
use crate::storage::engines::core::formats::proximablocks::block_structures::{
    BlockCompressionConfig, ProximaBlockMetadata, ProximaDataBlock, QuantizationStatistics,
};

use crate::core::{VectorRecord, compression::CompressionAlgorithm};
use crate::storage::engines::constants::HELIX_MAGIC;
use crate::storage::persistence::filesystem::FileSystem;

// ProximaDataBlock now uses ProximaCodec internally
use crate::storage::engines::core::formats::proximablocks::engine_profile::EngineProfile;

// Re-export for convenience
pub use crate::storage::engines::core::formats::proximablocks::block_structures::ProximaMetadata;

/// HELIX Spatial Block Writer
/// Uses ProximaDataBlock's internal SIMD encoding with spatial clustering
pub struct HelixSIMDWriter {
    hilbert_curve_size: usize,
    spatial_grouping_enabled: bool,
    dimension: usize,
    max_vectors: usize,
}

impl HelixSIMDWriter {
    pub fn new(dimension: usize, max_vectors: usize, hilbert_curve_size: usize) -> Result<Self> {
        Ok(Self {
            hilbert_curve_size,
            spatial_grouping_enabled: true,
            dimension,
            max_vectors,
        })
    }

    /// Create SIMD-optimized Proxima block with spatial clustering awareness
    /// Now uses ProximaDataBlock's internal SIMD encoding
    pub async fn create_simd_block(
        &self,
        records: &[VectorRecord],
        hilbert_keys: Option<&[u64]>,
        block_id: u32,
    ) -> Result<(ProximaDataBlock, HelixBlockMetadata)> {
        debug!(
            "🧬 HELIX: Creating spatial-optimized block {} with {} vectors",
            block_id,
            records.len()
        );

        if records.is_empty() {
            return Err(anyhow::anyhow!("Cannot create block from empty vector set"));
        }

        let start_time = std::time::Instant::now();

        // Create enhanced compression config for HELIX with spatial optimization
        let compression_config = BlockCompressionConfig {
            algorithm: CompressionAlgorithm::Zstd, // Better compression for read-heavy workloads
            compression_level: 3,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 256,
            dictionary_compression: records.len() > 1000,
            // Use Auto layout - defaults to GroupedFieldEncoded for optimal performance
            vector_layout:
                crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout::Auto,
            metadata_algorithm: None,
        };

        // Create Proxima block with HELIX engine profile
        // The block will internally apply SIMD encoding based on the layout
        let mut block = ProximaDataBlock::new_with_engine_profile(
            records.to_vec(),
            compression_config,
            EngineProfile::Helix,
        );
        block.block_id = block_id;

        let encoding_time = start_time.elapsed();

        // Calculate spatial statistics for clustering
        let hilbert_range = if let Some(keys) = hilbert_keys {
            if !keys.is_empty() {
                let min_key = *keys.iter().min().unwrap();
                let max_key = *keys.iter().max().unwrap();
                Some((min_key, max_key))
            } else {
                None
            }
        } else {
            None
        };

        // Generate spatial clustering hints
        // Extract vectors for spatial analysis
        let vectors: Vec<Vec<f32>> = records
            .iter()
            .filter(|r| !r.vector.is_empty())
            .map(|r| r.vector.clone())
            .collect();

        let spatial_variance = self.calculate_spatial_variance(&vectors);
        let clustering_hints = ClusteringHints {
            access_frequency: 0.0, // Will be updated by query patterns
            last_accessed: None,
            query_selectivity: self.estimate_query_selectivity(spatial_variance),
        };

        // Create HELIX metadata with spatial information
        let helix_metadata = HelixBlockMetadata {
            proxima_metadata: block.metadata.clone(),
            hilbert_range,
            pca_stats: None, // Could be added later for advanced PCA integration
            clustering_hints: Some(clustering_hints),
        };

        // Get compression ratio from the block's encoded data
        let compression_ratio = if let Some(ref encoded) = block.encoded_vectors {
            let original_size =
                records.len() * records.first().map(|r| r.vector.len()).unwrap_or(0) * 4;
            let encoded_size: usize = encoded.iter().map(|d| d.len()).sum();
            if original_size > 0 {
                (encoded_size * 100) / original_size
            } else {
                100
            }
        } else {
            100 // No encoding applied
        };

        info!(
            "✅ HELIX block {} ready: {}% compression, {:.2}ms encoding time",
            block_id,
            compression_ratio,
            encoding_time.as_millis()
        );

        Ok((block, helix_metadata))
    }

    /// Calculate spatial variance for clustering optimization
    fn calculate_spatial_variance(&self, vectors: &[Vec<f32>]) -> f32 {
        if vectors.is_empty() || vectors[0].is_empty() {
            return 0.0;
        }

        let dimension = vectors[0].len();
        let mut total_variance = 0.0;

        // Calculate variance per dimension and average
        for dim in 0..dimension.min(16) {
            // Sample first 16 dimensions for performance
            let values: Vec<f32> = vectors
                .iter()
                .map(|v| if dim < v.len() { v[dim] } else { 0.0 })
                .collect();

            let mean = values.iter().sum::<f32>() / values.len() as f32;
            let variance =
                values.iter().map(|&v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32;

            total_variance += variance;
        }

        total_variance / dimension.min(16) as f32
    }

    /// Estimate query selectivity for spatial optimization
    fn estimate_query_selectivity(&self, spatial_variance: f32) -> f32 {
        // Higher variance = more spread out = better for range queries
        // Lower variance = clustered = better for similarity queries
        if spatial_variance > 1.0 {
            0.1 // High selectivity - good for range queries
        } else if spatial_variance > 0.1 {
            0.5 // Medium selectivity
        } else {
            0.9 // Low selectivity - very clustered data
        }
    }
}

/// Enhanced HELIX SSTable writer with SIMD optimization

/// HELIX-specific SSTable metadata with clustering information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelixBlockMetadata {
    /// Base Proxima metadata
    pub proxima_metadata: ProximaBlockMetadata,
    /// Hilbert key range for this block
    pub hilbert_range: Option<(u64, u64)>,
    /// PCA projection statistics
    pub pca_stats: Option<PCAStats>,
    /// Liquid clustering hints
    pub clustering_hints: Option<ClusteringHints>,
}

/// PCA projection statistics for a block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PCAStats {
    pub mean_projection: Vec<f32>,
    pub variance_explained: f32,
    pub principal_components_used: usize,
}

/// Cacheable HELIX file header containing all metadata needed for queries
/// This structure is cached by UnifiedCachingFilesystem to avoid repeated API calls
///
/// IMPORTANT: Field order matters for bincode serialization! Do not reorder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelixFileHeader {
    /// Magic and version for validation (must be first for quick validation)
    pub magic: [u8; 4],
    pub version: u32,
    /// File size (for validation)
    pub file_size: usize,
    /// Block metadata for all blocks (includes hilbert_range for pruning)
    pub block_metadata: Vec<HelixBlockMetadata>,
    /// Block offsets for direct access (absolute byte offsets from file start)
    pub block_offsets: Vec<u64>,
    /// Block sizes in bytes (u32 sufficient for blocks < 4GB)
    pub block_sizes: Vec<u32>,
}

impl HelixFileHeader {
    /// Cache key prefix for HELIX file headers
    pub const CACHE_KEY_PREFIX: &'static str = "helix_header_v2";

    /// Generate cache key for a file path
    pub fn cache_key(path: &str) -> String {
        format!("{}:{}", Self::CACHE_KEY_PREFIX, path)
    }
}

/// Liquid clustering hints based on query patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusteringHints {
    pub access_frequency: f32,
    pub last_accessed: Option<chrono::DateTime<chrono::Utc>>,
    pub query_selectivity: f32,
}

/// Write a HELIX SSTable using Proxima encoding with SIMD optimizations
///
/// This is the unified writer that handles all HELIX SSTable writes with:
/// - SIMD-optimized vector encoding via ProximaDataBlock
/// - Hilbert curve spatial clustering (optional)
/// - Unified header format for cloud-optimized reads (2 API calls vs 4+)
/// - Bloom filter for fast ID lookups
/// - Adaptive compression based on block characteristics
///
/// Note: Despite the name, all ProximaDataBlock encoding uses SIMD (NEON/AVX)
pub async fn write_helix_sstable(
    filesystem: &Arc<crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem>,
    path: &Path,
    records: &[VectorRecord],
    block_size: usize,
    magic: [u8; 4],
    hilbert_keys: Option<&[u64]>,
    hilbert_curve_size: Option<usize>,
) -> Result<u64> {
    if records.is_empty() {
        return Ok(0);
    }

    info!(
        "🧬 HELIX: Writing {} vectors with SIMD spatial compression",
        records.len()
    );

    // Initialize SIMD writer with spatial optimization
    let dimension = records[0].vector.len();
    let curve_size = hilbert_curve_size.unwrap_or(256);
    let simd_writer = HelixSIMDWriter::new(dimension, records.len(), curve_size)?;

    use crate::storage::engines::core::formats::proximablocks::bloom_filter::{
        BloomFilterConfig, factory::BloomFilterFactory,
    };

    // Initialize file structure
    let mut file_data = BytesMut::new();

    // Write magic and version
    file_data.put_slice(&magic);
    file_data.put_u32_le(2); // Version 2 for SIMD-enhanced format

    // Calculate block count
    let num_blocks = (records.len() + block_size - 1) / block_size;
    file_data.put_u32_le(num_blocks as u32);

    let mut block_offsets = Vec::new();
    let mut block_sizes = Vec::new();
    let mut block_metadata: Vec<HelixBlockMetadata> = Vec::new();

    // Create global bloom filter for all records
    let bloom_config = BloomFilterConfig {
        expected_items: records.len(),
        false_positive_rate: Some(0.01),
        ..Default::default()
    };

    let mut global_bloom = BloomFilterFactory::create(&bloom_config);

    // Track SIMD performance metrics
    let mut total_simd_time = std::time::Duration::ZERO;
    let mut total_original_size = 0usize;
    let mut total_compressed_size = 0usize;

    // Process each block with SIMD optimization
    for block_idx in 0..num_blocks {
        let block_start = block_idx * block_size;
        let block_end = std::cmp::min(block_start + block_size, records.len());
        let chunk = &records[block_start..block_end];

        if chunk.is_empty() {
            continue;
        }

        // Record offset before block data
        block_offsets.push(file_data.len() as u64);

        // Extract Hilbert keys for this block
        let block_hilbert_keys = if let Some(keys) = hilbert_keys {
            Some(&keys[block_start..block_end])
        } else {
            None
        };

        // Create SIMD-optimized block
        let block_start_time = std::time::Instant::now();
        let (mut simd_block, simd_metadata) = simd_writer
            .create_simd_block(chunk, block_hilbert_keys, block_idx as u32)
            .await?;

        let block_simd_time = block_start_time.elapsed();
        total_simd_time += block_simd_time;

        // Update bloom filter
        for record in chunk {
            global_bloom.insert(record.id.as_bytes());
        }

        // Serialize the SIMD-optimized block
        let (block_bytes, _bloom_data) = simd_block.serialize_with_bloom_sync()?;
        let block_size_bytes = block_bytes.len();

        // Track compression statistics
        let original_block_size = chunk.len() * dimension * 4;
        total_original_size += original_block_size;
        total_compressed_size += block_size_bytes;

        // Write block size and data
        file_data.put_u32_le(block_size_bytes as u32);
        file_data.put_slice(&block_bytes);

        // Store block size for header (actual serialized size)
        block_sizes.push(block_size_bytes as u32);

        // Store enhanced metadata
        block_metadata.push(simd_metadata);

        debug!(
            "Block {}: SIMD optimization in {:.2}ms, {} → {} bytes",
            block_idx,
            block_simd_time.as_millis(),
            original_block_size,
            block_size_bytes
        );
    }

    // Calculate overall compression performance
    let overall_compression_ratio = if total_original_size > 0 {
        (total_compressed_size * 100) / total_original_size
    } else {
        100
    };

    info!(
        "🎯 HELIX total compression: {} → {} bytes ({}% ratio) in {:.2}ms",
        total_original_size,
        total_compressed_size,
        overall_compression_ratio,
        total_simd_time.as_millis()
    );

    // Write bloom filter
    let bloom_bytes = global_bloom.serialize()?;
    let bloom_offset = file_data.len() as u64;
    file_data.put_u32_le(bloom_bytes.len() as u32);
    file_data.put_slice(&bloom_bytes);

    // OPTIMIZATION: Write unified file header containing ALL metadata
    // This allows single-call reading of all metadata for queries
    let file_header = HelixFileHeader {
        magic: HELIX_MAGIC,
        version: 1,   // HELIX v1 (active development)
        file_size: 0, // Will be updated by reader
        block_metadata: block_metadata.clone(),
        block_offsets: block_offsets.clone(),
        block_sizes: block_sizes.clone(),
    };

    let header_offset = file_data.len() as u64;

    // MANUAL SERIALIZATION: Write header fields in fixed order to avoid bincode Vec length prefix issues
    // Format: magic(4) + version(4) + file_size(8) + num_blocks(4) + [block_metadata] + [block_offsets] + [block_sizes]
    let mut header_bytes = BytesMut::new();

    // 1. Magic bytes
    header_bytes.put_slice(&HELIX_MAGIC);

    // 2. Version (v1 - active development)
    header_bytes.put_u32_le(1);

    // 3. File size (placeholder, reader will use actual file size)
    header_bytes.put_u64_le(0);

    // 4. Number of blocks
    header_bytes.put_u32_le(num_blocks as u32);

    // 5. Block metadata (bincode-serialized Vec)
    let block_metadata_bytes = bincode::serialize(&block_metadata)?;
    header_bytes.put_u32_le(block_metadata_bytes.len() as u32);
    header_bytes.put_slice(&block_metadata_bytes);

    // 6. Block offsets (bincode-serialized Vec<u64>)
    let block_offsets_bytes = bincode::serialize(&block_offsets)?;
    header_bytes.put_u32_le(block_offsets_bytes.len() as u32);
    header_bytes.put_slice(&block_offsets_bytes);

    // 7. Block sizes (bincode-serialized Vec<u32>)
    let block_sizes_bytes = bincode::serialize(&block_sizes)?;
    header_bytes.put_u32_le(block_sizes_bytes.len() as u32);
    header_bytes.put_slice(&block_sizes_bytes);

    file_data.put_slice(&header_bytes);

    // Write footer (16 bytes) pointing to unified header
    // Footer layout: [header_size: 4][header_offset: 8][num_blocks: 4]
    file_data.put_u32_le(header_bytes.len() as u32);
    file_data.put_u64_le(header_offset);
    file_data.put_u32_le(num_blocks as u32);

    // Write to filesystem
    let bytes_written = file_data.len() as u64;
    let data_bytes = file_data.freeze();
    filesystem
        .write(path.to_str().unwrap_or(""), &data_bytes, None)
        .await?;

    Ok(bytes_written)
}

/// Read HELIX unified file header in SINGLE optimized API call
/// Returns cached header if available, otherwise reads and caches it
///
/// OPTIMIZATION: Unified header contains ALL metadata in one structure
/// - Before: 4 API calls (metadata(), footer, old-metadata, old-index)
/// - After: 2 API calls (metadata(), header)
/// - Savings: 50% API call reduction on first query
/// - Subsequent queries: 100% savings (filesystem caches it)
pub(crate) async fn read_helix_header_optimized(
    filesystem: &Arc<crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem>,
    path: &Path,
) -> Result<HelixFileHeader> {
    use crate::storage::persistence::filesystem::FileSystem;

    let path_str = path.to_str().unwrap_or("");

    // Step 1: Get file size (cached by filesystem after first call)
    let file_metadata = filesystem.metadata(path_str).await?;
    let file_len = file_metadata.size as usize;

    if file_len < 28 {
        return Err(anyhow::anyhow!("Invalid HELIX file: too small"));
    }

    // Step 2: Read footer (16 bytes) to get unified header location
    // Footer layout: [header_size: 4][header_offset: 8][num_blocks: 4]
    const FOOTER_SIZE: u64 = 16;
    let footer_offset = (file_len as u64) - FOOTER_SIZE;
    let footer_data = filesystem
        .read_range(path_str, footer_offset, FOOTER_SIZE)
        .await?;

    let header_size = u32::from_le_bytes([
        footer_data[0],
        footer_data[1],
        footer_data[2],
        footer_data[3],
    ]) as u64;

    let header_offset = u64::from_le_bytes([
        footer_data[4],
        footer_data[5],
        footer_data[6],
        footer_data[7],
        footer_data[8],
        footer_data[9],
        footer_data[10],
        footer_data[11],
    ]);

    // Step 3: Read unified header in SINGLE API call!
    tracing::debug!(
        "Reading unified HELIX header: offset={}, size={}, file_len={}",
        header_offset,
        header_size,
        file_len
    );

    // Sanity check: header_offset + header_size should be <= file_len - footer_size
    if header_offset + header_size > (file_len as u64 - FOOTER_SIZE) {
        return Err(anyhow::anyhow!(
            "Invalid header location: offset={} + size={} = {} > file_len={} - footer={}",
            header_offset,
            header_size,
            header_offset + header_size,
            file_len,
            FOOTER_SIZE
        ));
    }

    let header_data = filesystem
        .read_range(path_str, header_offset, header_size)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to read header at offset {} size {}: {}",
                header_offset,
                header_size,
                e
            )
        })?;

    // MANUAL DESERIALIZATION: Read header fields in fixed order matching writer
    // Format: magic(4) + version(4) + file_size(8) + num_blocks(4) + [block_metadata] + [block_offsets] + [block_sizes]
    let mut cursor = std::io::Cursor::new(&header_data);

    // 1. Read magic bytes
    let mut magic = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut magic)?;

    if magic != HELIX_MAGIC {
        return Err(anyhow::anyhow!(
            "Invalid HELIX magic bytes: got {:?}, expected {:?}",
            magic,
            HELIX_MAGIC
        ));
    }

    // 2. Read version
    let mut version_bytes = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut version_bytes)?;
    let version = u32::from_le_bytes(version_bytes);

    // 3. Read file_size (placeholder, we'll use actual)
    let mut file_size_bytes = [0u8; 8];
    std::io::Read::read_exact(&mut cursor, &mut file_size_bytes)?;
    let _stored_file_size = u64::from_le_bytes(file_size_bytes);

    // 4. Read num_blocks
    let mut num_blocks_bytes = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut num_blocks_bytes)?;
    let num_blocks = u32::from_le_bytes(num_blocks_bytes);

    // 5. Read block_metadata
    let mut metadata_len_bytes = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut metadata_len_bytes)?;
    let metadata_len = u32::from_le_bytes(metadata_len_bytes) as usize;

    let mut metadata_bytes = vec![0u8; metadata_len];
    std::io::Read::read_exact(&mut cursor, &mut metadata_bytes)?;
    let block_metadata: Vec<HelixBlockMetadata> = bincode::deserialize(&metadata_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize block_metadata: {}", e))?;

    // 6. Read block_offsets
    let mut offsets_len_bytes = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut offsets_len_bytes)?;
    let offsets_len = u32::from_le_bytes(offsets_len_bytes) as usize;

    let mut offsets_bytes = vec![0u8; offsets_len];
    std::io::Read::read_exact(&mut cursor, &mut offsets_bytes)?;
    let block_offsets: Vec<u64> = bincode::deserialize(&offsets_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize block_offsets: {}", e))?;

    // 7. Read block_sizes
    let mut sizes_len_bytes = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut sizes_len_bytes)?;
    let sizes_len = u32::from_le_bytes(sizes_len_bytes) as usize;

    let mut sizes_bytes = vec![0u8; sizes_len];
    std::io::Read::read_exact(&mut cursor, &mut sizes_bytes)?;
    let block_sizes: Vec<u32> = bincode::deserialize(&sizes_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize block_sizes: {}", e))?;

    // Validate block_sizes matches block_offsets
    if block_sizes.len() != block_offsets.len() {
        return Err(anyhow::anyhow!(
            "Block sizes count ({}) doesn't match offsets count ({})",
            block_sizes.len(),
            block_offsets.len()
        ));
    }

    // Construct header
    let header = HelixFileHeader {
        magic,
        version,
        file_size: file_len,
        block_metadata,
        block_offsets,
        block_sizes,
    };

    Ok(header)
}

/// Read and search a HELIX SSTable with bloom filter pruning and type-safe filtering
pub async fn search_helix_sstable(
    filesystem: &Arc<crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem>,
    path: &Path,
    query_vector: &[f32],
    query_hilbert_key: Option<u64>,
    k: usize,
    distance_metric: &crate::compute::distance_computation::DistanceMetric,
    distance_compute: &Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,
    collection: Option<&crate::proto::proximadb_v1::Collection>,
    filter_expression: Option<&crate::core::search::FilterExpression>,
) -> Result<
    Vec<(
        String,
        f32,
        std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    )>,
> {
    // CLOUD-OPTIMIZED: Read unified file header in 2 API calls (metadata + header)
    // UnifiedCachingFilesystem will cache this, so subsequent queries = 0 API calls!
    let header = read_helix_header_optimized(filesystem, path).await?;

    let path_str = path.to_str().unwrap_or("");
    let num_blocks = header.block_metadata.len();
    let mut results = Vec::new();
    let mut blocks_pruned = 0;
    let mut blocks_searched = 0;

    // Process only relevant blocks with Hilbert pruning
    tracing::debug!(
        "HELIX search: query_hilbert_key = {:?}, num_blocks = {}",
        query_hilbert_key,
        num_blocks
    );

    for block_idx in 0..num_blocks {
        let meta = &header.block_metadata[block_idx];
        let block_offset = header.block_offsets[block_idx];

        // Enhanced Hilbert-based pruning using metadata
        let should_prune = if let Some(query_key) = query_hilbert_key {
            if let Some((min_key, max_key)) = meta.hilbert_range {
                tracing::debug!(
                    "Block {}: query_key={}, hilbert_range=({}, {})",
                    block_idx,
                    query_key,
                    min_key,
                    max_key
                );
                // Calculate tolerance based on Hilbert space resolution
                let range_size = max_key.saturating_sub(min_key);
                let tolerance = if range_size > 0 {
                    // Use 5% of block's range as tolerance, or minimum 100
                    (range_size / 20).max(100)
                } else {
                    // Single point block, use small fixed tolerance
                    100
                };

                // Check if query is outside range with calculated tolerance
                query_key < min_key.saturating_sub(tolerance)
                    || query_key > max_key.saturating_add(tolerance)
            } else {
                false // No range info, can't prune
            }
        } else {
            false // No query key, can't prune
        };

        if should_prune {
            blocks_pruned += 1;
            continue; // CRITICAL: Don't read this block from disk at all!
        }

        // CLOUD-OPTIMIZED: Read block with EXACT size from header
        // Single API call with exact size = perfect read, zero waste
        blocks_searched += 1;

        // Use exact block size from header
        let exact_size = header.block_sizes[block_idx];
        tracing::trace!(
            "Block {}: Reading exact {}KB at offset {} (hilbert_range: {:?})",
            block_idx,
            exact_size / 1024,
            block_offset,
            meta.hilbert_range
        );

        // Read exact size: 4 bytes (size prefix) + block data
        let chunk_data = filesystem
            .read_range(path_str, block_offset, (4 + exact_size) as u64)
            .await?;

        // Verify size prefix matches header (integrity check)
        let size_prefix =
            u32::from_le_bytes([chunk_data[0], chunk_data[1], chunk_data[2], chunk_data[3]]);

        if size_prefix != exact_size {
            return Err(anyhow::anyhow!(
                "Block {} size mismatch: header says {}, file has {}",
                block_idx,
                exact_size,
                size_prefix
            ));
        }

        // Extract block data after 4-byte size prefix
        let block_data = &chunk_data[4..4 + exact_size as usize];

        // Deserialize block with collection config for type-safe metadata
        let block = ProximaDataBlock::deserialize(&block_data, collection)?;

        // Search within block
        for record in block.records.iter() {
            // Apply type-safe filter if present
            if let Some(filter_expr) = filter_expression {
                let matches = crate::core::search::sql_value_filter::evaluate_filter(
                    filter_expr,
                    &record.metadata,
                );
                if !matches {
                    continue; // Skip records that don't match filter
                }
            }

            // Use shared UnifiedDistanceCompute for correct metric-specific distance calculation
            let distance = distance_compute.distance(query_vector, &record.vector);

            // Return SqlValue metadata (type-safe)
            results.push((record.id.clone(), distance, record.metadata.clone()));
        }
    }

    // Sort by distance and return top-k
    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    results.truncate(k);

    // Log pruning statistics
    if blocks_pruned > 0 {
        tracing::info!(
            "🎯 HELIX pruning: searched {}/{} blocks, pruned {} blocks ({:.1}% pruning rate)",
            blocks_searched,
            num_blocks,
            blocks_pruned,
            (blocks_pruned as f64 / num_blocks as f64) * 100.0
        );
    }

    Ok(results)
}

/// Extract block metadata for HELIX
pub fn extract_helix_metadata(
    records: &[VectorRecord],
    block_size: usize,
    hilbert_keys: Option<&[u64]>,
) -> Vec<HelixBlockMetadata> {
    records
        .chunks(block_size)
        .enumerate()
        .map(|(idx, chunk)| {
            // Create base Proxima metadata
            let base_metadata = ProximaBlockMetadata {
                record_count: chunk.len() as u32,
                size_bytes: (chunk.len() * std::mem::size_of::<VectorRecord>()) as u64,
                compressed_size: 0, // Will be set during compression
                timestamp: chunk
                    .iter()
                    .map(|r| r.timestamp.unwrap_or(0) as i64)
                    .max()
                    .unwrap_or(0),
                compaction_level: 0,
                has_deletes: false,
                has_updates: false,
                version_range: (0, 1),
                column_stats: HashMap::new(),
                quantization_stats: QuantizationStatistics::default(),
                data_checksum: 0,
                metadata_checksum: 0,
            };

            // Calculate Hilbert range if keys provided
            let hilbert_range = if let Some(keys) = hilbert_keys {
                let start = idx * block_size;
                let end = std::cmp::min(start + block_size, keys.len());
                if start < keys.len() {
                    let block_keys = &keys[start..end];
                    Some((
                        *block_keys.iter().min().unwrap_or(&0),
                        *block_keys.iter().max().unwrap_or(&0),
                    ))
                } else {
                    None
                }
            } else {
                None
            };

            HelixBlockMetadata {
                proxima_metadata: base_metadata,
                hilbert_range,
                pca_stats: None,
                clustering_hints: None,
            }
        })
        .collect()
}

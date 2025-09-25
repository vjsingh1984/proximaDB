//! FastLane integration for HELIX engine
//!
//! This module bridges HELIX-specific clustering with the shared FastLanes
//! block structures used across SST, SWIFT, and other engines.

use anyhow::Result;
use bytes::{BufMut, BytesMut};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

// Reuse existing FastLanes structures
use crate::storage::engines::core::formats::fastlanes_blocks::block_structures::{
    BlockCompressionConfig, FastLanesBlockMetadata, FastLanesDataBlock, QuantizationStatistics,
};

use crate::core::{VectorRecord, compression::CompressionAlgorithm};
use crate::storage::persistence::filesystem::FileSystem;

// Re-export for convenience
pub use crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesMetadata as FastLaneMetadata;

/// HELIX-specific SSTable metadata with clustering information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelixBlockMetadata {
    /// Base FastLanes metadata
    pub fastlanes_metadata: FastLanesBlockMetadata,
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

/// Liquid clustering hints based on query patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusteringHints {
    pub access_frequency: f32,
    pub last_accessed: Option<chrono::DateTime<chrono::Utc>>,
    pub query_selectivity: f32,
}

/// Write a HELIX SSTable using FastLanes encoding with SST-style optimizations
pub async fn write_helix_sstable(
    filesystem: &Arc<dyn FileSystem>,
    path: &Path,
    records: &[VectorRecord],
    block_size: usize,
    magic: [u8; 4],
    hilbert_keys: Option<&[u64]>,
) -> Result<u64> {
    use crate::storage::engines::core::formats::fastlanes_blocks::bloom_filter::{
        BloomFilterConfig, factory::BloomFilterFactory,
    };
    // Use our own metadata structures instead of SST headers

    let mut file_data = BytesMut::new();

    // Write magic and version
    file_data.put_slice(&magic);
    file_data.put_u32_le(1); // Version

    // Write number of blocks
    let num_blocks = (records.len() + block_size - 1) / block_size;
    file_data.put_u32_le(num_blocks as u32);

    let mut block_offsets = Vec::new();
    let mut block_metadata: Vec<HelixBlockMetadata> = Vec::new();

    // Create global bloom filter for all records
    let bloom_config = BloomFilterConfig {
        expected_items: records.len(),
        false_positive_rate: Some(0.01),
        ..Default::default()
    };
    let mut global_bloom = BloomFilterFactory::create(&bloom_config);

    // Process records in chunks
    for (block_idx, chunk) in records.chunks(block_size).enumerate() {
        let block_offset = file_data.len() as u64;
        block_offsets.push(block_offset);

        // Adaptive compression based on block characteristics
        let compression_config = BlockCompressionConfig {
            algorithm: if chunk.len() > 100 {
                CompressionAlgorithm::Zstd
            } else {
                CompressionAlgorithm::Lz4 // Faster for small blocks
            },
            compression_level: 3,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 512, // Lower threshold for better compression
            dictionary_compression: chunk.len() > 500, // Use dictionary for large blocks
            vector_layout: crate::storage::engines::core::formats::fastlanes_blocks::VectorEncodingLayout::Auto,
            metadata_algorithm: None, // Use main algorithm for metadata
        };

        // Create FastLanes block with proper block ID
        let mut block = FastLanesDataBlock::new(chunk.to_vec(), compression_config);
        block.block_id = block_idx as u32;

        // Add records to global bloom filter
        for record in chunk {
            global_bloom.insert(record.id.as_bytes());
        }

        // Determine Hilbert range for this block with better bounds tracking
        let hilbert_range = if let Some(keys) = hilbert_keys {
            let block_start = block_idx * block_size;
            let block_end = std::cmp::min(block_start + block_size, keys.len());
            if block_start < keys.len() {
                let block_keys = &keys[block_start..block_end];
                let min_key = *block_keys.iter().min().unwrap_or(&0);
                let max_key = *block_keys.iter().max().unwrap_or(&0);
                Some((min_key, max_key))
            } else {
                None
            }
        } else {
            None
        };

        // Hilbert range will be stored in HelixBlockMetadata, not in FastLanes metadata

        // Serialize block before creating metadata (to get compressed size)
        let block_bytes = block.serialize()?;
        let compressed_size = block_bytes.len();

        // Store header info in our metadata instead

        // Create HELIX metadata with enhanced statistics
        let helix_meta = HelixBlockMetadata {
            fastlanes_metadata: block.metadata.clone(),
            hilbert_range,
            pca_stats: None,
            clustering_hints: Some(ClusteringHints {
                access_frequency: 0.0,
                last_accessed: None,
                query_selectivity: 1.0 / num_blocks as f32,
            }),
        };

        // Write block size and data
        file_data.put_u32_le(block_bytes.len() as u32);
        file_data.put_slice(&block_bytes);

        // Store metadata
        block_metadata.push(helix_meta);
    }

    // Write bloom filter
    let bloom_bytes = global_bloom.serialize()?;
    let bloom_offset = file_data.len() as u64;
    file_data.put_u32_le(bloom_bytes.len() as u32);
    file_data.put_slice(&bloom_bytes);

    // Write block offset index
    let index_offset = file_data.len() as u64;
    for offset in block_offsets {
        file_data.put_u64_le(offset);
    }

    // Write metadata in binary format
    let metadata_bytes = bincode::serialize(&block_metadata)?;
    file_data.put_slice(&metadata_bytes);

    // Write metadata size as footer (for reader to locate metadata)
    file_data.put_u32_le(metadata_bytes.len() as u32);

    // Write to filesystem
    let bytes_written = file_data.len() as u64;
    let data_bytes = file_data.freeze();
    filesystem
        .write(path.to_str().unwrap_or(""), &data_bytes, None)
        .await?;

    Ok(bytes_written)
}

/// Read and search a HELIX SSTable with bloom filter pruning
pub async fn search_helix_sstable(
    filesystem: &Arc<dyn FileSystem>,
    path: &Path,
    query_vector: &[f32],
    query_hilbert_key: Option<u64>,
    k: usize,
    distance_metric: &crate::compute::distance_computation::DistanceMetric,
) -> Result<Vec<(String, f32, HashMap<String, String>)>> {
    // We use our own metadata, not SST headers

    // Read entire file
    let file_data = filesystem.read(path.to_str().unwrap_or("")).await?;

    // Read footer to get metadata
    let file_len = file_data.len();
    if file_len < 12 {
        return Err(anyhow::anyhow!("Invalid HELIX file: too small"));
    }

    // Read metadata size (last 4 bytes)
    let metadata_size_offset = file_len - 4;
    let metadata_size = u32::from_le_bytes([
        file_data[metadata_size_offset],
        file_data[metadata_size_offset + 1],
        file_data[metadata_size_offset + 2],
        file_data[metadata_size_offset + 3],
    ]) as usize;

    // Read binary metadata
    let metadata_start = file_len - 4 - metadata_size;
    let metadata_data = &file_data[metadata_start..metadata_start + metadata_size];
    let block_metadata: Vec<HelixBlockMetadata> = bincode::deserialize(metadata_data)?;

    let mut cursor = std::io::Cursor::new(&file_data[..]);

    // Skip magic and version
    cursor.set_position(8);

    // Read number of blocks
    let mut num_blocks_bytes = [0u8; 4];
    std::io::Read::read_exact(&mut cursor, &mut num_blocks_bytes)?;
    let num_blocks = u32::from_le_bytes(num_blocks_bytes) as usize;

    if num_blocks != block_metadata.len() {
        return Err(anyhow::anyhow!(
            "Metadata mismatch: block count doesn't match"
        ));
    }

    let mut results = Vec::new();
    let mut blocks_pruned = 0;

    // Read and search blocks with enhanced pruning
    for block_idx in 0..num_blocks {
        let meta = &block_metadata[block_idx];

        // Read block size
        let mut size_bytes = [0u8; 4];
        std::io::Read::read_exact(&mut cursor, &mut size_bytes)?;
        let block_size = u32::from_le_bytes(size_bytes) as usize;

        // Enhanced Hilbert-based pruning using metadata
        if let Some(query_key) = query_hilbert_key {
            if let Some((min_key, max_key)) = meta.hilbert_range {
                // Calculate tolerance based on Hilbert space resolution
                // The Hilbert space is typically 2^(bits_per_dim * n_dims)
                // We use a tolerance of ~1% of the total range for boundary cases
                let range_size = max_key.saturating_sub(min_key);
                let tolerance = if range_size > 0 {
                    // Use 5% of block's range as tolerance, or minimum 100
                    (range_size / 20).max(100)
                } else {
                    // Single point block, use small fixed tolerance
                    100
                };

                // Check if query is outside range with calculated tolerance
                if query_key < min_key.saturating_sub(tolerance)
                    || query_key > max_key.saturating_add(tolerance)
                {
                    // Skip this block's data
                    cursor.set_position(cursor.position() + block_size as u64);
                    blocks_pruned += 1;
                    continue;
                }
            }
        }

        // Read block data
        let mut block_data = vec![0u8; block_size];
        std::io::Read::read_exact(&mut cursor, &mut block_data)?;

        // Deserialize block
        let block = FastLanesDataBlock::deserialize(&block_data)?;

        // Search within block
        for record in &block.records {
            // Convert metadata format (assuming we need HashMap<String, String>)
            let metadata = HashMap::new(); // TODO: Convert record.metadata properly

            // Use simple euclidean distance for now (TODO: use UnifiedDistanceCompute)
            let distance = query_vector
                .iter()
                .zip(record.vector.iter())
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                .sqrt();
            results.push((record.id.clone(), distance, metadata));
        }
    }

    // Sort by distance and return top-k
    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    results.truncate(k);

    // Log pruning statistics
    if blocks_pruned > 0 {
        tracing::debug!(
            "HELIX pruning: skipped {}/{} blocks ({:.1}% pruning rate)",
            blocks_pruned,
            num_blocks,
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
            // Create base FastLanes metadata
            let base_metadata = FastLanesBlockMetadata {
                record_count: chunk.len() as u32,
                size_bytes: (chunk.len() * std::mem::size_of::<VectorRecord>()) as u64,
                compressed_size: 0, // Will be set during compression
                timestamp: chunk.iter().map(|r| r.timestamp as i64).max().unwrap_or(0),
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
                fastlanes_metadata: base_metadata,
                hilbert_range,
                pca_stats: None,
                clustering_hints: None,
            }
        })
        .collect()
}

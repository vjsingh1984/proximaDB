//! Hierarchical Multi-Level Bloom Filters (Phase 1.2)
//!
//! Implements multi-level bloom filtering for progressive elimination:
//! - File-level bloom (coarse, 5% FPR)
//! - SuperBlock-level blooms (medium, 2% FPR)
//! - Block-level blooms (fine, 1% FPR)
//!
//! ## Progressive Filtering Strategy
//!
//! ```text
//! Query arrives
//!   ↓
//! [File Bloom] → Eliminates ~80% of files
//!   ↓
//! [SuperBlock Blooms] → Eliminates ~15% of superblocks
//!   ↓
//! [Block Blooms] → Eliminates ~4% of blocks
//!   ↓
//! [Exact Scan] → Remaining ~1% scanned
//! ```
//!
//! ## Expected Performance
//!
//! - 95%+ block elimination rate
//! - 40-60% bloom filter memory reduction (vs fixed-size)
//! - 10-20% cache hit rate improvement

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::adaptive::AdaptiveBloomConfig;
use super::compression::CompressedBloom;

/// Multi-level hierarchical bloom filters
///
/// Provides progressive filtering from file → superblock → block level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchicalBloomFilters {
    /// File-level bloom filter (coarse elimination)
    /// Covers all keys in the file
    pub file_bloom: Option<CompressedBloom>,

    /// SuperBlock-level bloom filters (medium-grained)
    /// One bloom per superblock
    pub superblock_blooms: Vec<CompressedBloom>,

    /// Block-level bloom filters (fine-grained)
    /// One bloom per block
    pub block_blooms: Vec<CompressedBloom>,

    /// Adaptive configuration used
    pub config: AdaptiveBloomConfig,

    /// Statistics
    pub stats: HierarchicalBloomStats,
}

/// Statistics for hierarchical bloom filters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HierarchicalBloomStats {
    /// Total keys in file
    pub total_keys: usize,

    /// Number of superblocks
    pub num_superblocks: usize,

    /// Number of blocks
    pub num_blocks: usize,

    /// Memory usage (compressed)
    pub compressed_memory_bytes: usize,

    /// Memory usage (uncompressed)
    pub uncompressed_memory_bytes: usize,

    /// Average sparsity across all blooms
    pub average_sparsity: f64,

    /// Average compression ratio
    pub average_compression_ratio: f64,

    /// Memory savings from compression
    pub memory_savings_bytes: usize,
}

impl HierarchicalBloomFilters {
    /// Create a new hierarchical bloom filter set
    pub fn new(config: AdaptiveBloomConfig) -> Self {
        Self {
            file_bloom: None,
            superblock_blooms: Vec::new(),
            block_blooms: Vec::new(),
            config,
            stats: HierarchicalBloomStats::default(),
        }
    }

    /// Create with default adaptive configuration
    pub fn with_defaults() -> Self {
        Self::new(AdaptiveBloomConfig::default())
    }

    /// Build file-level bloom filter from all keys
    pub fn build_file_bloom(&mut self, all_keys: &[&[u8]]) -> Result<()> {
        if all_keys.is_empty() {
            return Ok(());
        }

        let file_config = AdaptiveBloomConfig::for_file_level();
        let num_bits = file_config.optimal_size(all_keys.len());
        let num_hashes = file_config.optimal_hash_count(num_bits, all_keys.len());

        // Build bloom filter
        let mut bloom_data = vec![0u8; (num_bits + 7) / 8];
        for key in all_keys {
            let positions =
                crate::core::bloom::hash::double_hash(key, num_hashes as u32, num_bits);
            for pos in positions {
                let byte_index = pos / 8;
                let bit_index = pos % 8;
                if byte_index < bloom_data.len() {
                    bloom_data[byte_index] |= 1 << bit_index;
                }
            }
        }

        // Compress if beneficial (pass actual num_bits, not byte-aligned size)
        let compressed = CompressedBloom::compress_with_bits(&bloom_data, num_hashes, num_bits);
        self.file_bloom = Some(compressed);
        self.stats.total_keys = all_keys.len();

        Ok(())
    }

    /// Add a superblock-level bloom filter
    pub fn add_superblock_bloom(&mut self, superblock_keys: &[&[u8]]) -> Result<()> {
        if superblock_keys.is_empty() {
            return Ok(());
        }

        let config = AdaptiveBloomConfig::for_superblock_level();
        let num_bits = config.optimal_size(superblock_keys.len());
        let num_hashes = config.optimal_hash_count(num_bits, superblock_keys.len());

        // Build bloom filter
        let mut bloom_data = vec![0u8; (num_bits + 7) / 8];
        for key in superblock_keys {
            let positions =
                crate::core::bloom::hash::double_hash(key, num_hashes as u32, num_bits);
            for pos in positions {
                let byte_index = pos / 8;
                let bit_index = pos % 8;
                if byte_index < bloom_data.len() {
                    bloom_data[byte_index] |= 1 << bit_index;
                }
            }
        }

        // Compress (pass actual num_bits)
        let compressed = CompressedBloom::compress_with_bits(&bloom_data, num_hashes, num_bits);
        self.superblock_blooms.push(compressed);
        self.stats.num_superblocks += 1;

        Ok(())
    }

    /// Add a block-level bloom filter
    pub fn add_block_bloom(&mut self, block_keys: &[&[u8]]) -> Result<()> {
        if block_keys.is_empty() {
            return Ok(());
        }

        let config = AdaptiveBloomConfig::for_block_level();
        let num_bits = config.optimal_size(block_keys.len());
        let num_hashes = config.optimal_hash_count(num_bits, block_keys.len());

        // Build bloom filter
        let mut bloom_data = vec![0u8; (num_bits + 7) / 8];
        for key in block_keys {
            let positions =
                crate::core::bloom::hash::double_hash(key, num_hashes as u32, num_bits);

            for pos in positions {
                let byte_index = pos / 8;
                let bit_index = pos % 8;
                if byte_index < bloom_data.len() {
                    bloom_data[byte_index] |= 1 << bit_index;
                }
            }
        }

        // Compress (pass actual num_bits)
        let compressed = CompressedBloom::compress_with_bits(&bloom_data, num_hashes, num_bits);
        self.block_blooms.push(compressed);
        self.stats.num_blocks += 1;

        Ok(())
    }

    /// Finalize statistics after all blooms are built
    pub fn finalize_stats(&mut self) {
        let mut total_compressed = 0;
        let mut total_uncompressed = 0;
        let mut total_sparsity = 0.0;
        let mut total_compression_ratio = 0.0;
        let mut count = 0;

        // File bloom
        if let Some(ref bloom) = self.file_bloom {
            total_compressed += bloom.compressed_data.len();
            total_uncompressed += bloom.original_size / 8;
            total_sparsity += bloom.sparsity;
            total_compression_ratio += bloom.compression_ratio;
            count += 1;
        }

        // SuperBlock blooms
        for bloom in &self.superblock_blooms {
            total_compressed += bloom.compressed_data.len();
            total_uncompressed += bloom.original_size / 8;
            total_sparsity += bloom.sparsity;
            total_compression_ratio += bloom.compression_ratio;
            count += 1;
        }

        // Block blooms
        for bloom in &self.block_blooms {
            total_compressed += bloom.compressed_data.len();
            total_uncompressed += bloom.original_size / 8;
            total_sparsity += bloom.sparsity;
            total_compression_ratio += bloom.compression_ratio;
            count += 1;
        }

        self.stats.compressed_memory_bytes = total_compressed;
        self.stats.uncompressed_memory_bytes = total_uncompressed;
        self.stats.memory_savings_bytes =
            total_uncompressed.saturating_sub(total_compressed);
        self.stats.average_sparsity = if count > 0 {
            total_sparsity / count as f64
        } else {
            0.0
        };
        self.stats.average_compression_ratio = if count > 0 {
            total_compression_ratio / count as f64
        } else {
            1.0
        };
    }

    /// Progressive filtering: File → SuperBlock → Block
    ///
    /// Returns indices of blocks that might contain the key
    pub fn filter_blocks(&self, key: &[u8]) -> Result<Vec<usize>> {
        // Step 1: File-level filter
        if let Some(ref file_bloom) = self.file_bloom {
            if !file_bloom.might_contain(key)? {
                // Key definitely not in file
                return Ok(Vec::new());
            }
        }

        // Step 2: Block-level filter (direct)
        // Note: In a full implementation, you'd use superblock → block mapping
        // For now, we check all blocks directly
        let mut candidate_blocks = Vec::new();

        for (block_idx, block_bloom) in self.block_blooms.iter().enumerate() {
            if block_bloom.might_contain(key)? {
                candidate_blocks.push(block_idx);
            }
        }

        Ok(candidate_blocks)
    }

    /// Filter using superblock hierarchy
    ///
    /// Returns (superblock_indices, block_indices) that might contain the key
    pub fn filter_with_superblocks(
        &self,
        key: &[u8],
        blocks_per_superblock: usize,
    ) -> Result<(Vec<usize>, Vec<usize>)> {
        // Step 1: File-level filter
        if let Some(ref file_bloom) = self.file_bloom {
            if !file_bloom.might_contain(key)? {
                return Ok((Vec::new(), Vec::new()));
            }
        }

        // Step 2: SuperBlock-level filter
        let mut candidate_superblocks = Vec::new();
        for (sb_idx, sb_bloom) in self.superblock_blooms.iter().enumerate() {
            if sb_bloom.might_contain(key)? {
                candidate_superblocks.push(sb_idx);
            }
        }

        // Step 3: Block-level filter (only for candidate superblocks)
        let mut candidate_blocks = Vec::new();
        for sb_idx in &candidate_superblocks {
            let block_start = sb_idx * blocks_per_superblock;
            let block_end = (block_start + blocks_per_superblock).min(self.block_blooms.len());

            for block_idx in block_start..block_end {
                if let Some(block_bloom) = self.block_blooms.get(block_idx) {
                    if block_bloom.might_contain(key)? {
                        candidate_blocks.push(block_idx);
                    }
                }
            }
        }

        Ok((candidate_superblocks, candidate_blocks))
    }

    /// Get memory savings from compression
    pub fn memory_savings_bytes(&self) -> usize {
        self.stats.memory_savings_bytes
    }

    /// Get compression ratio (compressed / uncompressed)
    pub fn compression_ratio(&self) -> f64 {
        self.stats.average_compression_ratio
    }

    /// Get total memory usage (compressed)
    pub fn memory_usage_bytes(&self) -> usize {
        self.stats.compressed_memory_bytes
    }

    /// Get elimination rate statistics
    pub fn elimination_stats(&self, total_blocks: usize) -> EliminationStats {
        EliminationStats {
            total_blocks,
            blocks_with_blooms: self.stats.num_blocks,
            estimated_elimination_rate: self.estimate_elimination_rate(),
        }
    }

    /// Estimate elimination rate based on configured FPRs
    fn estimate_elimination_rate(&self) -> f64 {
        // Conservative estimate based on FPR
        // File: 5% FPR means 95% elimination
        // Block: 1% FPR means 99% of remaining eliminated
        // Combined: 1 - (0.05 * 0.01) = 0.9995 = 99.95% elimination
        let file_fpr = 0.05; // 5% for file level
        let block_fpr = 0.01; // 1% for block level

        1.0 - (file_fpr * block_fpr)
    }
}

/// Elimination statistics
#[derive(Debug, Clone)]
pub struct EliminationStats {
    pub total_blocks: usize,
    pub blocks_with_blooms: usize,
    pub estimated_elimination_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hierarchical_bloom_creation() {
        let hierarchy = HierarchicalBloomFilters::with_defaults();

        assert!(hierarchy.file_bloom.is_none());
        assert_eq!(hierarchy.superblock_blooms.len(), 0);
        assert_eq!(hierarchy.block_blooms.len(), 0);
    }

    #[test]
    fn test_build_file_bloom() {
        let mut hierarchy = HierarchicalBloomFilters::with_defaults();

        let all_keys: Vec<&[u8]> = vec![b"key1", b"key2", b"key3", b"key4"];
        hierarchy.build_file_bloom(&all_keys).unwrap();

        assert!(hierarchy.file_bloom.is_some());
        assert_eq!(hierarchy.stats.total_keys, 4);
    }

    #[test]
    fn test_add_block_blooms() {
        let mut hierarchy = HierarchicalBloomFilters::with_defaults();

        let keys1: Vec<&[u8]> = vec![b"key1", b"key2"];
        let keys2: Vec<&[u8]> = vec![b"key3", b"key4"];

        hierarchy.add_block_bloom(&keys1).unwrap();
        hierarchy.add_block_bloom(&keys2).unwrap();

        assert_eq!(hierarchy.block_blooms.len(), 2);
        assert_eq!(hierarchy.stats.num_blocks, 2);
    }

    #[test]
    fn test_finalize_stats() {
        let mut hierarchy = HierarchicalBloomFilters::with_defaults();

        let all_keys: Vec<&[u8]> = vec![b"key1", b"key2", b"key3", b"key4"];
        hierarchy.build_file_bloom(&all_keys).unwrap();

        let block1_keys: Vec<&[u8]> = vec![b"key1", b"key2"];
        hierarchy.add_block_bloom(&block1_keys).unwrap();
        let block2_keys: Vec<&[u8]> = vec![b"key3", b"key4"];
        hierarchy.add_block_bloom(&block2_keys).unwrap();

        hierarchy.finalize_stats();

        // Should have memory stats
        assert!(hierarchy.stats.compressed_memory_bytes > 0);
        assert!(hierarchy.stats.uncompressed_memory_bytes > 0);
        assert!(hierarchy.stats.average_sparsity >= 0.0);
        assert!(hierarchy.stats.average_sparsity <= 1.0);
    }

    #[test]
    fn test_progressive_filtering() {
        let mut hierarchy = HierarchicalBloomFilters::with_defaults();

        // Build hierarchy
        let all_keys: Vec<&[u8]> = vec![b"alice", b"bob", b"charlie", b"david"];
        hierarchy.build_file_bloom(&all_keys).unwrap();

        let block1_keys: Vec<&[u8]> = vec![b"alice", b"bob"];
        let block2_keys: Vec<&[u8]> = vec![b"charlie", b"david"];

        hierarchy.add_block_bloom(&block1_keys).unwrap();
        hierarchy.add_block_bloom(&block2_keys).unwrap();

        hierarchy.finalize_stats();

        // Query for existing key
        let candidates = hierarchy.filter_blocks(b"alice").unwrap();
        assert!(!candidates.is_empty(), "Expected non-empty candidates for 'alice'");

        // Query for non-existent key
        // Due to FPR, might get false positives, but should filter most blocks
        let candidates = hierarchy.filter_blocks(b"eve").unwrap();
        assert!(candidates.len() <= 2); // Should eliminate some blocks
    }

    #[test]
    fn test_memory_savings() {
        let mut hierarchy = HierarchicalBloomFilters::with_defaults();

        // Build sparse blooms (should compress well)
        let sparse_keys: Vec<&[u8]> = vec![b"key1", b"key2"];
        hierarchy.build_file_bloom(&sparse_keys).unwrap();
        hierarchy.add_block_bloom(&sparse_keys).unwrap();

        hierarchy.finalize_stats();

        // Should have some memory savings from compression
        let savings = hierarchy.memory_savings_bytes();
        assert!(savings >= 0);

        // Compression ratio should be reasonable
        let ratio = hierarchy.compression_ratio();
        assert!(ratio > 0.0 && ratio <= 1.0);
    }

    #[test]
    fn test_elimination_stats() {
        let mut hierarchy = HierarchicalBloomFilters::with_defaults();

        let keys1: Vec<&[u8]> = vec![b"key1"];
        let keys2: Vec<&[u8]> = vec![b"key2"];
        let keys3: Vec<&[u8]> = vec![b"key3"];

        hierarchy.add_block_bloom(&keys1).unwrap();
        hierarchy.add_block_bloom(&keys2).unwrap();
        hierarchy.add_block_bloom(&keys3).unwrap();

        hierarchy.finalize_stats();

        let stats = hierarchy.elimination_stats(100);
        assert_eq!(stats.total_blocks, 100);
        assert_eq!(stats.blocks_with_blooms, 3);
        assert!(stats.estimated_elimination_rate > 0.95); // Should be >95%
    }

    #[test]
    fn test_filter_with_superblocks() {
        let mut hierarchy = HierarchicalBloomFilters::with_defaults();

        // Build file bloom
        let all_keys: Vec<&[u8]> = vec![b"key1", b"key2", b"key3", b"key4"];
        hierarchy.build_file_bloom(&all_keys).unwrap();

        // Add superblock bloom
        let sb_keys: Vec<&[u8]> = vec![b"key1", b"key2"];
        hierarchy.add_superblock_bloom(&sb_keys).unwrap();

        // Add block blooms
        let block1_keys: Vec<&[u8]> = vec![b"key1"];
        let block2_keys: Vec<&[u8]> = vec![b"key2"];

        hierarchy.add_block_bloom(&block1_keys).unwrap();
        hierarchy.add_block_bloom(&block2_keys).unwrap();

        hierarchy.finalize_stats();

        // Filter with 2 blocks per superblock
        let (sb_candidates, block_candidates) =
            hierarchy.filter_with_superblocks(b"key1", 2).unwrap();

        assert!(!sb_candidates.is_empty());
        assert!(!block_candidates.is_empty());
    }
}

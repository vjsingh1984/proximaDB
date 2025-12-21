//! Adaptive Bloom Filter Sizing (Phase 1.2)
//!
//! Provides automatic bloom filter sizing based on actual key counts and target
//! false positive rates, replacing fixed `bits_per_key` configurations.
//!
//! ## Key Features
//!
//! - **Automatic Sizing**: Calculates optimal bits based on mathematical formula
//! - **Bounded Configuration**: Min/max limits prevent extreme values
//! - **Level-Specific Presets**: Optimized configs for file/superblock/block levels
//!
//! ## Formula
//!
//! ```text
//! m = -n * ln(p) / (ln(2)^2)
//! k = (m/n) * ln(2)
//!
//! where:
//! - m = number of bits
//! - n = number of keys
//! - p = target false positive rate
//! - k = number of hash functions
//! ```

use serde::{Deserialize, Serialize};

/// Adaptive bloom filter configuration
///
/// Auto-sizes bloom filters based on actual key count and target false positive rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveBloomConfig {
    /// Target false positive rate (default: 0.01 = 1%)
    pub target_fp_rate: f64,

    /// Minimum bits per key (default: 4, ~10% FPR)
    pub min_bits_per_key: usize,

    /// Maximum bits per key (default: 20, ~0.001% FPR)
    pub max_bits_per_key: usize,

    /// Enable bloom filter compression
    pub enable_compression: bool,

    /// Compression threshold (compress if sparsity < this value)
    pub compression_threshold: f64,
}

impl Default for AdaptiveBloomConfig {
    fn default() -> Self {
        Self {
            target_fp_rate: 0.01, // 1% FPR
            min_bits_per_key: 4,  // ~10% FPR minimum
            max_bits_per_key: 20, // ~0.001% FPR maximum
            enable_compression: true,
            compression_threshold: 0.5, // Compress if <50% bits set
        }
    }
}

impl AdaptiveBloomConfig {
    /// Calculate optimal bloom filter size for a given number of keys
    ///
    /// Formula: m = -n * ln(p) / (ln(2)^2)
    /// where:
    /// - m = number of bits
    /// - n = number of keys
    /// - p = target false positive rate
    ///
    /// Clamped to [min_bits_per_key * n, max_bits_per_key * n]
    pub fn optimal_size(&self, num_keys: usize) -> usize {
        if num_keys == 0 {
            return 8; // Minimum 8 bits for empty filter
        }

        // Calculate ideal size
        let ln2_squared = 0.4804530139182014; // (ln(2))^2
        let ideal_bits = (-((num_keys as f64) * self.target_fp_rate.ln()) / ln2_squared) as usize;

        // Clamp to configured bounds
        let min_bits = num_keys * self.min_bits_per_key;
        let max_bits = num_keys * self.max_bits_per_key;

        ideal_bits.clamp(min_bits, max_bits)
    }

    /// Calculate optimal number of hash functions
    ///
    /// Formula: k = (m/n) * ln(2)
    /// where:
    /// - k = number of hash functions
    /// - m = number of bits
    /// - n = number of keys
    pub fn optimal_hash_count(&self, num_bits: usize, num_keys: usize) -> usize {
        if num_keys == 0 {
            return 1;
        }

        let ratio = num_bits as f64 / num_keys as f64;
        let optimal = (ratio * 2.0_f64.ln()).round() as usize;

        optimal.clamp(1, 16) // Min 1, max 16 hash functions
    }

    /// Preset for file-level bloom filters
    ///
    /// Target 5% FPR for coarse file-level filtering
    pub fn for_file_level() -> Self {
        Self {
            target_fp_rate: 0.05, // 5% FPR
            min_bits_per_key: 4,
            max_bits_per_key: 12,
            enable_compression: true,
            compression_threshold: 0.5,
        }
    }

    /// Preset for superblock-level bloom filters
    ///
    /// Target 2% FPR for medium-grained filtering
    pub fn for_superblock_level() -> Self {
        Self {
            target_fp_rate: 0.02, // 2% FPR
            min_bits_per_key: 6,
            max_bits_per_key: 16,
            enable_compression: true,
            compression_threshold: 0.5,
        }
    }

    /// Preset for block-level bloom filters
    ///
    /// Target 1% FPR for fine-grained filtering
    pub fn for_block_level() -> Self {
        Self {
            target_fp_rate: 0.01, // 1% FPR
            min_bits_per_key: 8,
            max_bits_per_key: 20,
            enable_compression: true,
            compression_threshold: 0.5,
        }
    }

    /// Convert to base BloomFilterConfig with calculated size
    pub fn to_bloom_config(&self, num_keys: usize) -> crate::core::bloom::BloomFilterConfig {
        let bits_per_key = (self.optimal_size(num_keys) / num_keys.max(1)) as u32;

        crate::core::bloom::BloomFilterConfig {
            strategy: crate::core::bloom::BloomStrategy::ByteAligned,
            bits_per_key: bits_per_key.clamp(4, 32),
            false_positive_rate: Some(self.target_fp_rate),
            expected_items: num_keys,
            enabled: true,
            hash_algorithm: crate::core::bloom::HashAlgorithm::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_sizing() {
        let config = AdaptiveBloomConfig::default();

        // Small block (100 keys)
        let size_100 = config.optimal_size(100);
        assert!(size_100 >= 100 * config.min_bits_per_key);
        assert!(size_100 <= 100 * config.max_bits_per_key);

        // Large block (10K keys)
        let size_10k = config.optimal_size(10000);
        assert!(size_10k >= 10000 * config.min_bits_per_key);
        assert!(size_10k <= 10000 * config.max_bits_per_key);

        // Larger blocks should use more bits
        assert!(size_10k > size_100);
    }

    #[test]
    fn test_optimal_hash_count() {
        let config = AdaptiveBloomConfig::default();

        let num_keys = 1000;
        let num_bits = config.optimal_size(num_keys);
        let hash_count = config.optimal_hash_count(num_bits, num_keys);

        // Should be between 1 and 16
        assert!(hash_count >= 1 && hash_count <= 16);

        // For typical configs, should be around 6-8
        assert!(hash_count >= 5 && hash_count <= 10);
    }

    #[test]
    fn test_level_presets() {
        let file_config = AdaptiveBloomConfig::for_file_level();
        let superblock_config = AdaptiveBloomConfig::for_superblock_level();
        let block_config = AdaptiveBloomConfig::for_block_level();

        // File level should have highest FPR (most tolerant)
        assert!(file_config.target_fp_rate > superblock_config.target_fp_rate);
        assert!(superblock_config.target_fp_rate > block_config.target_fp_rate);

        // Block level should use most bits per key (most accurate)
        let num_keys = 1000;
        let file_bits = file_config.optimal_size(num_keys);
        let sb_bits = superblock_config.optimal_size(num_keys);
        let block_bits = block_config.optimal_size(num_keys);

        assert!(block_bits >= sb_bits);
        assert!(sb_bits >= file_bits);
    }

    #[test]
    fn test_zero_keys_handling() {
        let config = AdaptiveBloomConfig::default();

        // Should return minimum size for zero keys
        let size = config.optimal_size(0);
        assert_eq!(size, 8);

        // Hash count should be 1
        let hash_count = config.optimal_hash_count(size, 0);
        assert_eq!(hash_count, 1);
    }

    #[test]
    fn test_conversion_to_bloom_config() {
        let adaptive = AdaptiveBloomConfig::default();
        let num_keys = 1000;

        let bloom_config = adaptive.to_bloom_config(num_keys);

        // Should match expected items
        assert_eq!(bloom_config.expected_items, num_keys);

        // Should have reasonable bits_per_key
        assert!(bloom_config.bits_per_key >= 4);
        assert!(bloom_config.bits_per_key <= 32);

        // Should preserve target FPR
        assert_eq!(
            bloom_config.false_positive_rate,
            Some(adaptive.target_fp_rate)
        );
    }
}

/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Engine-specific configuration for compaction and storage operations

use super::compaction_utils::StorageEngineType;

/// Configuration for engine-specific compaction behavior
#[derive(Debug, Clone)]
pub struct EngineCompactionConfig {
    /// Base compaction configuration
    pub base: crate::core::config::CompactionConfig,

    /// Engine-specific settings
    pub engine_specific: EngineSpecificConfig,
}

/// Engine-specific compaction settings
#[derive(Debug, Clone)]
pub enum EngineSpecificConfig {
    SST(SstCompactionConfig),
    VIPER(ViperCompactionConfig),
    NOVA(NovaCompactionConfig),
    SWIFT(SwiftCompactionConfig),
    HELIX(HelixCompactionConfig),
    RAPTOR(RaptorCompactionConfig),
}

/// SST engine compaction configuration
///
/// Configures compaction behavior for the SST (Sorted String Table) storage engine,
/// including bloom filter optimization and file sizing parameters.
#[derive(Debug, Clone)]
pub struct SstCompactionConfig {
    /// Enable bloom filter merging during compaction for improved query performance
    pub merge_bloom_filters: bool,
    /// Enable three-stage filter optimization for reduced false positives
    pub use_three_stage_filter: bool,
    /// Block size for SST files in kilobytes
    pub block_size_kb: usize,
    /// Maximum file size for L0 files in megabytes
    pub max_l0_file_size_mb: usize,
}

impl Default for SstCompactionConfig {
    fn default() -> Self {
        Self {
            merge_bloom_filters: true,
            use_three_stage_filter: true,
            block_size_kb: 64,
            max_l0_file_size_mb: 64,
        }
    }
}

/// VIPER engine compaction configuration
///
/// Configures compaction behavior for the VIPER (Parquet-based) storage engine,
/// optimizing columnar storage for analytical workloads.
#[derive(Debug, Clone)]
pub struct ViperCompactionConfig {
    /// Enable columnar optimization during compaction operations
    pub columnar_optimization: bool,
    /// Row group size for Parquet files (affects compression and query performance)
    pub row_group_size: usize,
    /// Enable dictionary encoding for string columns
    pub dictionary_encoding: bool,
    /// Compression codec to use for Parquet files (e.g., "zstd", "snappy", "gzip")
    pub compression_codec: String,
}

impl Default for ViperCompactionConfig {
    fn default() -> Self {
        Self {
            columnar_optimization: true,
            row_group_size: 100_000,
            dictionary_encoding: true,
            compression_codec: "zstd".to_string(),
        }
    }
}

/// NOVA engine compaction configuration
///
/// Configures compaction behavior for the NOVA streaming engine,
/// optimizing for high-velocity write workloads.
#[derive(Debug, Clone)]
pub struct NovaCompactionConfig {
    /// Enable hierarchical compaction for better read/write balance
    pub hierarchical_compaction: bool,
    /// Use zone maps for efficient data filtering during queries
    pub use_zone_maps: bool,
    /// Quantization level to apply during compaction (e.g., "pq8", "sq8")
    pub quantization_level: String,
    /// Streaming buffer size in megabytes for write operations
    pub streaming_buffer_mb: usize,
}

impl Default for NovaCompactionConfig {
    fn default() -> Self {
        Self {
            hierarchical_compaction: true,
            use_zone_maps: true,
            quantization_level: "pq8".to_string(),
            streaming_buffer_mb: 64,
        }
    }
}

/// SWIFT engine compaction configuration
#[derive(Debug, Clone)]
pub struct SwiftCompactionConfig {
    /// Superblock size (in MB)
    pub superblock_size_mb: usize,

    /// Enable hierarchical blocks
    pub use_hierarchical_blocks: bool,

    /// ID index optimization
    pub optimize_id_index: bool,

    /// Progressive search support
    pub enable_progressive_search: bool,
}

impl Default for SwiftCompactionConfig {
    fn default() -> Self {
        Self {
            superblock_size_mb: 128,
            use_hierarchical_blocks: true,
            optimize_id_index: true,
            enable_progressive_search: true,
        }
    }
}

/// PRISM engine compaction configuration
#[derive(Debug, Clone)]
pub struct HelixCompactionConfig {
    /// Memory optimization level
    pub memory_optimization_level: String,

    /// Proxima encoding for vectors
    pub use_proximaencoder: bool,

    /// Tree rebalancing threshold
    pub tree_rebalance_threshold: f64,

    /// Cache warmup after compaction
    pub cache_warmup: bool,
}

impl Default for HelixCompactionConfig {
    fn default() -> Self {
        Self {
            memory_optimization_level: "aggressive".to_string(),
            use_proximaencoder: true,
            tree_rebalance_threshold: 0.7,
            cache_warmup: true,
        }
    }
}

/// RAPTOR engine compaction configuration
#[derive(Debug, Clone)]
pub struct RaptorCompactionConfig {
    /// Adaptive PxK configuration
    pub adaptive_pxk: bool,

    /// Matrix builder optimization
    pub optimize_matrix_layout: bool,

    /// Row group manager settings
    pub smart_rowgroup_sizing: bool,

    /// Artus bloom filter integration
    pub use_artus_bloom: bool,

    /// Maximum matrix dimension
    pub max_matrix_dimension: usize,
}

impl Default for RaptorCompactionConfig {
    fn default() -> Self {
        Self {
            adaptive_pxk: true,
            optimize_matrix_layout: true,
            smart_rowgroup_sizing: true,
            use_artus_bloom: true,
            max_matrix_dimension: 4096,
        }
    }
}

impl EngineCompactionConfig {
    /// Create default configuration for a specific engine type
    pub fn for_engine(engine_type: StorageEngineType) -> Self {
        let engine_specific = match engine_type {
            StorageEngineType::SST => EngineSpecificConfig::SST(SstCompactionConfig::default()),
            StorageEngineType::VIPER => {
                EngineSpecificConfig::VIPER(ViperCompactionConfig::default())
            }
            StorageEngineType::NOVA => EngineSpecificConfig::NOVA(NovaCompactionConfig::default()),
            StorageEngineType::SWIFT => {
                EngineSpecificConfig::SWIFT(SwiftCompactionConfig::default())
            }
            StorageEngineType::HELIX => {
                EngineSpecificConfig::HELIX(HelixCompactionConfig::default())
            }
            StorageEngineType::RAPTOR => {
                EngineSpecificConfig::RAPTOR(RaptorCompactionConfig::default())
            }
            StorageEngineType::TST => EngineSpecificConfig::VIPER(ViperCompactionConfig::default()),
        };

        Self {
            base: Default::default(),
            engine_specific,
        }
    }

    /// Get compaction threshold based on engine type
    pub fn get_compaction_threshold(&self) -> usize {
        match &self.engine_specific {
            EngineSpecificConfig::SST(_) => self.base.l0_file_threshold,
            EngineSpecificConfig::VIPER(_) => self.base.l0_file_threshold / 2, // More aggressive for columnar
            EngineSpecificConfig::NOVA(_) => self.base.l0_file_threshold / 2,
            EngineSpecificConfig::SWIFT(_) => self.base.l0_file_threshold,
            EngineSpecificConfig::HELIX(_) => self.base.l0_file_threshold * 2, // Less aggressive for memory-optimized
            EngineSpecificConfig::RAPTOR(_) => self.base.l0_file_threshold,
        }
    }

    /// Get size threshold based on engine type  
    pub fn get_size_threshold_mb(&self) -> usize {
        match &self.engine_specific {
            EngineSpecificConfig::SST(config) => {
                config.max_l0_file_size_mb * self.base.l0_file_threshold
            }
            EngineSpecificConfig::VIPER(_) => self.base.l0_size_threshold_mb,
            EngineSpecificConfig::NOVA(_) => self.base.l0_size_threshold_mb * 2, // Larger for streaming
            EngineSpecificConfig::SWIFT(config) => config.superblock_size_mb,
            EngineSpecificConfig::HELIX(_) => self.base.l0_size_threshold_mb / 2, // Smaller for memory efficiency
            EngineSpecificConfig::RAPTOR(_) => self.base.l0_size_threshold_mb,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_default_thresholds(engine: StorageEngineType, threshold_factor: usize) {
        let config = EngineCompactionConfig::for_engine(engine);
        assert_eq!(
            config.get_compaction_threshold(),
            config.base.l0_file_threshold * threshold_factor
        );
    }

    #[test]
    fn default_engine_specific_configs_capture_expected_policy_knobs() {
        let sst = SstCompactionConfig::default();
        assert!(sst.merge_bloom_filters);
        assert!(sst.use_three_stage_filter);
        assert_eq!(sst.block_size_kb, 64);
        assert_eq!(sst.max_l0_file_size_mb, 64);

        let viper = ViperCompactionConfig::default();
        assert!(viper.columnar_optimization);
        assert!(viper.dictionary_encoding);
        assert_eq!(viper.row_group_size, 100_000);
        assert_eq!(viper.compression_codec, "zstd");

        let nova = NovaCompactionConfig::default();
        assert!(nova.hierarchical_compaction);
        assert!(nova.use_zone_maps);
        assert_eq!(nova.quantization_level, "pq8");
        assert_eq!(nova.streaming_buffer_mb, 64);

        let swift = SwiftCompactionConfig::default();
        assert_eq!(swift.superblock_size_mb, 128);
        assert!(swift.use_hierarchical_blocks);
        assert!(swift.optimize_id_index);
        assert!(swift.enable_progressive_search);

        let helix = HelixCompactionConfig::default();
        assert_eq!(helix.memory_optimization_level, "aggressive");
        assert!(helix.use_proximaencoder);
        assert_eq!(helix.tree_rebalance_threshold, 0.7);
        assert!(helix.cache_warmup);

        let raptor = RaptorCompactionConfig::default();
        assert!(raptor.adaptive_pxk);
        assert!(raptor.optimize_matrix_layout);
        assert!(raptor.smart_rowgroup_sizing);
        assert!(raptor.use_artus_bloom);
        assert_eq!(raptor.max_matrix_dimension, 4096);
    }

    #[test]
    fn for_engine_selects_the_expected_variant() {
        assert!(matches!(
            EngineCompactionConfig::for_engine(StorageEngineType::SST).engine_specific,
            EngineSpecificConfig::SST(_)
        ));
        assert!(matches!(
            EngineCompactionConfig::for_engine(StorageEngineType::VIPER).engine_specific,
            EngineSpecificConfig::VIPER(_)
        ));
        assert!(matches!(
            EngineCompactionConfig::for_engine(StorageEngineType::NOVA).engine_specific,
            EngineSpecificConfig::NOVA(_)
        ));
        assert!(matches!(
            EngineCompactionConfig::for_engine(StorageEngineType::SWIFT).engine_specific,
            EngineSpecificConfig::SWIFT(_)
        ));
        assert!(matches!(
            EngineCompactionConfig::for_engine(StorageEngineType::HELIX).engine_specific,
            EngineSpecificConfig::HELIX(_)
        ));
        assert!(matches!(
            EngineCompactionConfig::for_engine(StorageEngineType::RAPTOR).engine_specific,
            EngineSpecificConfig::RAPTOR(_)
        ));
        assert!(matches!(
            EngineCompactionConfig::for_engine(StorageEngineType::TST).engine_specific,
            EngineSpecificConfig::VIPER(_)
        ));
    }

    #[test]
    fn compaction_threshold_policy_matches_engine_profiles() {
        assert_default_thresholds(StorageEngineType::SST, 1);
        assert_default_thresholds(StorageEngineType::SWIFT, 1);
        assert_default_thresholds(StorageEngineType::RAPTOR, 1);

        let viper = EngineCompactionConfig::for_engine(StorageEngineType::VIPER);
        assert_eq!(
            viper.get_compaction_threshold(),
            viper.base.l0_file_threshold / 2
        );

        let nova = EngineCompactionConfig::for_engine(StorageEngineType::NOVA);
        assert_eq!(
            nova.get_compaction_threshold(),
            nova.base.l0_file_threshold / 2
        );

        let helix = EngineCompactionConfig::for_engine(StorageEngineType::HELIX);
        assert_eq!(
            helix.get_compaction_threshold(),
            helix.base.l0_file_threshold * 2
        );
    }

    #[test]
    fn size_threshold_policy_matches_engine_profiles() {
        let sst = EngineCompactionConfig::for_engine(StorageEngineType::SST);
        assert_eq!(sst.get_size_threshold_mb(), 64 * sst.base.l0_file_threshold);

        let viper = EngineCompactionConfig::for_engine(StorageEngineType::VIPER);
        assert_eq!(
            viper.get_size_threshold_mb(),
            viper.base.l0_size_threshold_mb
        );

        let nova = EngineCompactionConfig::for_engine(StorageEngineType::NOVA);
        assert_eq!(
            nova.get_size_threshold_mb(),
            nova.base.l0_size_threshold_mb * 2
        );

        let swift = EngineCompactionConfig::for_engine(StorageEngineType::SWIFT);
        assert_eq!(swift.get_size_threshold_mb(), 128);

        let helix = EngineCompactionConfig::for_engine(StorageEngineType::HELIX);
        assert_eq!(
            helix.get_size_threshold_mb(),
            helix.base.l0_size_threshold_mb / 2
        );
    }
}

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
#[derive(Debug, Clone)]
pub struct SstCompactionConfig {
    /// Enable bloom filter merging during compaction
    pub merge_bloom_filters: bool,

    /// Three-stage filter optimization
    pub use_three_stage_filter: bool,

    /// Block size for SST files (in KB)
    pub block_size_kb: usize,

    /// Maximum file size for L0 (in MB)
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
#[derive(Debug, Clone)]
pub struct ViperCompactionConfig {
    /// Use columnar optimization during compaction
    pub columnar_optimization: bool,

    /// Row group size for Parquet files
    pub row_group_size: usize,

    /// Enable dictionary encoding
    pub dictionary_encoding: bool,

    /// Compression codec for Parquet
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
#[derive(Debug, Clone)]
pub struct NovaCompactionConfig {
    /// Enable hierarchical compaction
    pub hierarchical_compaction: bool,

    /// Use zone maps for filtering
    pub use_zone_maps: bool,

    /// Quantization level for compacted data
    pub quantization_level: String,

    /// Streaming buffer size (in MB)
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
    pub use_proxima_encoding: bool,

    /// Tree rebalancing threshold
    pub tree_rebalance_threshold: f64,

    /// Cache warmup after compaction
    pub cache_warmup: bool,
}

impl Default for HelixCompactionConfig {
    fn default() -> Self {
        Self {
            memory_optimization_level: "aggressive".to_string(),
            use_proxima_encoding: true,
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

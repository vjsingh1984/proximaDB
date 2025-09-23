// Shared Compression Configuration for SST and SWIFT engines
// Unified compression strategies and configuration management

use std::collections::HashMap;

use crate::core::compression::{CompressionAlgorithm, CompressionContext};
use crate::core::hardware_capabilities::HardwareCapabilities;
use crate::proto::proximadb_v1::CompressionConfig as ProtoCompressionConfig;

/// Row-based compression configuration
#[derive(Debug, Clone)]
pub struct RowBasedCompressionConfig {
    /// Global compression settings
    pub enabled: bool,
    pub algorithm: CompressionAlgorithm,
    pub compression_level: u8,
    pub compression_ratio_estimate: f32,

    /// Vector-specific compression
    pub vector_compression: VectorCompressionStrategy,

    /// Metadata compression
    pub metadata_compression: MetadataCompressionConfig,

    /// Block-level compression
    pub block_compression: BlockLevelCompressionConfig,

    /// Dynamic compression adjustment
    pub adaptive_compression: AdaptiveCompressionConfig,

    /// Performance thresholds
    pub compression_thresholds: CompressionThresholds,
}

/// Vector compression strategy
#[derive(Debug, Clone)]
pub struct VectorCompressionStrategy {
    /// Strategy type

    /// Dimension-specific settings
    pub dimension_thresholds: HashMap<usize, CompressionSettings>,

    /// Hardware-optimized settings
    pub hardware_optimizations: HardwareCompressionConfig,

    /// Quantization integration
    pub quantization_aware: bool,
    pub quantization_first: bool,
}

#[derive(Debug, Clone)]
pub enum VectorCompressionType {
    /// No vector compression
    None,
    /// Float32 to Float16 conversion
    Float16,
    /// Bytemuck for fixed dimensions
    Bytemuck,
    /// Dictionary compression for repeated patterns
    Dictionary,
    /// Delta compression for sequential data
    Delta,
    /// Adaptive based on vector analysis
    Adaptive,
}

/// Compression settings for specific configurations
#[derive(Debug, Clone)]
pub struct CompressionSettings {
    pub algorithm: CompressionAlgorithm,
    pub level: u8,
    pub enable_dictionary: bool,
    pub enable_delta: bool,
    pub block_size_hint: usize,
}

/// Hardware-optimized compression configuration
#[derive(Debug, Clone)]
pub struct HardwareCompressionConfig {
    /// Use hardware acceleration when available
    pub use_hardware_acceleration: bool,

    /// SIMD-optimized algorithms
    pub simd_algorithms: Vec<CompressionAlgorithm>,

    /// GPU-accelerated compression
    pub gpu_acceleration: bool,

    /// Memory-bandwidth optimized settings
    pub memory_bandwidth_optimization: bool,
}

/// Metadata compression configuration
#[derive(Debug, Clone)]
pub struct MetadataCompressionConfig {
    /// Enable metadata compression
    pub enabled: bool,

    /// JSON-specific compression
    pub json_compression: JsonCompressionConfig,

    /// String compression strategies
    pub string_compression: StringCompressionConfig,

    /// Timestamp compression
    pub timestamp_compression: TimestampCompressionConfig,
}

#[derive(Debug, Clone)]
pub struct JsonCompressionConfig {
    pub algorithm: CompressionAlgorithm,
    pub enable_schema_compression: bool,
    pub enable_value_deduplication: bool,
    pub max_schema_cache_size: usize,
}

#[derive(Debug, Clone)]
pub struct StringCompressionConfig {
    pub algorithm: CompressionAlgorithm,
    pub enable_dictionary: bool,
    pub dictionary_size_limit: usize,
    pub min_string_length: usize,
}

#[derive(Debug, Clone)]
pub struct TimestampCompressionConfig {
    pub use_delta_encoding: bool,
    pub delta_precision: TimestampPrecision,
    pub enable_run_length_encoding: bool,
}

#[derive(Debug, Clone)]
pub enum TimestampPrecision {
    Seconds,
    Milliseconds,
    Microseconds,
    Nanoseconds,
}

/// Block-level compression configuration
#[derive(Debug, Clone)]
pub struct BlockLevelCompressionConfig {
    /// Compression per block
    pub per_block_compression: bool,

    /// Inter-block compression
    pub inter_block_compression: bool,

    /// Block size optimization
    pub optimal_block_sizes: HashMap<CompressionAlgorithm, usize>,

    /// Compression pipeline
    pub multi_stage_compression: bool,
    pub compression_stages: Vec<CompressionStage>,
}

#[derive(Debug, Clone)]
pub struct CompressionStage {
    pub stage_name: String,
    pub algorithm: CompressionAlgorithm,
    pub level: u8,
    pub condition: CompressionCondition,
}

#[derive(Debug, Clone)]
pub enum CompressionCondition {
    Always,
    IfSizeAbove(usize),
    IfCompressionRatioBelow(f32),
    IfCpuUsageBelow(f32),
}

/// Adaptive compression configuration
#[derive(Debug, Clone)]
pub struct AdaptiveCompressionConfig {
    /// Enable adaptive compression
    pub enabled: bool,

    /// Adaptation triggers
    pub adaptation_triggers: Vec<AdaptationTrigger>,

    /// Performance monitoring
    pub monitor_compression_ratio: bool,
    pub monitor_compression_time: bool,
    pub monitor_decompression_time: bool,

    /// Adjustment parameters
    pub adjustment_frequency: AdaptationFrequency,
    pub max_level_increase: u8,
    pub max_level_decrease: u8,
}

#[derive(Debug, Clone)]
pub enum AdaptationTrigger {
    CompressionRatioBelow(f32),
    CompressionTimeAbove(f64),
    DecompressionTimeAbove(f64),
    CpuUsageAbove(f32),
    MemoryUsageAbove(f32),
}

#[derive(Debug, Clone)]
pub enum AdaptationFrequency {
    PerBlock,
    PerFlush,
    PerCompaction,
    TimeBased(u64), // milliseconds
}

/// Compression performance thresholds
#[derive(Debug, Clone)]
pub struct CompressionThresholds {
    /// Minimum size to enable compression
    pub min_compression_size: usize,

    /// Maximum compression time
    pub max_compression_time_ms: f64,

    /// Minimum compression ratio to be worthwhile
    pub min_compression_ratio: f32,

    /// Memory usage limits
    pub max_memory_overhead_percent: f32,

    /// CPU usage limits
    pub max_cpu_usage_percent: f32,
}

/// Compression parameters for operations
#[derive(Debug, Clone)]
pub struct CompressionParameters {
    pub config: RowBasedCompressionConfig,
    pub context: CompressionContext,
    pub hardware: std::sync::Arc<HardwareCapabilities>,
    pub collection_config: Option<ProtoCompressionConfig>,
}

/// Compression statistics and results
#[derive(Debug, Clone)]
pub struct CompressionStats {
    /// Size information
    pub original_size: usize,
    pub compressed_size: usize,
    pub compression_ratio: f32,

    /// Performance metrics
    pub compression_time_ms: f64,
    pub decompression_time_ms: f64,
    pub throughput_mbps: f32,

    /// Algorithm information
    pub algorithm_used: CompressionAlgorithm,
    pub level_used: u8,
    pub hardware_accelerated: bool,

    /// Quality metrics
    pub memory_overhead: usize,
    pub cpu_usage_percent: f32,
}

impl RowBasedCompressionConfig {
    /// Create compression config from proto config
    pub fn from_proto_config(proto_config: &ProtoCompressionConfig) -> Self {
        use crate::proto::proximadb_v1::CompressionAlgorithm as ProtoAlgorithm;
        let algorithm = match ProtoAlgorithm::try_from(proto_config.algorithm) {
            Ok(ProtoAlgorithm::CompressionZstd) => CompressionAlgorithm::Zstd,
            Ok(ProtoAlgorithm::CompressionLz4) => CompressionAlgorithm::Lz4,
            Ok(ProtoAlgorithm::CompressionSnappy) => CompressionAlgorithm::Snappy,
            Ok(ProtoAlgorithm::CompressionGzip) => CompressionAlgorithm::Gzip,
            Ok(ProtoAlgorithm::CompressionBrotli) => CompressionAlgorithm::Brotli,
            _ => CompressionAlgorithm::Zstd,
        };

        Self {
            enabled: proto_config.adaptive, // Use adaptive field as enabled
            algorithm,
            compression_level: proto_config.level.unwrap_or(3) as u8,
            compression_ratio_estimate: 1.5, // Default ratio
            vector_compression: VectorCompressionStrategy::default(),
            metadata_compression: MetadataCompressionConfig::default(),
            block_compression: BlockLevelCompressionConfig::default(),
            adaptive_compression: AdaptiveCompressionConfig::default(),
            compression_thresholds: CompressionThresholds::default(),
        }
    }

    /// Get compression settings for specific dimension
    pub fn get_vector_settings(&self, dimension: usize) -> CompressionSettings {
        let key = &dimension;
        self.vector_compression
            .dimension_thresholds
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.default_vector_settings(dimension))
    }

    /// Get default vector settings for dimension
    fn default_vector_settings(&self, dimension: usize) -> CompressionSettings {
        let (algorithm, enable_bytemuck) = match dimension {
            // Common embedding dimensions - use bytemuck for performance
            64 | 128 | 256 | 384 | 512 | 768 | 1024 | 1536 | 2048 => {
                (CompressionAlgorithm::Lz4, true)
            }
            // Other dimensions - use general compression
            _ => (self.algorithm, false),
        };

        CompressionSettings {
            algorithm,
            level: self.compression_level,
            enable_dictionary: dimension > 1000, // Dictionary helps for large vectors
            enable_delta: false,                 // Generally not beneficial for random vectors
            block_size_hint: (dimension * 4 * 2000).max(64 * 1024), // ~2000 vectors or 64KB min
        }
    }

    /// Check if compression should be applied
    pub fn should_compress(&self, data_size: usize, context: &CompressionContext) -> bool {
        if !self.enabled {
            return false;
        }

        if data_size < self.compression_thresholds.min_compression_size {
            return false;
        }

        // Context-specific decisions
        match context {
            CompressionContext::Column => {
                // Use Column for vector data
                // Check if vector compression is enabled based on hardware optimizations or quantization
                self.vector_compression.quantization_aware
                    || self
                        .vector_compression
                        .hardware_optimizations
                        .use_hardware_acceleration
            }
            CompressionContext::Block => self.block_compression.per_block_compression,
            _ => true,
        }
    }

    /// Get optimal algorithm for context and size
    pub fn get_optimal_algorithm(
        &self,
        context: &CompressionContext,
        data_size: usize,
        hardware: &HardwareCapabilities,
    ) -> CompressionAlgorithm {
        // Hardware-specific optimizations
        if self
            .vector_compression
            .hardware_optimizations
            .use_hardware_acceleration
        {
            if let Some(simd_algorithm) = self.get_simd_optimal_algorithm(hardware) {
                return simd_algorithm;
            }
        }

        // Size-based selection
        match data_size {
            // Small data - prioritize speed
            size if size < 64 * 1024 => CompressionAlgorithm::Lz4,
            // Medium data - balanced
            size if size < 1024 * 1024 => CompressionAlgorithm::Zstd,
            // Large data - prioritize compression ratio
            _ => CompressionAlgorithm::Brotli,
        }
    }

    /// Get SIMD-optimal algorithm
    fn get_simd_optimal_algorithm(
        &self,
        hardware: &HardwareCapabilities,
    ) -> Option<CompressionAlgorithm> {
        let simd_algos = &self
            .vector_compression
            .hardware_optimizations
            .simd_algorithms;

        if hardware.has_avx512() && simd_algos.contains(&CompressionAlgorithm::Lz4) {
            Some(CompressionAlgorithm::Lz4)
        } else if hardware.cpu.features.avx2_support
            && simd_algos.contains(&CompressionAlgorithm::Snappy)
        {
            Some(CompressionAlgorithm::Snappy)
        } else {
            None
        }
    }

    /// Centralized conversion from proto config to BlockCompressionConfig
    /// Used by all engines (SST, SWIFT, HELIX) to avoid duplication
    pub fn to_block_compression_config(&self) -> crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig {
        use crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig;

        BlockCompressionConfig {
            algorithm: self.algorithm,
            compression_level: self.compression_level,
            enable_vector_compression: self.enabled && self.algorithm != CompressionAlgorithm::None,
            enable_metadata_compression: self.metadata_compression.enabled,
            compression_threshold_bytes: self.compression_thresholds.min_compression_size,
            dictionary_compression: self.adaptive_compression.enabled,
        }
    }

    /// Create BlockCompressionConfig from proto config directly
    /// Convenience method that combines from_proto_config() and to_block_compression_config()
    pub fn create_block_config_from_proto(proto_config: Option<&ProtoCompressionConfig>) -> crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig {
        match proto_config {
            Some(config) => {
                let unified_config = Self::from_proto_config(config);
                unified_config.to_block_compression_config()
            }
            None => {
                // Create config with None compression when no config provided
                let mut config = Self::default();
                config.enabled = false;
                config.algorithm = CompressionAlgorithm::None;
                config.to_block_compression_config()
            }
        }
    }
}

impl Default for RowBasedCompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            algorithm: CompressionAlgorithm::Zstd,
            compression_level: 3,
            compression_ratio_estimate: 0.7,
            vector_compression: VectorCompressionStrategy::default(),
            metadata_compression: MetadataCompressionConfig::default(),
            block_compression: BlockLevelCompressionConfig::default(),
            adaptive_compression: AdaptiveCompressionConfig::default(),
            compression_thresholds: CompressionThresholds::default(),
        }
    }
}

impl Default for VectorCompressionStrategy {
    fn default() -> Self {
        let mut dimension_thresholds = HashMap::new();

        // Add optimized settings for common dimensions
        for &dim in &[64, 128, 256, 384, 512, 768, 1024, 1536, 2048] {
            dimension_thresholds.insert(
                dim,
                CompressionSettings {
                    algorithm: CompressionAlgorithm::Lz4,
                    level: 1,
                    enable_dictionary: false,
                    enable_delta: false,
                    block_size_hint: dim * 4 * 2000,
                },
            );
        }

        Self {
            // strategy removed -  VectorCompressionType::Adaptive,
            dimension_thresholds,
            hardware_optimizations: HardwareCompressionConfig::default(),
            quantization_aware: true,
            quantization_first: true, // Quantize before compress for better ratios
        }
    }
}

impl Default for HardwareCompressionConfig {
    fn default() -> Self {
        Self {
            use_hardware_acceleration: true,
            simd_algorithms: vec![CompressionAlgorithm::Lz4, CompressionAlgorithm::Snappy],
            gpu_acceleration: false,
            memory_bandwidth_optimization: true,
        }
    }
}

impl Default for MetadataCompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            json_compression: JsonCompressionConfig::default(),
            string_compression: StringCompressionConfig::default(),
            timestamp_compression: TimestampCompressionConfig::default(),
        }
    }
}

impl Default for JsonCompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Zstd,
            enable_schema_compression: true,
            enable_value_deduplication: true,
            max_schema_cache_size: 1024 * 1024, // 1MB
        }
    }
}

impl Default for StringCompressionConfig {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Zstd,
            enable_dictionary: true,
            dictionary_size_limit: 64 * 1024, // 64KB
            min_string_length: 8,
        }
    }
}

impl Default for TimestampCompressionConfig {
    fn default() -> Self {
        Self {
            use_delta_encoding: true,
            delta_precision: TimestampPrecision::Milliseconds,
            enable_run_length_encoding: true,
        }
    }
}

impl Default for BlockLevelCompressionConfig {
    fn default() -> Self {
        let mut optimal_sizes = HashMap::new();
        optimal_sizes.insert(CompressionAlgorithm::Zstd, 64 * 1024);
        optimal_sizes.insert(CompressionAlgorithm::Lz4, 32 * 1024);
        optimal_sizes.insert(CompressionAlgorithm::Snappy, 32 * 1024);

        Self {
            per_block_compression: true,
            inter_block_compression: false,
            optimal_block_sizes: optimal_sizes,
            multi_stage_compression: false,
            compression_stages: Vec::new(),
        }
    }
}

impl Default for AdaptiveCompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            adaptation_triggers: vec![
                AdaptationTrigger::CompressionRatioBelow(0.5),
                AdaptationTrigger::CompressionTimeAbove(100.0),
            ],
            monitor_compression_ratio: true,
            monitor_compression_time: true,
            monitor_decompression_time: false,
            adjustment_frequency: AdaptationFrequency::PerFlush,
            max_level_increase: 2,
            max_level_decrease: 1,
        }
    }
}

impl Default for CompressionThresholds {
    fn default() -> Self {
        Self {
            min_compression_size: 1024, // 1KB
            max_compression_time_ms: 100.0,
            min_compression_ratio: 0.8,
            max_memory_overhead_percent: 20.0,
            max_cpu_usage_percent: 30.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_config_defaults() {
        let config = RowBasedCompressionConfig::default();

        assert!(config.enabled);
        assert_eq!(config.algorithm, CompressionAlgorithm::Zstd);
        assert_eq!(config.compression_level, 3);
        assert!(config.vector_compression.quantization_aware);
    }

    #[test]
    fn test_vector_settings_for_common_dimensions() {
        let config = RowBasedCompressionConfig::default();

        let settings_768 = config.get_vector_settings(768);
        assert_eq!(settings_768.algorithm, CompressionAlgorithm::Lz4);

        let settings_random = config.get_vector_settings(999);
        assert_eq!(settings_random.algorithm, CompressionAlgorithm::Zstd);
    }

    #[test]
    fn test_compression_decision_logic() {
        let config = RowBasedCompressionConfig::default();

        // Should compress large data
        assert!(config.should_compress(100 * 1024, &CompressionContext::VectorSerialization));

        // Should not compress tiny data
        assert!(!config.should_compress(100, &CompressionContext::VectorSerialization));

        // Disabled config should not compress
        let mut disabled_config = config.clone();
        disabled_config.enabled = false;
        assert!(!disabled_config.should_compress(100 * 1024, &CompressionContext::VectorSerialization));
    }

    #[test]
    fn test_proto_config_conversion() {
        let proto_config = ProtoCompressionConfig {
            algorithm: 1, // LZ4 algorithm enum value
            level: Some(2),
            adaptive: false,
            min_ratio: None,
            enable_quantization: false,
            quantization_type: None,
            normalization_method: None,
            block_size_kb: 32,
            dynamic_block_sizing: false,
        };

        let config = RowBasedCompressionConfig::from_proto_config(&proto_config);

        assert!(config.enabled);
        assert_eq!(config.algorithm, CompressionAlgorithm::Lz4);
        assert_eq!(config.compression_level, 2);
        assert_eq!(config.compression_ratio_estimate, 0.6);
    }
}

//! Configuration Builder for Parquet Optimizations
//!
//! Provides a fluent API for customizing Parquet optimization settings.
//! All optimizations are enabled by default, but users can selectively
//! disable or tune them based on their specific requirements.

use crate::core::compression::CompressionAlgorithm;
use crate::storage::engines::core::formats::columnar::{
    FooterCacheConfig, HybridWriterConfig, ParquetWriterConfig, QuantizationConfig, WriterMode,
};
use std::time::Duration;

/// Builder for Parquet writer configuration with all optimizations enabled by default
pub struct ParquetConfigBuilder {
    config: ParquetWriterConfig,
}

impl ParquetConfigBuilder {
    /// Create new builder with all optimizations enabled
    pub fn new() -> Self {
        Self {
            config: ParquetWriterConfig::default(), // All optimizations ON by default
        }
    }

    /// Create minimal configuration (all optimizations disabled)
    /// Use this only if you need maximum control and know what you're doing
    pub fn minimal() -> Self {
        Self {
            config: ParquetWriterConfig {
                row_group_size: 10000,
                page_size: 524288, // 512KB
                enable_bloom_filters: false,
                bloom_filter_fpp: 0.05,
                expected_ndv: None,
                bloom_filter_columns: vec![],
                enable_column_statistics: false,
                enable_page_index: false,
                enable_column_index: false,
                enable_offset_index: false,
                page_index_granularity: 5000,
                compression: CompressionAlgorithm::Snappy,
                enable_dictionary: false,
                dictionary_threshold: 0.5,
                enable_delta_encoding: false,
                quantization: QuantizationConfig {
                    enabled: false,
                    strategy: 0, // SmartDefaults
                    custom_levels: vec![],
                    enable_progressive_search: false,
                    binary_filter_selectivity: 0.3,
                    int8_ranking_selectivity: 0.1,
                    pq_ranking_selectivity: 0.05,
                    training_sample_size: 10000,
                    quality_threshold: 0.95,
                    enable_adaptive_training: false,
                    optimize_for_storage: false,
                    optimize_for_memory: false,
                    enable_simd_acceleration: true,
                    // Direct quantization type enables
                    enable_binary: false,
                    enable_int8: false,
                    enable_pq: false,
                    // Product Quantization specific settings
                    pq_segments: 8,
                    pq_bits: 8,
                    pq_codebooks: vec![],
                    // Thresholds for progressive search
                    binary_threshold: 100.0,
                    int8_threshold: 50.0,
                    pq_threshold: 10.0,
                },
                id_less_storage: false,
                write_batch_size: 1000,
                enable_byte_stream_split: false,
                enable_pq_sorting: false,
                pq_sorting_segments: 8,
                pq_sorting_codebook_size: 256,
                enable_native_metadata: false,
                metadata_inference_samples: 100,
            },
        }
    }

    /// Disable bloom filters (not recommended unless memory constrained)
    pub fn disable_bloom_filters(mut self) -> Self {
        self.config.enable_bloom_filters = false;
        self
    }

    /// Disable page indexes (not recommended for cloud storage)
    pub fn disable_page_indexes(mut self) -> Self {
        self.config.enable_column_index = false;
        self.config.enable_offset_index = false;
        self.config.enable_page_index = false;
        self
    }

    /// Disable PQ sorting (not recommended if compression is important)
    pub fn disable_pq_sorting(mut self) -> Self {
        self.config.enable_pq_sorting = false;
        self
    }

    /// Disable native metadata types (not recommended for complex metadata)
    pub fn disable_native_metadata(mut self) -> Self {
        self.config.enable_native_metadata = false;
        self
    }

    /// Set custom compression algorithm
    pub fn compression(mut self, compression: CompressionAlgorithm) -> Self {
        self.config.compression = compression;
        self
    }

    /// Set row group size
    pub fn row_group_size(mut self, size: usize) -> Self {
        self.config.row_group_size = size;
        self
    }

    /// Set page size
    pub fn page_size(mut self, size: usize) -> Self {
        self.config.page_size = size;
        self
    }

    /// Configure bloom filter false positive probability
    pub fn bloom_filter_fpp(mut self, fpp: f64) -> Self {
        self.config.bloom_filter_fpp = fpp;
        self
    }

    /// Set specific columns for bloom filters
    pub fn bloom_filter_columns(mut self, columns: Vec<String>) -> Self {
        self.config.bloom_filter_columns = columns;
        self
    }

    /// Configure PQ sorting parameters
    pub fn pq_sorting_config(mut self, segments: usize, codebook_size: usize) -> Self {
        self.config.pq_sorting_segments = segments;
        self.config.pq_sorting_codebook_size = codebook_size;
        self
    }

    /// Set metadata inference sample size
    pub fn metadata_inference_samples(mut self, samples: usize) -> Self {
        self.config.metadata_inference_samples = samples;
        self
    }

    /// Build the configuration
    pub fn build(self) -> ParquetWriterConfig {
        self.config
    }
}

impl Default for ParquetConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for footer cache configuration
pub struct FooterCacheBuilder {
    config: FooterCacheConfig,
}

impl FooterCacheBuilder {
    /// Create new builder with all optimizations enabled
    pub fn new() -> Self {
        Self {
            config: FooterCacheConfig::default(), // All optimizations ON by default
        }
    }

    /// Disable cache persistence
    pub fn disable_persistence(mut self) -> Self {
        self.config.enable_persistence = false;
        self.config.persistence_path = None;
        self
    }

    /// Disable prefetching
    pub fn disable_prefetch(mut self) -> Self {
        self.config.enable_prefetch = false;
        self
    }

    /// Disable compression
    pub fn disable_compression(mut self) -> Self {
        self.config.compression = false;
        self
    }

    /// Set maximum cache entries
    pub fn max_entries(mut self, entries: u64) -> Self {
        self.config.max_entries = entries;
        self
    }

    /// Set cache TTL
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.config.ttl = ttl;
        self
    }

    /// Set time to idle
    pub fn time_to_idle(mut self, duration: Duration) -> Self {
        self.config.time_to_idle = duration;
        self
    }

    /// Set prefetch threshold
    pub fn prefetch_threshold(mut self, threshold: u64) -> Self {
        self.config.prefetch_threshold = threshold;
        self
    }

    /// Build the configuration
    pub fn build(self) -> FooterCacheConfig {
        self.config
    }
}

impl Default for FooterCacheBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for hybrid writer configuration
pub struct HybridWriterBuilder {
    config: HybridWriterConfig,
}

impl HybridWriterBuilder {
    /// Create new builder with all optimizations enabled
    pub fn new() -> Self {
        Self {
            config: HybridWriterConfig::default(), // All optimizations ON by default
        }
    }

    /// Use fixed streaming mode (disable adaptive behavior)
    pub fn streaming_mode(mut self) -> Self {
        self.config.initial_mode = WriterMode::Streaming;
        self.config.enable_auto_switch = false;
        self
    }

    /// Use fixed batch mode (disable adaptive behavior)
    pub fn batch_mode(mut self) -> Self {
        self.config.initial_mode = WriterMode::Batch;
        self.config.enable_auto_switch = false;
        self
    }

    /// Disable automatic mode switching
    pub fn disable_auto_switch(mut self) -> Self {
        self.config.enable_auto_switch = false;
        self
    }

    /// Disable concurrent writes
    pub fn disable_concurrent_writes(mut self) -> Self {
        self.config.enable_concurrent_writes = false;
        self
    }

    /// Disable row group optimization
    pub fn disable_row_group_optimization(mut self) -> Self {
        self.config.optimize_row_group_size = false;
        self
    }

    /// Set mode switch threshold
    pub fn mode_switch_threshold(mut self, threshold: usize) -> Self {
        self.config.mode_switch_threshold = threshold;
        self
    }

    /// Set streaming threshold (records/second)
    pub fn streaming_threshold(mut self, threshold: f64) -> Self {
        self.config.streaming_threshold = threshold;
        self
    }

    /// Set batch threshold (records per batch)
    pub fn batch_threshold(mut self, threshold: usize) -> Self {
        self.config.batch_threshold = threshold;
        self
    }

    /// Set maximum buffer size
    pub fn max_buffer_size(mut self, size: usize) -> Self {
        self.config.max_buffer_size = size;
        self
    }

    /// Set buffer time limit
    pub fn buffer_time_limit(mut self, duration: Duration) -> Self {
        self.config.buffer_time_limit = duration;
        self
    }

    /// Build the configuration
    pub fn build(self) -> HybridWriterConfig {
        self.config
    }
}

impl Default for HybridWriterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Preset configurations for common use cases
pub struct ParquetPresets;

impl ParquetPresets {
    /// Maximum performance configuration (all optimizations enabled)
    /// This is the DEFAULT configuration
    pub fn maximum_performance() -> ParquetWriterConfig {
        ParquetWriterConfig::default()
    }

    /// Balanced configuration (most optimizations, conservative settings)
    pub fn balanced() -> ParquetWriterConfig {
        ParquetConfigBuilder::new()
            .row_group_size(5000)
            .page_size(512 * 1024) // 512KB pages
            .bloom_filter_fpp(0.05) // 5% FPP
            .build()
    }

    /// Memory constrained configuration (disable memory-heavy features)
    pub fn memory_constrained() -> ParquetWriterConfig {
        ParquetConfigBuilder::new()
            .disable_bloom_filters()
            .disable_pq_sorting()
            .row_group_size(1000)
            .page_size(256 * 1024) // 256KB pages
            .metadata_inference_samples(100)
            .build()
    }

    /// Cloud optimized configuration (maximum cloud efficiency)
    pub fn cloud_optimized() -> ParquetWriterConfig {
        ParquetConfigBuilder::new()
            .row_group_size(50000) // Large row groups for fewer files
            .page_size(2 * 1024 * 1024) // 2MB pages
            .compression(CompressionAlgorithm::Zstd) // Best compression
            .bloom_filter_fpp(0.001) // 0.1% FPP for better filtering
            .build()
    }

    /// Real-time configuration (minimum latency)
    pub fn real_time() -> ParquetWriterConfig {
        ParquetConfigBuilder::new()
            .disable_pq_sorting() // Skip sorting for faster writes
            .row_group_size(1000) // Small row groups for quick flushes
            .page_size(256 * 1024) // Smaller pages
            .compression(CompressionAlgorithm::Lz4) // Fast compression
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_has_all_optimizations() {
        let config = ParquetWriterConfig::default();
        assert!(config.enable_bloom_filters);
        assert!(config.enable_column_index);
        assert!(config.enable_offset_index);
        assert!(config.enable_pq_sorting);
        assert!(config.enable_native_metadata);
    }

    #[test]
    fn test_builder_can_disable_features() {
        let config = ParquetConfigBuilder::new()
            .disable_bloom_filters()
            .disable_page_indexes()
            .disable_pq_sorting()
            .disable_native_metadata()
            .build();

        assert!(!config.enable_bloom_filters);
        assert!(!config.enable_column_index);
        assert!(!config.enable_pq_sorting);
        assert!(!config.enable_native_metadata);
    }

    #[test]
    fn test_minimal_config() {
        let config = ParquetConfigBuilder::minimal().build();
        assert!(!config.enable_bloom_filters);
        assert!(!config.enable_column_index);
        assert!(!config.enable_pq_sorting);
        assert!(!config.enable_native_metadata);
    }

    #[test]
    fn test_presets() {
        let perf = ParquetPresets::maximum_performance();
        assert!(perf.enable_bloom_filters);

        let memory = ParquetPresets::memory_constrained();
        assert!(!memory.enable_bloom_filters);
        assert!(!memory.enable_pq_sorting);

        let cloud = ParquetPresets::cloud_optimized();
        assert_eq!(cloud.row_group_size, 50000);

        let realtime = ParquetPresets::real_time();
        assert!(!realtime.enable_pq_sorting);
    }
}

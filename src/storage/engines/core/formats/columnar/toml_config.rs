//! TOML Configuration Support for Parquet Optimizations
//! 
//! Provides structures to parse TOML configuration files and apply
//! settings to storage engines with support for per-collection overrides.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use anyhow::{Result, Context};

use crate::core::compression::CompressionAlgorithm;
use crate::storage::engines::core::formats::columnar::{
    ParquetWriterConfig, FooterCacheConfig, HybridWriterConfig,
    WriterMode, ParquetConfigBuilder, FooterCacheBuilder, HybridWriterBuilder,
    ParquetPresets,
};

/// Root configuration structure matching TOML file
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StorageConfig {
    /// Global storage settings
    #[serde(default)]
    pub storage: GlobalStorageConfig,
    
    /// Monitoring settings
    #[serde(default)]
    pub monitoring: MonitoringConfig,
    
    /// Migration settings
    #[serde(default)]
    pub migration: MigrationConfig,
    
    /// Advanced settings
    #[serde(default)]
    pub advanced: AdvancedConfig,
}

/// Global storage configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GlobalStorageConfig {
    /// Default storage engine
    #[serde(default = "default_engine")]
    pub default_engine: String,
    
    /// Enable Parquet optimizations globally
    #[serde(default = "default_true")]
    pub enable_parquet_optimizations: bool,
    
    /// Parquet writer configuration
    #[serde(default)]
    pub parquet_writer: TomlParquetWriterConfig,
    
    /// Footer cache configuration
    #[serde(default)]
    pub footer_cache: TomlFooterCacheConfig,
    
    /// Hybrid writer configuration
    #[serde(default)]
    pub hybrid_writer: TomlHybridWriterConfig,
    
    /// Per-engine configurations
    #[serde(default)]
    pub engines: EngineConfigs,
}

/// TOML representation of Parquet writer config
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TomlParquetWriterConfig {
    #[serde(default = "default_row_group_size")]
    pub row_group_size: usize,
    
    #[serde(default = "default_page_size")]
    pub page_size: usize,
    
    #[serde(default = "default_true")]
    pub enable_bloom_filters: bool,
    
    #[serde(default = "default_bloom_fpp")]
    pub bloom_filter_fpp: f64,
    
    #[serde(default)]
    pub bloom_filter_columns: Vec<String>,
    
    #[serde(default = "default_true")]
    pub enable_column_statistics: bool,
    
    #[serde(default = "default_true")]
    pub enable_page_index: bool,
    
    #[serde(default = "default_true")]
    pub enable_column_index: bool,
    
    #[serde(default = "default_true")]
    pub enable_offset_index: bool,
    
    #[serde(default = "default_page_index_granularity")]
    pub page_index_granularity: usize,
    
    #[serde(default = "default_compression")]
    pub compression: String,
    
    #[serde(default = "default_true")]
    pub enable_dictionary: bool,
    
    #[serde(default = "default_dictionary_threshold")]
    pub dictionary_threshold: f64,
    
    #[serde(default = "default_true")]
    pub enable_delta_encoding: bool,
    
    #[serde(default = "default_true")]
    pub enable_byte_stream_split: bool,
    
    #[serde(default = "default_true")]
    pub enable_pq_sorting: bool,
    
    #[serde(default = "default_pq_segments")]
    pub pq_sorting_segments: usize,
    
    #[serde(default = "default_pq_codebook_size")]
    pub pq_sorting_codebook_size: usize,
    
    #[serde(default = "default_true")]
    pub enable_native_metadata: bool,
    
    #[serde(default = "default_metadata_samples")]
    pub metadata_inference_samples: usize,
    
    #[serde(default = "default_write_batch_size")]
    pub write_batch_size: usize,
    
    #[serde(default = "default_false")]
    pub id_less_storage: bool,
}

/// TOML representation of footer cache config
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TomlFooterCacheConfig {
    #[serde(default = "default_true")]
    pub enable: bool,
    
    #[serde(default = "default_cache_entries")]
    pub max_entries: u64,
    
    #[serde(default = "default_ttl_seconds")]
    pub ttl_seconds: u64,
    
    #[serde(default = "default_idle_seconds")]
    pub time_to_idle_seconds: u64,
    
    #[serde(default = "default_true")]
    pub enable_persistence: bool,
    
    #[serde(default = "default_persistence_path")]
    pub persistence_path: String,
    
    #[serde(default = "default_true")]
    pub enable_prefetch: bool,
    
    #[serde(default = "default_prefetch_threshold")]
    pub prefetch_threshold: u64,
    
    #[serde(default = "default_warming_interval")]
    pub warming_interval_seconds: u64,
    
    #[serde(default = "default_true")]
    pub compression: bool,
    
    #[serde(default = "default_compression_level")]
    pub compression_level: i32,
}

/// TOML representation of hybrid writer config
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TomlHybridWriterConfig {
    #[serde(default = "default_true")]
    pub enable: bool,
    
    #[serde(default = "default_writer_mode")]
    pub initial_mode: String,
    
    #[serde(default = "default_true")]
    pub enable_auto_switch: bool,
    
    #[serde(default = "default_mode_switch_threshold")]
    pub mode_switch_threshold: usize,
    
    #[serde(default = "default_pattern_window")]
    pub pattern_window_size: usize,
    
    #[serde(default = "default_streaming_threshold")]
    pub streaming_threshold: f64,
    
    #[serde(default = "default_batch_threshold")]
    pub batch_threshold: usize,
    
    #[serde(default = "default_max_buffer_size")]
    pub max_buffer_size: usize,
    
    #[serde(default = "default_buffer_time_limit")]
    pub buffer_time_limit_seconds: u64,
    
    #[serde(default = "default_true")]
    pub enable_concurrent_writes: bool,
    
    #[serde(default = "default_concurrent_writers")]
    pub max_concurrent_writers: usize,
    
    #[serde(default = "default_true")]
    pub optimize_row_group_size: bool,
    
    #[serde(default = "default_min_row_group")]
    pub min_row_group_size: usize,
    
    #[serde(default = "default_max_row_group")]
    pub max_row_group_size: usize,
}

/// Per-engine configurations
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EngineConfigs {
    #[serde(default)]
    pub viper: EngineConfig,
    
    #[serde(default)]
    pub nova: EngineConfig,
    
    #[serde(default)]
    pub sst: SstEngineConfig,
}

/// Individual engine configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EngineConfig {
    #[serde(default = "default_true")]
    pub inherit_global_settings: bool,
    
    /// Optional overrides
    pub parquet_writer: Option<TomlParquetWriterConfig>,
    pub footer_cache: Option<TomlFooterCacheConfig>,
    pub hybrid_writer: Option<TomlHybridWriterConfig>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            inherit_global_settings: true,
            parquet_writer: None,
            footer_cache: None,
            hybrid_writer: None,
        }
    }
}

/// SST-specific configuration
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SstEngineConfig {
    #[serde(default = "default_true")]
    pub enable_bloom_filters: bool,
    
    #[serde(default = "default_bloom_fpp")]
    pub bloom_filter_fpp: f64,
    
    #[serde(default = "default_sst_compression")]
    pub compression: String,
    
    #[serde(default = "default_compression_level")]
    pub compression_level: i32,
}

/// Monitoring configuration
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MonitoringConfig {
    #[serde(default)]
    pub parquet_optimizations: MonitoringSettings,
}

/// Monitoring settings
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MonitoringSettings {
    #[serde(default = "default_true")]
    pub enable_metrics: bool,
    
    #[serde(default = "default_metrics_interval")]
    pub metrics_interval_seconds: u64,
    
    #[serde(default = "default_cache_threshold")]
    pub cache_hit_rate_threshold: f64,
    
    #[serde(default = "default_compression_threshold")]
    pub compression_ratio_threshold: f64,
    
    #[serde(default = "default_switch_frequency")]
    pub mode_switch_frequency: usize,
}

impl Default for MonitoringSettings {
    fn default() -> Self {
        Self {
            enable_metrics: true,
            metrics_interval_seconds: 60,
            cache_hit_rate_threshold: 0.8,
            compression_ratio_threshold: 2.0,
            mode_switch_frequency: 10,
        }
    }
}

/// Migration configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MigrationConfig {
    #[serde(default = "default_true")]
    pub auto_migrate: bool,
    
    #[serde(default = "default_true")]
    pub apply_optimizations_to_existing: bool,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            auto_migrate: true,
            apply_optimizations_to_existing: true,
        }
    }
}

/// Advanced configuration
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AdvancedConfig {
    #[serde(default = "default_writer_memory")]
    pub max_memory_per_writer_mb: usize,
    
    #[serde(default = "default_cache_memory")]
    pub cache_memory_limit_mb: usize,
    
    #[serde(default = "default_io_threads")]
    pub io_threads: usize,
    
    #[serde(default = "default_prefetch_depth")]
    pub prefetch_depth: usize,
    
    #[serde(default = "default_true")]
    pub enable_hardware_acceleration: bool,
}

// Default value functions
fn default_true() -> bool { true }
fn default_false() -> bool { false }
fn default_engine() -> String { "viper".to_string() }
fn default_row_group_size() -> usize { 10_000 }
fn default_page_size() -> usize { 1_048_576 }
fn default_bloom_fpp() -> f64 { 0.01 }
fn default_page_index_granularity() -> usize { 1_000 }
fn default_compression() -> String { "mixed".to_string() }
fn default_dictionary_threshold() -> f64 { 0.7 }
fn default_pq_segments() -> usize { 8 }
fn default_pq_codebook_size() -> usize { 256 }
fn default_metadata_samples() -> usize { 1_000 }
fn default_write_batch_size() -> usize { 1_000 }
fn default_cache_entries() -> u64 { 10_000 }
fn default_ttl_seconds() -> u64 { 3_600 }
fn default_idle_seconds() -> u64 { 1_800 }
fn default_persistence_path() -> String { "data/footer_cache.bin".to_string() }
fn default_prefetch_threshold() -> u64 { 10 }
fn default_warming_interval() -> u64 { 300 }
fn default_compression_level() -> i32 { 3 }
fn default_writer_mode() -> String { "adaptive".to_string() }
fn default_mode_switch_threshold() -> usize { 1_000 }
fn default_pattern_window() -> usize { 100 }
fn default_streaming_threshold() -> f64 { 100.0 }
fn default_batch_threshold() -> usize { 1_000 }
fn default_max_buffer_size() -> usize { 100_000 }
fn default_buffer_time_limit() -> u64 { 30 }
fn default_concurrent_writers() -> usize { 4 }
fn default_min_row_group() -> usize { 1_000 }
fn default_max_row_group() -> usize { 100_000 }
fn default_sst_compression() -> String { "zstd".to_string() }
fn default_metrics_interval() -> u64 { 60 }
fn default_cache_threshold() -> f64 { 0.8 }
fn default_compression_threshold() -> f64 { 2.0 }
fn default_switch_frequency() -> usize { 10 }
fn default_writer_memory() -> usize { 512 }
fn default_cache_memory() -> usize { 1_024 }
fn default_io_threads() -> usize { 8 }
fn default_prefetch_depth() -> usize { 10 }

impl Default for GlobalStorageConfig {
    fn default() -> Self {
        Self {
            default_engine: default_engine(),
            enable_parquet_optimizations: true,
            parquet_writer: TomlParquetWriterConfig::default(),
            footer_cache: TomlFooterCacheConfig::default(),
            hybrid_writer: TomlHybridWriterConfig::default(),
            engines: EngineConfigs::default(),
        }
    }
}

impl Default for TomlParquetWriterConfig {
    fn default() -> Self {
        Self {
            row_group_size: default_row_group_size(),
            page_size: default_page_size(),
            enable_bloom_filters: true,
            bloom_filter_fpp: default_bloom_fpp(),
            bloom_filter_columns: vec![],
            enable_column_statistics: true,
            enable_page_index: true,
            enable_column_index: true,
            enable_offset_index: true,
            page_index_granularity: default_page_index_granularity(),
            compression: default_compression(),
            enable_dictionary: true,
            dictionary_threshold: default_dictionary_threshold(),
            enable_delta_encoding: true,
            enable_byte_stream_split: true,
            enable_pq_sorting: true,
            pq_sorting_segments: default_pq_segments(),
            pq_sorting_codebook_size: default_pq_codebook_size(),
            enable_native_metadata: true,
            metadata_inference_samples: default_metadata_samples(),
            write_batch_size: default_write_batch_size(),
            id_less_storage: false,
        }
    }
}

impl Default for TomlFooterCacheConfig {
    fn default() -> Self {
        Self {
            enable: true,
            max_entries: default_cache_entries(),
            ttl_seconds: default_ttl_seconds(),
            time_to_idle_seconds: default_idle_seconds(),
            enable_persistence: true,
            persistence_path: default_persistence_path(),
            enable_prefetch: true,
            prefetch_threshold: default_prefetch_threshold(),
            warming_interval_seconds: default_warming_interval(),
            compression: true,
            compression_level: default_compression_level(),
        }
    }
}

impl Default for TomlHybridWriterConfig {
    fn default() -> Self {
        Self {
            enable: true,
            initial_mode: default_writer_mode(),
            enable_auto_switch: true,
            mode_switch_threshold: default_mode_switch_threshold(),
            pattern_window_size: default_pattern_window(),
            streaming_threshold: default_streaming_threshold(),
            batch_threshold: default_batch_threshold(),
            max_buffer_size: default_max_buffer_size(),
            buffer_time_limit_seconds: default_buffer_time_limit(),
            enable_concurrent_writes: true,
            max_concurrent_writers: default_concurrent_writers(),
            optimize_row_group_size: true,
            min_row_group_size: default_min_row_group(),
            max_row_group_size: default_max_row_group(),
        }
    }
}

/// Configuration loader and converter
pub struct ConfigLoader;

impl ConfigLoader {
    /// Load configuration from TOML file
    pub fn load_from_file(path: impl AsRef<std::path::Path>) -> Result<StorageConfig> {
        let content = std::fs::read_to_string(path)
            .context("Failed to read configuration file")?;
        
        let config: StorageConfig = toml::from_str(&content)
            .context("Failed to parse TOML configuration")?;
        
        Ok(config)
    }
    
    /// Load configuration with preset
    pub fn load_with_preset(preset: &str) -> Result<StorageConfig> {
        let mut config = StorageConfig::default();
        
        match preset {
            "maximum_performance" => {
                // Already the default
            }
            "balanced" => {
                config.storage.parquet_writer.row_group_size = 5_000;
                config.storage.parquet_writer.page_size = 512 * 1024;
                config.storage.parquet_writer.bloom_filter_fpp = 0.05;
            }
            "memory_constrained" => {
                config.storage.parquet_writer.enable_bloom_filters = false;
                config.storage.parquet_writer.enable_pq_sorting = false;
                config.storage.parquet_writer.row_group_size = 1_000;
                config.storage.footer_cache.max_entries = 1_000;
            }
            "cloud_optimized" => {
                config.storage.parquet_writer.row_group_size = 50_000;
                config.storage.parquet_writer.page_size = 2 * 1024 * 1024;
                config.storage.parquet_writer.storage.as_ref().and_then(|s| s.compression.as_ref()) = "zstd".to_string();
                config.storage.footer_cache.max_entries = 20_000;
            }
            "real_time" => {
                config.storage.parquet_writer.enable_pq_sorting = false;
                config.storage.parquet_writer.row_group_size = 1_000;
                config.storage.parquet_writer.storage.as_ref().and_then(|s| s.compression.as_ref()) = "lz4".to_string();
                config.storage.hybrid_writer.initial_mode = "streaming".to_string();
            }
            _ => return Err(anyhow::anyhow!("Unknown preset: {}", preset)),
        }
        
        Ok(config)
    }
    
    /// Convert TOML config to Parquet writer config
    pub fn to_parquet_config(
        toml_config: &TomlParquetWriterConfig,
        enable_optimizations: bool
    ) -> ParquetWriterConfig {
        if !enable_optimizations {
            // Return minimal config if optimizations disabled
            return ParquetConfigBuilder::minimal().build();
        }
        
        let mut builder = ParquetConfigBuilder::new();
        
        // Apply settings from TOML
        builder = builder
            .row_group_size(toml_config.row_group_size)
            .page_size(toml_config.page_size)
            .bloom_filter_fpp(toml_config.bloom_filter_fpp)
            .bloom_filter_columns(toml_config.bloom_filter_columns.clone())
            .metadata_inference_samples(toml_config.metadata_inference_samples);
        
        // Handle compression
        let compression = match toml_config.storage.as_ref().and_then(|s| s.compression.as_ref()).as_deref() {
            "zstd" => CompressionAlgorithm::Zstd,
            "lz4" => CompressionAlgorithm::Lz4,
            "snappy" => CompressionAlgorithm::Snappy,
            "gzip" => CompressionAlgorithm::Gzip,
            "mixed" | _ => CompressionAlgorithm::Mixed,
        };
        builder = builder.storage.as_ref().and_then(|s| s.compression.as_ref())(compression);
        
        // Apply feature toggles
        if !toml_config.enable_bloom_filters {
            builder = builder.disable_bloom_filters();
        }
        if !toml_config.enable_column_index || !toml_config.enable_offset_index {
            builder = builder.disable_page_indexes();
        }
        if !toml_config.enable_pq_sorting {
            builder = builder.disable_pq_sorting();
        }
        if !toml_config.enable_native_metadata {
            builder = builder.disable_native_metadata();
        }
        
        builder.build()
    }
    
    /// Convert TOML config to footer cache config
    pub fn to_footer_cache_config(
        toml_config: &TomlFooterCacheConfig,
        enable: bool
    ) -> FooterCacheConfig {
        if !enable {
            // Return disabled config
            return FooterCacheBuilder::new()
                .disable_persistence()
                .disable_prefetch()
                .disable_compression()
                .max_entries(0)
                .build();
        }
        
        let mut builder = FooterCacheBuilder::new()
            .max_entries(toml_config.max_entries)
            .ttl(Duration::from_secs(toml_config.ttl_seconds))
            .time_to_idle(Duration::from_secs(toml_config.time_to_idle_seconds))
            .prefetch_threshold(toml_config.prefetch_threshold);
        
        if !toml_config.enable_persistence {
            builder = builder.disable_persistence();
        }
        if !toml_config.enable_prefetch {
            builder = builder.disable_prefetch();
        }
        if !toml_config.enable_compression {
            builder = builder.disable_compression();
        }
        
        builder.build()
    }
    
    /// Convert TOML config to hybrid writer config
    pub fn to_hybrid_writer_config(
        toml_config: &TomlHybridWriterConfig,
        base_config: ParquetWriterConfig,
        enable: bool
    ) -> HybridWriterConfig {
        if !enable {
            // Return simple batch mode if disabled
            return HybridWriterBuilder::new()
                .batch_mode()
                .disable_auto_switch()
                .build();
        }
        
        let mut builder = HybridWriterBuilder::new();
        
        // Set mode
        match toml_config.initial_mode.as_str() {
            "streaming" => builder = builder.streaming_mode(),
            "batch" => builder = builder.batch_mode(),
            _ => {} // Keep adaptive
        }
        
        // Apply settings
        builder = builder
            .mode_switch_threshold(toml_config.mode_switch_threshold)
            .streaming_threshold(toml_config.streaming_threshold)
            .batch_threshold(toml_config.batch_threshold)
            .max_buffer_size(toml_config.max_buffer_size)
            .buffer_time_limit(Duration::from_secs(toml_config.buffer_time_limit_seconds));
        
        if !toml_config.enable_auto_switch {
            builder = builder.disable_auto_switch();
        }
        if !toml_config.enable_concurrent_writes {
            builder = builder.disable_concurrent_writes();
        }
        if !toml_config.optimize_row_group_size {
            builder = builder.disable_row_group_optimization();
        }
        
        let mut config = builder.build();
        config.base_config = base_config;
        config
    }
    
    /// Get configuration for a specific engine
    pub fn get_engine_config(
        config: &StorageConfig,
        engine: &str
    ) -> (ParquetWriterConfig, FooterCacheConfig, HybridWriterConfig) {
        let global_enabled = config.storage.enable_parquet_optimizations;
        
        // Get engine-specific config or use global
        let (parquet_toml, cache_toml, hybrid_toml, inherit) = match engine {
            "viper" => {
                let eng = &config.storage.engines.viper;
                (
                    eng.parquet_writer.as_ref(),
                    eng.footer_cache.as_ref(),
                    eng.hybrid_writer.as_ref(),
                    eng.inherit_global_settings
                )
            }
            "nova" => {
                let eng = &config.storage.engines.nova;
                (
                    eng.parquet_writer.as_ref(),
                    eng.footer_cache.as_ref(),
                    eng.hybrid_writer.as_ref(),
                    eng.inherit_global_settings
                )
            }
            _ => (
                &config.storage.parquet_writer,
                &config.storage.footer_cache,
                &config.storage.hybrid_writer,
                true
            )
        };
        
        let enable = global_enabled && inherit;
        
        let parquet_config = Self::to_parquet_config(parquet_toml, enable);
        let cache_config = Self::to_footer_cache_config(cache_toml, enable && cache_toml.enable);
        let hybrid_config = Self::to_hybrid_writer_config(
            hybrid_toml,
            parquet_config.clone(),
            enable && hybrid_toml.enable
        );
        
        (parquet_config, cache_config, hybrid_config)
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage: GlobalStorageConfig::default(),
            monitoring: MonitoringConfig::default(),
            migration: MigrationConfig::default(),
            advanced: AdvancedConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_default_config_has_optimizations() {
        let config = StorageConfig::default();
        assert!(config.storage.enable_parquet_optimizations);
        assert!(config.storage.parquet_writer.enable_bloom_filters);
        assert!(config.storage.parquet_writer.enable_pq_sorting);
        assert!(config.storage.footer_cache.enable);
        assert!(config.storage.hybrid_writer.enable);
    }
    
    #[test]
    fn test_load_preset_configs() {
        let perf = ConfigLoader::load_with_preset("maximum_performance").unwrap();
        assert!(perf.storage.parquet_writer.enable_bloom_filters);
        
        let memory = ConfigLoader::load_with_preset("memory_constrained").unwrap();
        assert!(!memory.storage.parquet_writer.enable_bloom_filters);
        assert!(!memory.storage.parquet_writer.enable_pq_sorting);
        
        let cloud = ConfigLoader::load_with_preset("cloud_optimized").unwrap();
        assert_eq!(cloud.storage.parquet_writer.row_group_size, 50_000);
    }
    
    #[test]
    fn test_engine_specific_config() {
        let mut config = StorageConfig::default();
        
        // Set VIPER-specific override
        config.storage.engines.viper.parquet_writer = Some(TomlParquetWriterConfig {
            row_group_size: 100_000,
            ..Default::default()
        });
        
        let (viper_config, _, _) = ConfigLoader::get_engine_config(&config, "viper");
        assert_eq!(viper_config.row_group_size, 100_000);
        
        let (nova_config, _, _) = ConfigLoader::get_engine_config(&config, "nova");
        assert_eq!(nova_config.row_group_size, 10_000); // Uses global default
    }
    
    #[test]
    fn test_disable_optimizations() {
        let mut config = StorageConfig::default();
        config.storage.enable_parquet_optimizations = false;
        
        let (parquet_config, _, _) = ConfigLoader::get_engine_config(&config, "viper");
        assert!(!parquet_config.enable_bloom_filters);
        assert!(!parquet_config.enable_pq_sorting);
    }
}
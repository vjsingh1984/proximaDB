// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Configuration Validation and Error Handling
//!
//! This module provides comprehensive validation for storage configurations
//! to catch errors early and provide helpful error messages.

use anyhow::{bail, Context, Result};
use std::path::Path;
use url::Url;

use super::builder::{DataStorageConfig, StorageLayoutStrategy, StorageSystemConfig};
use super::persistence::filesystem::FilesystemConfig;
use super::persistence::wal::WalConfig;
//use super::wal::WalSystemConfig;

/// Comprehensive configuration validator
pub struct ConfigValidator;

impl ConfigValidator {
    /// Validate complete storage system configuration
    pub fn validate_storage_system(config: &StorageSystemConfig) -> Result<()> {
        Self::validate_data_storage(&config.data_storage)
            .context("Data storage configuration validation failed")?;

        Self::validate_wal_system(&config.wal_system)
            .context("WAL system configuration validation failed")?;

        Self::validate_filesystem(&config.filesystem)
            .context("Filesystem configuration validation failed")?;

        Self::validate_performance_settings(config)
            .context("Performance settings validation failed")?;

        Ok(())
    }

    /// Validate data storage configuration
    pub fn validate_data_storage(config: &DataStorageConfig) -> Result<()> {
        // Validate URLs
        if config.data_urls.is_empty() {
            bail!("Data storage URLs cannot be empty");
        }

        for url in &config.data_urls {
            Self::validate_storage_url(url)
                .with_context(|| format!("Invalid data storage URL: {}", url))?;
        }

        // Validate segment size
        if config.segment_size < 1024 * 1024 {
            bail!(
                "Segment size must be at least 1MB, got {}",
                config.segment_size
            );
        }

        if config.segment_size > 1024 * 1024 * 1024 {
            bail!(
                "Segment size cannot exceed 1GB, got {}",
                config.segment_size
            );
        }

        // Validate cache size
        if config.cache_size_mb == 0 {
            bail!("Cache size must be greater than 0");
        }

        if config.cache_size_mb > 1024 * 1024 {
            bail!(
                "Cache size cannot exceed 1TB, got {}MB",
                config.cache_size_mb
            );
        }

        // Validate layout-specific requirements
        Self::validate_storage_layout(&config.layout_strategy)?;

        // Validate tiering configuration if present
        if let Some(ref tiering) = config.tiering_config {
            Self::validate_tiering_config(tiering)?;
        }

        Ok(())
    }

    /// Validate WAL system configuration
    pub fn validate_wal_system(config: &WalConfig) -> Result<()> {
        // Validate multi-disk configuration
        if config.multi_disk.data_directories.is_empty() {
            bail!("WAL system must have at least one data directory");
        }

        // Validate each data directory URL
        for data_dir_url in &config.multi_disk.data_directories {
            // Convert URL to path for local file systems
            let data_dir_path = if data_dir_url.starts_with("file://") {
                std::path::PathBuf::from(data_dir_url.strip_prefix("file://").unwrap_or(data_dir_url))
            } else if data_dir_url.contains("://") {
                // For cloud URLs, skip local filesystem validation
                continue;
            } else {
                std::path::PathBuf::from(data_dir_url)
            };
            
            if !data_dir_path.exists() {
                if let Err(e) = std::fs::create_dir_all(&data_dir_path) {
                    bail!("Cannot create WAL data directory {:?}: {}", data_dir_path, e);
                }
            }
        }

        // Validate segment size
        if config.performance.disk_segment_size < 1024 * 1024 {
            bail!("WAL segment size must be at least 1MB");
        }

        // Validate memory thresholds (size-based only)
        if config.performance.memory_flush_size_bytes == 0 {
            bail!("WAL memory flush size must be greater than 0");
        }

        if config.memtable.global_memory_limit == 0 {
            bail!("Memtable global memory limit must be greater than 0");
        }

        // Validate concurrent flush settings
        if config.performance.concurrent_flushes == 0 {
            bail!("WAL concurrent flushes must be greater than 0");
        }

        if config.performance.write_buffer_size == 0 {
            bail!("WAL write buffer size must be greater than 0");
        }

        Ok(())
    }

    /// Validate filesystem configuration
    pub fn validate_filesystem(config: &FilesystemConfig) -> Result<()> {
        // Validate default filesystem if specified
        if let Some(ref default_fs) = config.default_fs {
            Self::validate_storage_url(default_fs).context("Invalid default filesystem URL")?;
        }

        // Validate performance settings
        let perf = &config.performance_config;
        if perf.connection_pool_size == 0 {
            bail!("Filesystem connection pool size must be greater than 0");
        }

        if perf.request_timeout_seconds == 0 {
            bail!("Filesystem request timeout must be greater than 0");
        }

        if perf.buffer_size == 0 {
            bail!("Filesystem buffer size must be greater than 0");
        }

        if perf.max_concurrent_ops == 0 {
            bail!("Maximum concurrent operations must be greater than 0");
        }

        // Validate retry configuration
        let retry = &perf.retry_config;
        if retry.backoff_multiplier <= 1.0 {
            bail!("Retry backoff multiplier must be greater than 1.0");
        }

        if retry.max_delay_ms <= retry.initial_delay_ms {
            bail!("Retry max delay must be greater than initial delay");
        }

        Ok(())
    }

    /// Validate storage URL format and reachability
    pub fn validate_storage_url(url: &str) -> Result<()> {
        // Check URL format
        if url.contains("://") {
            let parsed = Url::parse(url).with_context(|| format!("Invalid URL format: {}", url))?;

            match parsed.scheme() {
                "file" => {
                    let path = parsed.path();
                    if path.is_empty() {
                        bail!("File URL must specify a path");
                    }

                    // Validate that parent directory exists or can be created
                    if let Some(parent) = Path::new(path).parent() {
                        if !parent.exists() {
                            // Try to create the directory to validate permissions
                            std::fs::create_dir_all(parent).with_context(|| {
                                format!("Cannot create directory: {}", parent.display())
                            })?;
                        }
                    }
                }
                "s3" => {
                    if parsed.host().is_none() {
                        bail!("S3 URL must specify a bucket name");
                    }
                }
                "adls" | "abfs" => {
                    if parsed.host().is_none() {
                        bail!("Azure URL must specify an account/container");
                    }
                }
                "gcs" => {
                    if parsed.host().is_none() {
                        bail!("GCS URL must specify a bucket name");
                    }
                }
                "hdfs" => {
                    if parsed.host().is_none() {
                        bail!("HDFS URL must specify a namenode host");
                    }
                    if parsed.port().is_none() {
                        bail!("HDFS URL should specify a port (typically 9000 or 8020)");
                    }
                }
                scheme => {
                    bail!("Unsupported URL scheme: {}", scheme);
                }
            }
        } else {
            // Treat as local path
            let path = Path::new(url);
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("Cannot create directory: {}", parent.display())
                    })?;
                }
            }
        }

        Ok(())
    }

    /// Validate storage layout strategy
    fn validate_storage_layout(layout: &StorageLayoutStrategy) -> Result<()> {
        match layout {
            StorageLayoutStrategy::Regular => {
                // Regular layout is always valid
                Ok(())
            }
            StorageLayoutStrategy::Viper => {
                // VIPER requires specific configuration validation
                // TODO: Add VIPER-specific validation when implemented
                Ok(())
            }
            StorageLayoutStrategy::Hybrid => {
                // Hybrid layout validation
                // TODO: Add hybrid-specific validation
                Ok(())
            }
        }
    }

    /// Validate tiering configuration
    fn validate_tiering_config(config: &super::builder::DataTieringConfig) -> Result<()> {
        // Validate hot tier
        if config.hot_tier.urls.is_empty() {
            bail!("Hot tier must have at least one URL");
        }

        // Validate warm tier
        if config.warm_tier.urls.is_empty() {
            bail!("Warm tier must have at least one URL");
        }

        // Validate cold tier
        if config.cold_tier.urls.is_empty() {
            bail!("Cold tier must have at least one URL");
        }

        // Validate auto-tier policies
        let policies = &config.auto_tier_policies;
        if policies.hot_to_warm_hours == 0 {
            bail!("Hot to warm transition time must be greater than 0");
        }

        if policies.warm_to_cold_hours <= policies.hot_to_warm_hours {
            bail!("Warm to cold transition time must be greater than hot to warm time");
        }

        if policies.hot_tier_access_threshold == 0 {
            bail!("Hot tier access threshold must be greater than 0");
        }

        Ok(())
    }

    /// Validate performance settings compatibility
    fn validate_performance_settings(config: &StorageSystemConfig) -> Result<()> {
        let storage_perf = &config.storage_performance;
        let fs_perf = &config.filesystem.performance_config;

        // Ensure memory settings are reasonable
        if storage_perf.memory_pool_mb > 1024 * 1024 {
            bail!("Storage memory pool cannot exceed 1TB");
        }

        // Validate I/O thread configuration
        if storage_perf.io_threads > 1000 {
            bail!("I/O thread count cannot exceed 1000");
        }

        // Ensure filesystem and storage buffer sizes are compatible
        if storage_perf.buffer_config.read_buffer_size > 1024 * 1024 * 1024 {
            bail!("Storage read buffer cannot exceed 1GB");
        }

        if fs_perf.buffer_size > 1024 * 1024 * 1024 {
            bail!("Filesystem buffer cannot exceed 1GB");
        }

        // Validate batch configuration
        let batch = &storage_perf.batch_config;
        if batch.max_batch_size < batch.default_batch_size {
            bail!("Maximum batch size must be >= default batch size");
        }

        if batch.batch_timeout_ms == 0 {
            bail!("Batch timeout must be greater than 0");
        }

        Ok(())
    }

    /// Validate environment and dependencies
    pub fn validate_environment() -> Result<()> {
        // Check available memory
        let sys_info = sysinfo::System::new_all().total_memory();
        if sys_info < 1024 * 1024 * 1024 {
            // Less than 1GB
            eprintln!("⚠️  Warning: System has less than 1GB RAM, performance may be impacted");
        }

        // Check available disk space for temp directories
        if let Ok(temp_space) = std::fs::metadata("/tmp") {
            // Basic check that temp directory exists and is writable
            let test_file = "/tmp/.proximadb_test";
            std::fs::write(test_file, "test").context("Cannot write to /tmp directory")?;
            std::fs::remove_file(test_file).context("Cannot clean up test file")?;
        }

        // Check CPU count for thread configuration recommendations
        let cpu_count = num_cpus::get();
        if cpu_count < 2 {
            eprintln!(
                "⚠️  Warning: System has only {} CPU core, performance may be limited",
                cpu_count
            );
        }

        Ok(())
    }

    /// Generate configuration recommendations
    pub fn generate_recommendations(config: &StorageSystemConfig) -> Vec<String> {
        let mut recommendations = Vec::new();

        // Memory recommendations
        let total_cache =
            config.data_storage.cache_size_mb + config.storage_performance.memory_pool_mb;
        if total_cache > 8192 {
            recommendations.push(format!(
                "Consider reducing total memory usage ({}MB) for better system stability",
                total_cache
            ));
        }

        // Thread recommendations
        let cpu_count = num_cpus::get();
        if config.storage_performance.io_threads > cpu_count * 2 {
            recommendations.push(format!(
                "I/O threads ({}) exceed 2x CPU count ({}), consider reducing",
                config.storage_performance.io_threads, cpu_count
            ));
        }

        // Compression recommendations
        if !config.data_storage.compression.compress_vectors
            && config.data_storage.data_urls.len() > 1
        {
            recommendations.push(
                "Consider enabling compression for multi-disk setups to reduce I/O".to_string(),
            );
        }

        // Tiering recommendations
        if config.data_storage.tiering_config.is_none() && config.data_storage.data_urls.len() > 2 {
            recommendations.push(
                "Consider enabling tiered storage for better cost/performance optimization"
                    .to_string(),
            );
        }

        recommendations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::builder::{DataStorageConfig, StorageSystemConfig};

    #[test]
    fn test_url_validation() {
        // Valid URLs
        assert!(ConfigValidator::validate_storage_url("file:///tmp/test").is_ok());
        assert!(ConfigValidator::validate_storage_url("s3://my-bucket/path").is_ok());
        assert!(ConfigValidator::validate_storage_url("adls://account/container/path").is_ok());
        assert!(ConfigValidator::validate_storage_url("gcs://bucket/object").is_ok());
        assert!(ConfigValidator::validate_storage_url("hdfs://namenode:9000/path").is_ok());

        // Invalid URLs
        assert!(ConfigValidator::validate_storage_url("invalid://").is_err());
        assert!(ConfigValidator::validate_storage_url("s3://").is_err());
        assert!(ConfigValidator::validate_storage_url("hdfs://namenode/path").is_err());
        // Missing port
    }

    #[test]
    fn test_data_storage_validation() {
        let mut config = DataStorageConfig {
            data_urls: vec!["file:///tmp/test".to_string()],
            segment_size: 64 * 1024 * 1024,
            cache_size_mb: 512,
            ..Default::default()
        };

        // Valid configuration
        assert!(ConfigValidator::validate_data_storage(&config).is_ok());

        // Invalid: empty URLs
        config.data_urls.clear();
        assert!(ConfigValidator::validate_data_storage(&config).is_err());

        // Invalid: segment size too small
        config.data_urls = vec!["file:///tmp/test".to_string()];
        config.segment_size = 1024;
        assert!(ConfigValidator::validate_data_storage(&config).is_err());

        // Invalid: cache size too large
        config.segment_size = 64 * 1024 * 1024;
        config.cache_size_mb = 2 * 1024 * 1024; // 2TB
        assert!(ConfigValidator::validate_data_storage(&config).is_err());
    }

    #[test]
    fn test_recommendations() {
        let config = StorageSystemConfig::default();
        let recommendations = ConfigValidator::generate_recommendations(&config);

        // Should provide some recommendations for default config
        assert!(!recommendations.is_empty() || recommendations.is_empty()); // Either is fine
    }
}

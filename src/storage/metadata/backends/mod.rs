// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metadata Storage Backends
//!
//! Provides pluggable metadata storage implementations via the InternalCollectionProvider trait:
//! - UniversalMetadataBackend - Filesystem-based (supports file://, s3://, gs://, adls://)
//! - LocalRocksDbBackend - High-performance local RocksDB storage

// Active backends
#[cfg(feature = "rocksdb")]
pub mod local_rocksdb_backend;
pub mod universal_backend;

// Utilities
pub mod common_utils;
pub mod metrics_decorator;

#[cfg(test)]
mod tests {
    pub mod filestore_tests;
    // pub mod integration_tests; // TODO: Add integration tests module
}

use self::metrics_decorator::MetricsDecorator;
use crate::storage::traits::UnifiedMetricsCollector;
use anyhow::Result;
use std::sync::Arc;

/// Factory for creating metadata backend instances
pub struct MetadataBackendFactory;

impl MetadataBackendFactory {
    /// Create a metadata backend based on URL scheme
    ///
    /// # URL Schemes:
    /// - `file://` - Local filesystem
    /// - `s3://` - Amazon S3
    /// - `gs://` - Google Cloud Storage  
    /// - `adls://` - Azure Data Lake Storage
    /// - `rocksdb://` - Local RocksDB (if feature enabled)
    pub async fn create_from_url(
        url: &str,
    ) -> Result<Box<dyn crate::storage::traits::InternalCollectionProvider>> {
        if url.starts_with("rocksdb://") {
            #[cfg(feature = "rocksdb")]
            {
                let path = url.strip_prefix("rocksdb://").unwrap_or("./data/rocksdb");
                let config = local_rocksdb_backend::RocksDbMetadataConfig {
                    db_path: path.to_string(),
                    cache_size_mb: 128,
                    enable_compression: true,
                    enable_statistics: true,
                    max_open_files: 1000,
                    write_buffer_size_mb: 64,
                    max_write_buffer_number: 4,
                    target_file_size_base_mb: 64,
                    max_background_jobs: 4,
                    enable_pipelined_write: true,
                    optimize_for_point_lookup_mb: None,
                    prefix_extractor_len: None,
                    enable_bloom_filter: true,
                    bloom_filter_bits_per_key: 10,
                };
                let backend = local_rocksdb_backend::LocalRocksDbBackend::new(config).await?;
                Ok(Box::new(backend))
            }
            #[cfg(not(feature = "rocksdb"))]
            {
                anyhow::bail!("RocksDB support not compiled in. Enable 'rocksdb' feature.");
            }
        } else {
            // All other URLs use filesystem-based backend
            // The FilesystemFactory will route to the appropriate implementation
            let config = universal_backend::UniversalMetadataConfig {
                storage_url: url.to_string(),
                compression: true,
                enable_snapshots: true,
                snapshot_threshold: 1000,
                keep_snapshots: 3,
                backup_url: None,
                temp_dir: None,
            };

            // Create filesystem factory
            let fs_config = crate::storage::persistence::filesystem::FilesystemConfig {
                default_fs: Some(url.to_string()),
                local: None,
                global_options: Default::default(),
                auth_config: None,
                performance_config: Default::default(),
                scheme_mapping: Default::default(),
            };

            let filesystem_factory = Arc::new(
                crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config).await?,
            );

            let backend =
                universal_backend::UniversalMetadataBackend::new(config, filesystem_factory)
                    .await?;

            Ok(Box::new(backend))
        }
    }

    /// Create default metadata backend (local filesystem)
    pub async fn create_default()
    -> Result<Box<dyn crate::storage::traits::InternalCollectionProvider>> {
        Self::create_from_url("file://./data/metadata_info").await
    }

    /// Create a metadata backend with metrics collection
    pub async fn create_with_metrics(
        url: &str,
        metrics: Arc<UnifiedMetricsCollector>,
    ) -> Result<Box<dyn crate::storage::traits::InternalCollectionProvider>> {
        // Create the base backend
        if url.starts_with("rocksdb://") {
            #[cfg(feature = "rocksdb")]
            {
                let path = url.strip_prefix("rocksdb://").unwrap_or("./data/rocksdb");
                let config = local_rocksdb_backend::RocksDbMetadataConfig {
                    db_path: path.to_string(),
                    cache_size_mb: 128,
                    enable_compression: true,
                    enable_statistics: true,
                    max_open_files: 1000,
                    write_buffer_size_mb: 64,
                    max_write_buffer_number: 4,
                    target_file_size_base_mb: 64,
                    max_background_jobs: 4,
                    enable_pipelined_write: true,
                    optimize_for_point_lookup_mb: None,
                    prefix_extractor_len: None,
                    enable_bloom_filter: true,
                    bloom_filter_bits_per_key: 10,
                };
                let backend = local_rocksdb_backend::LocalRocksDbBackend::new(config).await?;
                Ok(Box::new(MetricsDecorator::new(backend, metrics))
                    as Box<
                        dyn crate::storage::traits::InternalCollectionProvider,
                    >)
            }
            #[cfg(not(feature = "rocksdb"))]
            {
                anyhow::bail!("RocksDB support not compiled in. Enable 'rocksdb' feature.");
            }
        } else {
            let config = universal_backend::UniversalMetadataConfig {
                storage_url: url.to_string(),
                compression: true,
                enable_snapshots: true,
                snapshot_threshold: 1000,
                keep_snapshots: 3,
                backup_url: None,
                temp_dir: None,
            };

            let fs_config = crate::storage::persistence::filesystem::FilesystemConfig {
                default_fs: Some(url.to_string()),
                local: None,
                global_options: Default::default(),
                auth_config: None,
                performance_config: Default::default(),
                scheme_mapping: Default::default(),
            };

            let filesystem_factory = Arc::new(
                crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config).await?,
            );

            let backend =
                universal_backend::UniversalMetadataBackend::new(config, filesystem_factory)
                    .await?;

            Ok(Box::new(MetricsDecorator::new(backend, metrics))
                as Box<
                    dyn crate::storage::traits::InternalCollectionProvider,
                >)
        }
    }
}

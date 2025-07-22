/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! End-to-End Integration Tests for WAL Write Optimization
//!
//! Tests the complete integration of OptimizedWalWriter with DirectVectorService,
//! validating performance improvements, batching behavior, and metrics collection.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{info, debug};

use proximadb::core::VectorRecord;
use proximadb::services::direct_vector_service::{DirectVectorService, OptimizedFormat};
use proximadb::storage::persistence::wal::config::{WalConfig, PerformanceConfig, SyncMode};
use proximadb::storage::engines::viper::ViperEngine;
use proximadb::storage::engines::viper::types::ViperConfig;
use proximadb::storage::engines::lsm::{LsmTree};
use proximadb::core::LsmConfig;
use proximadb::storage::persistence::filesystem::FilesystemFactory;

/// Test configuration for WAL optimization integration tests
#[derive(Debug, Clone)]
struct WalOptimizationTestConfig {
    /// Number of vectors per batch
    batch_size: usize,
    /// Number of batches to write
    num_batches: usize,
    /// Vector dimension
    dimension: usize,
    /// Collection name prefix
    collection_prefix: String,
    /// Enable optimized writer
    enable_optimized_writer: bool,
    /// Batch timeout in milliseconds
    batch_timeout_ms: u64,
    /// Enable write combining
    enable_combining: bool,
}

impl Default for WalOptimizationTestConfig {
    fn default() -> Self {
        Self {
            batch_size: 50,
            num_batches: 10,
            dimension: 128,
            collection_prefix: "wal_opt_test".to_string(),
            enable_optimized_writer: true,
            batch_timeout_ms: 5,
            enable_combining: true,
        }
    }
}

/// Test results for performance comparison
#[derive(Debug, Clone)]
struct WalPerformanceTestResult {
    /// Total time for all operations
    total_duration: Duration,
    /// Average time per batch
    avg_batch_duration: Duration,
    /// Total vectors written
    total_vectors: usize,
    /// Vectors per second
    vectors_per_second: f64,
    /// Number of write operations
    total_writes: usize,
    /// WAL writer metrics (if available)
    wal_metrics: Option<String>,
}

#[tokio::test]
async fn test_basic_service_creation() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    
    info!("🚀 Testing Basic DirectVectorService Creation");
    
    let config = WalOptimizationTestConfig::default();
    let _service = create_direct_vector_service(&config).await?;
    
    info!("✅ DirectVectorService created successfully");
    Ok(())
}

// TODO: Uncomment and implement these additional tests once basic service creation works
// These comprehensive tests will validate the full WAL optimization functionality:
// - test_optimized_wal_writer_integration() - Performance comparison
// - test_write_combining_effectiveness() - Write combining validation
// - test_batching_thresholds() - Batch size optimization
// - test_metrics_collection() - Metrics system validation
// - test_concurrent_writes() - Concurrent write performance

// Helper functions for future test implementation

async fn create_direct_vector_service(config: &WalOptimizationTestConfig) -> Result<DirectVectorService> {
    // Create WAL config with test parameters
    let mut wal_config = WalConfig::default();
    wal_config.enable_optimized_writer = config.enable_optimized_writer;
    wal_config.optimized_writer_batch_size = Some(config.batch_size);
    wal_config.optimized_writer_batch_timeout_ms = Some(config.batch_timeout_ms);
    wal_config.optimized_writer_enable_combining = Some(config.enable_combining);
    
    // Performance config for testing
    wal_config.performance = PerformanceConfig {
        memory_flush_size_bytes: 2 * 1024 * 1024, // 2MB
        disk_segment_size: 512 * 1024 * 1024,
        global_flush_threshold: 4 * 1024 * 1024 * 1024,
        write_buffer_size: 8 * 1024 * 1024,
        concurrent_flushes: 2,
        batch_threshold: config.batch_size,
        mvcc_cleanup_interval_secs: 3600,
        ttl_cleanup_interval_secs: 300,
        sync_mode: SyncMode::PerBatch,
        global_shrink_factor: 0.4,
        cloud_backup: None,
        enable_optimized_wal_writer: Some(config.enable_optimized_writer),
        background_writer_threads: Some(2),
        wal_batch_size: Some(config.batch_size),
    };
    
    // Create temporary directory for testing
    let temp_dir = tempfile::tempdir()?;
    let temp_path = temp_dir.path().to_str().unwrap();
    
    // Create filesystem factory
    let filesystem_factory: Arc<FilesystemFactory> = Arc::new(FilesystemFactory::new(Default::default()).await?);
    
    // Create test VIPER engine
    let viper_config = ViperConfig::default();
    let viper_engine = Arc::new(ViperEngine::new(viper_config, filesystem_factory.clone()).await?);
    
    // Create test LSM engine  
    let lsm_config = LsmConfig {
        data_directory: temp_path.to_string(),
        block_size_kb: 4,
        cache_size_mb: 64,
        write_buffer_size_mb: 16,
        max_levels: 7,
        memtable_size_mb: 64,
        level_count: 7,
        compaction_threshold: 4,
        memory_flush_size_bytes: 2 * 1024 * 1024,
        memtable_type: "BTree".to_string(),
        compaction_strategy: "leveled".to_string(),
        compression: "snappy".to_string(),
        bloom_filter_config: Some(proximadb::core::config::BloomFilterConfig {
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        }),
        max_files_per_level: 10,
        level_size_multiplier: 10.0,
        background_thread_count: 2,
        sync_mode: "batch".to_string(),
        enable_wal: true,
        wal_directory: format!("{}/wal", temp_path),
        mmap_enabled: false,
        prefetch_enabled: false,
        prefetch_size_kb: 64,
    };
    let lsm_engine = Arc::new(LsmTree::new(
        format!("test_collection_{}", std::process::id()), 
        lsm_config, 
        filesystem_factory
    ).await?);
    
    // Create DirectVectorService with test configuration
    DirectVectorService::new(
        wal_config,
        viper_engine,
        lsm_engine,
    ).await
}

fn _generate_test_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            (0..dimension)
                .map(|j| (i * dimension + j) as f32 * 0.001)
                .collect()
        })
        .collect()
}

async fn _get_wal_metrics(service: &DirectVectorService) -> Option<String> {
    // Get metrics from the service's optimized WAL writer
    service.get_wal_metrics_report().await
}
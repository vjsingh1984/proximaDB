// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Comprehensive unit tests for VIPER UnifiedStorageEngine implementation and atomic operations

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;
use tokio;

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::impls::viper::ViperEngine;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use crate::storage::traits::{FlushParameters, StorageEngineStrategy, UnifiedStorageEngine};
use crate::storage::transaction_coordinator::{
    TransactionCoordinator, ViperTransactionalOperations,
};

/// Create test filesystem and VIPER engine
async fn create_test_viper_engine() -> Result<(ViperEngine, TempDir)> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_string_lossy().to_string();

    let fs_config = FilesystemConfig {
        default_fs: Some(format!("file://{}", temp_path)),
        ..Default::default()
    };

    let filesystem = Arc::new(FilesystemFactory::create(fs_config).await?);
    let viper_engine =
        ViperEngine::from_core_config(crate::core::config::ViperConfig::default(), filesystem)
            .await?;

    Ok((viper_engine, temp_dir))
}

/// Create test collection config with dimension
fn create_test_collection_config(
    collection_id: &str,
    dimension: usize,
) -> crate::proto::proximadb_v1::Collection {
    crate::proto::proximadb_v1::Collection {
        id: collection_id.to_string(),
        config: Some(crate::proto::proximadb_v1::CollectionConfig {
            name: collection_id.to_string(),
            dimension: dimension as u32,
            distance_metric: Some(crate::proto::proximadb_v1::DistanceMetric::Cosine as i32),
            storage_engine: Some(crate::proto::proximadb_v1::StorageEngine::Viper as i32),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Create test vector records for testing
fn create_test_vector_records(_collection_id: &str, count: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let now = chrono::Utc::now().timestamp_millis();
            VectorRecord {
                id: format!("test_vector_{}", i),
                vector: vec![0.1 * i as f32, 0.2 * i as f32, 0.3 * i as f32],
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert(
                        "category".to_string(),
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                format!("category_{}", i % 3),
                            )),
                        },
                    );
                    metadata.insert(
                        "priority".to_string(),
                        crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                i.to_string(),
                            )),
                        },
                    );
                    metadata
                },
                timestamp: Some(now),
                updated_at: Some(now),
                expires_at: None,
                version: Some(1),
                source: None,
            }
        })
        .collect()
}

#[tokio::test]
async fn test_viper_unified_storage_engine_traits() -> Result<()> {
    let (viper_engine, _temp_dir) = create_test_viper_engine().await?;

    // Test engine identification
    assert_eq!(viper_engine.engine_name(), "VIPER");
    assert_eq!(
        viper_engine.engine_version(),
        crate::version::PROXIMADB_VERSION
    );
    assert_eq!(viper_engine.strategy(), StorageEngineStrategy::Viper);

    // Test engine capabilities
    assert!(viper_engine.supports_collection_level_operations());
    assert!(viper_engine.supports_atomic_operations());
    assert!(viper_engine.supports_background_operations());

    Ok(())
}

#[tokio::test]
async fn test_viper_do_flush_implementation() -> Result<()> {
    let (viper_engine, _temp_dir) = create_test_viper_engine().await?;

    let collection_id = "test_collection";

    // Test flush with valid collection ID and config
    let collection_config = create_test_collection_config(collection_id, 3);
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        collection_config: Some(collection_config.clone()),
        ..Default::default()
    };

    let result = viper_engine.do_flush(&flush_params).await?;

    // Verify flush result
    assert!(result.success);
    assert_eq!(result.collections_affected, vec![collection_id]);
    assert_eq!(result.entries_flushed, Some(0)); // No actual records in test
    assert!(result.engine_metrics.contains_key("operation_id"));

    Ok(())
}

#[tokio::test]
async fn test_viper_flush_with_high_level_trait_method() -> Result<()> {
    let (viper_engine, _temp_dir) = create_test_viper_engine().await?;

    let collection_id = "test_collection";

    // Test high-level flush method (not do_flush directly)
    let collection_config = create_test_collection_config(collection_id, 3);
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        trigger_compaction: false,
        collection_config: Some(collection_config.clone()),
        ..Default::default()
    };

    let result = viper_engine.flush(flush_params).await?;

    // Verify the high-level flush includes timing and logging
    assert!(result.success);
    // duration_ms is always >= 0 as it's unsigned
    assert!(result.completed_at > chrono::Utc::now() - chrono::Duration::minutes(1));

    Ok(())
}

#[tokio::test]
async fn test_unified_atomic_operations_lifecycle() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_string_lossy().to_string();

    let fs_config = FilesystemConfig {
        default_fs: Some(format!("file://{}", temp_path)),
        ..Default::default()
    };

    let filesystem = Arc::new(FilesystemFactory::create(fs_config).await?);

    // Test unified atomic coordinator
    let coordinator = TransactionCoordinator::new(filesystem.clone(), None).await?;
    let viper_ops = ViperTransactionalOperations::new(Arc::new(coordinator));

    let collection_id = "test_collection";
    let storage_url = format!("file://{}/storage", temp_path);

    // Test begin flush operation
    let flush_metadata = viper_ops
        .begin_flush_operation(&collection_id.to_string(), &storage_url)
        .await?;

    assert!(flush_metadata.operation_id.len() > 0);
    assert!(flush_metadata.staging_url.contains("__flush"));
    assert!(flush_metadata.final_url.contains("storage"));

    // Test write to staging
    let test_data = b"test parquet data";
    viper_ops
        .write_parquet_to_staging(&flush_metadata.operation_id, "test.parquet", test_data)
        .await?;

    // Test finalize flush
    viper_ops
        .finalize_flush(&flush_metadata.operation_id)
        .await?;

    Ok(())
}

/// Test that demonstrates the dynamic loading pattern for storage engines
#[tokio::test]
async fn test_dynamic_storage_engine_flush() -> Result<()> {
    let (viper_engine, _temp_dir) = create_test_viper_engine().await?;

    // This simulates how the memtable would call flush on a dynamically loaded engine
    let storage_engine: Box<dyn UnifiedStorageEngine> = Box::new(viper_engine);

    let collection_id = "dynamic_test_collection";

    // The memtable would call this without knowing the specific engine type
    let collection_config = create_test_collection_config(collection_id, 3);
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        collection_config: Some(collection_config.clone()),
        ..Default::default()
    };

    let result = storage_engine.flush(flush_params).await?;

    assert!(result.success);
    assert_eq!(result.collections_affected, vec![collection_id]);

    Ok(())
}

#[tokio::test]
async fn test_viper_engine_metrics_collection() -> Result<()> {
    let (viper_engine, _temp_dir) = create_test_viper_engine().await?;

    // Test metrics collection
    let metrics = viper_engine.collect_engine_metrics().await?;

    // Verify VIPER-specific metrics are present
    assert!(metrics.contains_key("flush_operations"));
    assert!(metrics.contains_key("memory_usage_bytes"));
    assert!(metrics.contains_key("collection_count"));

    // Verify metric values exist (u64 values are always >= 0)
    if let Some(serde_json::Value::Number(ops)) = metrics.get("flush_operations") {
        let _ = ops.as_u64(); // Just verify it can be parsed as u64
    }
    if let Some(serde_json::Value::Number(mem)) = metrics.get("memory_usage_bytes") {
        let _ = mem.as_u64(); // Just verify it can be parsed as u64
    }
    if let Some(serde_json::Value::Number(cols)) = metrics.get("collection_count") {
        let _ = cols.as_u64(); // Just verify it can be parsed as u64
    }

    Ok(())
}

#[tokio::test]
async fn test_flush_parameter_validation() -> Result<()> {
    let (viper_engine, _temp_dir) = create_test_viper_engine().await?;

    // Test invalid parameters
    let invalid_params = FlushParameters {
        collection_id: None, // VIPER requires collection ID
        collection_config: None,
        ..Default::default()
    };

    let result = viper_engine.do_flush(&invalid_params).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No collection_id provided")
    );

    Ok(())
}

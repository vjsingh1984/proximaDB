// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Tests for the SST storage engine.

use super::*;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::traits::UnifiedStorageEngine;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn test_sst_storage_new() {
    let dir = tempdir().unwrap();
    let path = dir.path().to_str().unwrap().to_string();

    // Create filesystem factory with proper config
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let factory = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config).await.unwrap());
    let filesystem = factory.get_filesystem(&format!("file://{}", path)).unwrap();
    let distance_compute =
        Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default());
    let config = SstConfig::default();
    let sst = SstEngine::new()
        .await
        .unwrap();
    assert_eq!(sst.engine_name(), "sst");
}

#[tokio::test]
async fn test_sst_storage_flush_and_search() {
    let dir = tempdir().unwrap();
    let path = dir.path().to_str().unwrap().to_string();

    // Create filesystem factory with proper config
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let factory = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config).await.unwrap());
    let distance_compute =
        Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default());
    let config = SstConfig::default();
    let sst = SstEngine::new_with_config(config, factory, distance_compute)
        .await
        .unwrap();

    let collection_id = "test_collection".to_string();
    let records = vec![
        VectorRecord {
            id: "1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            ..Default::default()
        },
        VectorRecord {
            id: "2".to_string(),
            vector: vec![4.0, 5.0, 6.0],
            ..Default::default()
        },
    ];

    // Create collection config for flush
    let collection = crate::proto::proximadb_v1::Collection {
        id: collection_id.clone(),
        config: Some(crate::proto::proximadb_v1::CollectionConfig {
            name: collection_id.clone(),
            dimension: 3,
            distance_metric: crate::proto::proximadb_v1::DistanceMetric::Cosine as i32,
            storage_engine: crate::proto::proximadb_v1::StorageEngine::Sst as i32,
            ..Default::default()
        }),
        storage_assignment: Some(crate::proto::proximadb_v1::StorageAssignment {
            primary_path: format!("file://{}", path),
            base_location: format!("file://{}", path),
            ..Default::default()
        }),
        ..Default::default()
    };

    let params = FlushParameters {
        collection_id: Some(collection_id.clone()),
        vector_records: records,
        collection_config: Some(collection.clone()),
        ..Default::default()
    };

    let result = sst.do_flush(&params).await.unwrap();
    assert!(result.success);

    let search_params = crate::core::search::SearchParams {
        vector: Some(vec![1.0, 2.0, 3.0]),
        top_k: Some(1),
        ..Default::default()
    };

    // Use the same collection for search
    let ctx = crate::storage::traits::StorageQueryContext {
        search_params: Arc::new(search_params),
        collection: Arc::new(collection),
        metadata: crate::storage::traits::StorageQueryMetadata {
            collection_id: collection_id.clone(),
            ..Default::default()
        },
    };

    let results = sst.search_vectors_unified(&ctx).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "1");
}

#[tokio::test]
async fn test_sst_none_compression() {
    use crate::core::compression::CompressionAlgorithm;

    // Keep the tempdir alive for the entire test
    let dir = tempdir().unwrap();
    let path = dir.path().to_str().unwrap().to_string();

    // Create filesystem factory
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let factory = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config).await.unwrap());
    let distance_compute = Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default());
    let config = SstConfig::default();

    let sst = SstEngine::new_with_config(config, factory, distance_compute)
        .await
        .unwrap();

    let collection_id = "test_none_compression".to_string();

    // Create collection with NONE compression explicitly
    let collection = crate::proto::proximadb_v1::Collection {
        id: collection_id.clone(),
        config: Some(crate::proto::proximadb_v1::CollectionConfig {
            name: collection_id.clone(),
            dimension: 3,
            distance_metric: crate::proto::proximadb_v1::DistanceMetric::Cosine as i32,
            storage_engine: crate::proto::proximadb_v1::StorageEngine::Sst as i32,
            storage_config: Some(crate::proto::proximadb_v1::StorageConfig {
                compression: crate::proto::proximadb_v1::CompressionAlgorithm::CompressionNone as i32,
                ..Default::default()
            }),
            ..Default::default()
        }),
        storage_assignment: Some(crate::proto::proximadb_v1::StorageAssignment {
            primary_path: format!("file://{}/test", path),
            base_location: format!("file://{}", path),
            ..Default::default()
        }),
        ..Default::default()
    };

    let records = vec![
        VectorRecord {
            id: "vec_0".to_string(),
            vector: vec![1.0, 0.0, 0.0],
            ..Default::default()
        },
        VectorRecord {
            id: "vec_1".to_string(),
            vector: vec![0.0, 1.0, 0.0],
            ..Default::default()
        },
    ];

    // Flush with none compression
    let params = FlushParameters {
        collection_id: Some(collection_id.clone()),
        vector_records: records.clone(),
        collection_config: Some(collection.clone()),
        ..Default::default()
    };

    eprintln!("DEBUG: Flushing with none compression to {}", path);
    let result = sst.do_flush(&params).await.unwrap();
    eprintln!("DEBUG: Flush success={}, bytes_written={:?}", result.success, result.bytes_written);
    assert!(result.success);

    // List files created
    let test_path = crate::utils::StoragePath::collection_data_path(&format!("file://{}", path), "test_none_compression");
    // Remove file:// prefix for filesystem access
    let fs_path = test_path.strip_prefix("file://").unwrap_or(&test_path);
    eprintln!("DEBUG: Checking for files in: {}", fs_path);
    match std::fs::read_dir(fs_path) {
        Ok(entries) => {
            eprintln!("DEBUG: Files found in {}:", fs_path);
            let mut count = 0;
            for entry in entries {
                if let Ok(entry) = entry {
                    count += 1;
                    eprintln!("  - {:?} (size: {} bytes)", entry.path(), entry.metadata().map(|m| m.len()).unwrap_or(0));
                }
            }
            if count == 0 {
                eprintln!("DEBUG: Directory exists but is empty!");
                // Check if file exists directly
                let expected_file = format!("{}/L0_", fs_path);
                eprintln!("DEBUG: Checking for files starting with: {}", expected_file);
            }
        }
        Err(e) => {
            eprintln!("DEBUG: Directory {} error: {}", fs_path, e);
        }
    }

    // Search
    let search_params = crate::core::search::SearchParams {
        vector: Some(vec![1.0, 0.0, 0.0]),
        top_k: Some(2),
        ..Default::default()
    };

    let ctx = crate::storage::traits::StorageQueryContext {
        search_params: Arc::new(search_params),
        collection: Arc::new(collection),
        metadata: crate::storage::traits::StorageQueryMetadata {
            collection_id: collection_id.clone(),
            ..Default::default()
        },
    };

    eprintln!("DEBUG: Searching...");
    let results = sst.search_vectors_unified(&ctx).await.unwrap();
    eprintln!("DEBUG: Search returned {} results", results.len());

    // Verify we found results
    assert!(!results.is_empty(), "Expected to find vectors but got 0 results");
    assert_eq!(results[0].id, "vec_0");
}

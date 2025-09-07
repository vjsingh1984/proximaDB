// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Tests for the SST storage engine.

use super::*;
use crate::core::VectorRecord;
use crate::storage::traits::UnifiedStorageEngine;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn test_sst_storage_new() {
    let dir = tempdir().unwrap();
    let path = dir.path().to_str().unwrap().to_string();
    let filesystem = Arc::new(FilesystemFactory::new_local(&path));
    let distance_compute =
        Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default());
    let config = SstConfig::default();
    let sst = SstStorage::new(config, filesystem, distance_compute)
        .await
        .unwrap();
    assert_eq!(sst.engine_name(), "sst");
}

#[tokio::test]
async fn test_sst_storage_flush_and_search() {
    let dir = tempdir().unwrap();
    let path = dir.path().to_str().unwrap().to_string();
    let filesystem = Arc::new(FilesystemFactory::new_local(&path));
    let distance_compute =
        Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default());
    let config = SstConfig::default();
    let sst = SstStorage::new(config, filesystem, distance_compute)
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

    let params = FlushParameters {
        collection_id: Some(collection_id.clone()),
        vector_records: records,
        ..Default::default()
    };

    let result = sst.do_flush(&params).await.unwrap();
    assert!(result.success);

    let search_params = crate::core::search::SearchParams {
        vector: Some(vec![1.0, 2.0, 3.0]),
        top_k: Some(1),
        ..Default::default()
    };

    let collection = Arc::new(crate::proto::proximadb::Collection {
        id: collection_id.clone(),
        config: Some(crate::proto::proximadb::CollectionConfig {
            name: collection_id.clone(),
            dimension: 3,
            distance_metric: crate::proto::proximadb::DistanceMetric::Cosine as i32,
            storage_engine: crate::proto::proximadb::StorageEngine::Sst as i32,
            ..Default::default()
        }),
        ..Default::default()
    });

    let ctx = crate::storage::traits::StorageQueryContext {
        search_params: Arc::new(search_params),
        collection,
        metadata: Default::default(),
    };

    let results = sst.search_vectors_unified(&ctx).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "1");
}

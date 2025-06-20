/*
 * Copyright 2025 Vijaykumar Singh
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

use proximadb::core::{LsmConfig, StorageConfig};
use proximadb::network::grpc::service::ProximaDbGrpcService;
use proximadb::proto::vectordb::v1::vector_db_server::{VectorDb, VectorDbServer};
use proximadb::proto::vectordb::v1::*;
use proximadb::storage::StorageEngine;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;
use tonic::{transport::Server, Request};

#[tokio::test]
async fn test_grpc_health_check() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = StorageConfig {
        data_dirs: vec![data_dir.clone()],
        wal_dir: wal_dir.clone(),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 1,
            level_count: 3,
            compaction_threshold: 2,
            block_size_kb: 4,
        },
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    };

    let mut storage_engine = StorageEngine::new(config).await.unwrap();
    storage_engine.start().await.unwrap();

    let storage = Arc::new(RwLock::new(storage_engine));
    let service = ProximaDbGrpcService::new(storage);

    // Test health check
    let health_request = Request::new(HealthRequest {});
    let health_response = service.health(health_request).await.unwrap();

    assert_eq!(health_response.get_ref().status, "healthy");
    assert!(health_response.get_ref().timestamp.is_some());

    println!("gRPC health check test passed!");
}

#[tokio::test]
async fn test_grpc_collection_management() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = StorageConfig {
        data_dirs: vec![data_dir.clone()],
        wal_dir: wal_dir.clone(),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 1,
            level_count: 3,
            compaction_threshold: 2,
            block_size_kb: 4,
        },
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    };

    let mut storage_engine = StorageEngine::new(config).await.unwrap();
    storage_engine.start().await.unwrap();

    let storage = Arc::new(RwLock::new(storage_engine));
    let service = ProximaDbGrpcService::new(storage);

    // Test collection creation
    let create_request = Request::new(CreateCollectionRequest {
        collection_id: "test_grpc_collection".to_string(),
        name: "Test gRPC Collection".to_string(),
        dimension: 128,
        schema_type: SchemaType::Document as i32,
        schema: None,
    });

    let create_response = service.create_collection(create_request).await.unwrap();
    assert!(create_response.get_ref().success);

    // Test list collections
    let list_request = Request::new(ListCollectionsRequest {});
    let list_response = service.list_collections(list_request).await.unwrap();

    assert_eq!(list_response.get_ref().collections.len(), 1);
    assert_eq!(
        list_response.get_ref().collections[0].id,
        "test_grpc_collection"
    );
    assert_eq!(list_response.get_ref().collections[0].dimension, 128);

    println!("gRPC collection management test passed!");
}

#[tokio::test]
async fn test_grpc_vector_operations() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = StorageConfig {
        data_dirs: vec![data_dir.clone()],
        wal_dir: wal_dir.clone(),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 1,
            level_count: 3,
            compaction_threshold: 2,
            block_size_kb: 4,
        },
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    };

    let mut storage_engine = StorageEngine::new(config).await.unwrap();
    storage_engine.start().await.unwrap();

    let storage = Arc::new(RwLock::new(storage_engine));
    let service = ProximaDbGrpcService::new(storage);

    let collection_id = "test_vector_collection".to_string();

    // Create collection first
    let create_request = Request::new(CreateCollectionRequest {
        collection_id: collection_id.clone(),
        name: "Test Vector Collection".to_string(),
        dimension: 4,
        schema_type: SchemaType::Document as i32,
        schema: None,
    });

    let _create_response = service.create_collection(create_request).await.unwrap();

    // Test vector insertion
    let vector_record = VectorRecord {
        id: uuid::Uuid::new_v4().to_string(),
        collection_id: collection_id.clone(),
        vector: vec![1.0, 2.0, 3.0, 4.0],
        metadata: None,
        timestamp: Some(prost_types::Timestamp {
            seconds: chrono::Utc::now().timestamp(),
            nanos: 0,
        }),
    };

    let insert_request = Request::new(InsertRequest {
        collection_id: collection_id.clone(),
        record: Some(vector_record.clone()),
    });

    let insert_response = service.insert(insert_request).await.unwrap();
    assert!(insert_response.get_ref().success);
    let vector_id = insert_response.get_ref().vector_id.clone();

    // Test vector retrieval
    let get_request = Request::new(GetRequest {
        collection_id: collection_id.clone(),
        vector_id: vector_id.clone(),
    });

    let get_response = service.get(get_request).await.unwrap();
    assert!(get_response.get_ref().record.is_some());

    let retrieved_record = get_response.get_ref().record.as_ref().unwrap();
    assert_eq!(retrieved_record.vector, vec![1.0, 2.0, 3.0, 4.0]);

    // Test vector search
    let search_request = Request::new(SearchRequest {
        collection_id: collection_id.clone(),
        vector: vec![1.1, 2.1, 3.1, 4.1], // Similar vector
        k: 5,
        filters: None,
        threshold: None,
    });

    let search_response = service.search(search_request).await.unwrap();
    assert!(search_response.get_ref().results.len() >= 1);
    assert!(search_response.get_ref().results[0].score > 0.0);

    println!("gRPC vector operations test passed!");
}

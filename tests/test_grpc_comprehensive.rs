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

//! Comprehensive gRPC Integration Tests for ProximaDB
//!
//! Tests all implemented gRPC methods including error cases and edge scenarios.
//! Achieves comprehensive coverage of the gRPC API surface.

use proximadb::core::{LsmConfig, StorageConfig};
use proximadb::network::grpc::service::ProximaDbGrpcService;
use proximadb::proto::vectordb::v1::vector_db_server::{VectorDb, VectorDbServer};
use proximadb::proto::vectordb::v1::*;
use proximadb::storage::StorageEngine;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;
use tonic::{transport::Server, Request, Status};
use uuid::Uuid;

/// Test helper to create a fresh gRPC service instance
async fn create_test_service() -> (TempDir, ProximaDbGrpcService) {
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

    (temp_dir, service)
}

/// Test helper to create a test collection
async fn create_test_collection(
    service: &ProximaDbGrpcService,
    collection_id: &str,
    dimension: i32,
) -> Result<(), Status> {
    let create_request = Request::new(CreateCollectionRequest {
        collection_id: collection_id.to_string(),
        name: format!("Test Collection {}", collection_id),
        dimension,
        schema_type: SchemaType::Document as i32,
        schema: None,
    });

    let response = service.create_collection(create_request).await?;
    assert!(response.get_ref().success);
    Ok(())
}

/// Test helper to insert a test vector
async fn insert_test_vector(
    service: &ProximaDbGrpcService,
    collection_id: &str,
    vector_id: &str,
    vector: Vec<f32>,
) -> Result<String, Status> {
    let vector_record = VectorRecord {
        id: vector_id.to_string(),
        collection_id: collection_id.to_string(),
        vector,
        metadata: None,
        timestamp: Some(prost_types::Timestamp {
            seconds: chrono::Utc::now().timestamp(),
            nanos: 0,
        }),
    };

    let insert_request = Request::new(InsertVectorRequest {
        collection_identifier: Some(insert_vector_request::CollectionIdentifier::CollectionId(
            collection_id.to_string(),
        )),
        record: Some(vector_record),
    });

    let response = service.insert_vector(insert_request).await?;
    assert!(response.get_ref().success);
    Ok(response.get_ref().vector_id.clone())
}

#[tokio::test]
async fn test_health_endpoints() {
    let (_temp_dir, service) = create_test_service().await;

    // Test health check
    let health_request = Request::new(HealthRequest {});
    let health_response = service.health(health_request).await.unwrap();

    assert_eq!(health_response.get_ref().status, "healthy");
    assert!(health_response.get_ref().timestamp.is_some());

    // Test readiness check
    let readiness_request = Request::new(ReadinessRequest {});
    let readiness_response = service.readiness(readiness_request).await.unwrap();

    assert_eq!(readiness_response.get_ref().status, "ready");

    // Test liveness check
    let liveness_request = Request::new(LivenessRequest {});
    let liveness_response = service.liveness(liveness_request).await.unwrap();

    assert_eq!(liveness_response.get_ref().status, "alive");

    // Test status endpoint
    let status_request = Request::new(StatusRequest {});
    let status_response = service.status(status_request).await.unwrap();

    assert!(!status_response.get_ref().version.is_empty());

    println!("✅ All health endpoints passed!");
}

#[tokio::test]
async fn test_collection_management_comprehensive() {
    let (_temp_dir, service) = create_test_service().await;

    let collection_id = "test_collection_mgmt";
    let collection_name = "Test Collection Management";

    // Test collection creation
    create_test_collection(&service, collection_id, 128)
        .await
        .unwrap();

    // Test get collection by ID
    let get_by_id_request = Request::new(GetCollectionRequest {
        identifier: Some(get_collection_request::Identifier::CollectionId(
            collection_id.to_string(),
        )),
    });

    let get_by_id_response = service.get_collection(get_by_id_request).await.unwrap();
    let collection = get_by_id_response.get_ref().collection.as_ref().unwrap();
    assert_eq!(collection.id, collection_id);
    assert_eq!(collection.dimension, 128);

    // Test get collection by name
    let get_by_name_request = Request::new(GetCollectionRequest {
        identifier: Some(get_collection_request::Identifier::CollectionName(
            collection_name.to_string(),
        )),
    });

    let get_by_name_response = service.get_collection(get_by_name_request).await.unwrap();
    let collection_by_name = get_by_name_response.get_ref().collection.as_ref().unwrap();
    assert_eq!(collection_by_name.id, collection_id);

    // Test list collections
    let list_request = Request::new(ListCollectionsRequest {});
    let list_response = service.list_collections(list_request).await.unwrap();

    assert_eq!(list_response.get_ref().collections.len(), 1);
    assert_eq!(list_response.get_ref().collections[0].id, collection_id);

    // Test list collection IDs
    let list_ids_request = Request::new(ListCollectionIdsRequest {});
    let list_ids_response = service.list_collection_ids(list_ids_request).await.unwrap();

    assert_eq!(list_ids_response.get_ref().collection_ids.len(), 1);
    assert_eq!(list_ids_response.get_ref().collection_ids[0], collection_id);

    // Test list collection names
    let list_names_request = Request::new(ListCollectionNamesRequest {});
    let list_names_response = service
        .list_collection_names(list_names_request)
        .await
        .unwrap();

    assert_eq!(list_names_response.get_ref().collection_names.len(), 1);
    assert_eq!(
        list_names_response.get_ref().collection_names[0],
        collection_name
    );

    // Test get collection ID by name
    let get_id_by_name_request = Request::new(GetCollectionIdByNameRequest {
        collection_name: collection_name.to_string(),
    });

    let get_id_by_name_response = service
        .get_collection_id_by_name(get_id_by_name_request)
        .await
        .unwrap();
    assert_eq!(
        get_id_by_name_response.get_ref().collection_id,
        collection_id
    );

    // Test get collection name by ID
    let get_name_by_id_request = Request::new(GetCollectionNameByIdRequest {
        collection_id: collection_id.to_string(),
    });

    let get_name_by_id_response = service
        .get_collection_name_by_id(get_name_by_id_request)
        .await
        .unwrap();
    assert_eq!(
        get_name_by_id_response.get_ref().collection_name,
        collection_name
    );

    println!("✅ Collection management comprehensive tests passed!");
}

#[tokio::test]
async fn test_vector_operations_comprehensive() {
    let (_temp_dir, service) = create_test_service().await;

    let collection_id = "test_vector_ops";
    let test_vector = vec![1.0, 2.0, 3.0, 4.0];
    let client_id = "test_client_vector_123";

    // Create collection first
    create_test_collection(&service, collection_id, 4)
        .await
        .unwrap();

    // Test vector insertion
    let vector_id = insert_test_vector(&service, collection_id, client_id, test_vector.clone())
        .await
        .unwrap();

    // Test get vector by ID
    let get_by_id_request = Request::new(GetVectorRequest {
        collection_identifier: Some(get_vector_request::CollectionIdentifier::CollectionId(
            collection_id.to_string(),
        )),
        vector_id: vector_id.clone(),
    });

    let get_by_id_response = service.get_vector(get_by_id_request).await.unwrap();
    let retrieved_record = get_by_id_response.get_ref().record.as_ref().unwrap();
    assert_eq!(retrieved_record.vector, test_vector);
    assert_eq!(retrieved_record.id, client_id);

    // Test get vector by client ID
    let get_by_client_id_request = Request::new(GetVectorByClientIdRequest {
        collection_identifier: Some(
            get_vector_by_client_id_request::CollectionIdentifier::CollectionId(
                collection_id.to_string(),
            ),
        ),
        client_id: client_id.to_string(),
    });

    let get_by_client_id_response = service
        .get_vector_by_client_id(get_by_client_id_request)
        .await
        .unwrap();
    let retrieved_by_client_id = get_by_client_id_response.get_ref().record.as_ref().unwrap();
    assert_eq!(retrieved_by_client_id.vector, test_vector);
    assert_eq!(retrieved_by_client_id.id, client_id);

    // Test vector search
    let search_request = Request::new(SearchRequest {
        collection_id: collection_id.to_string(),
        vector: vec![1.1, 2.1, 3.1, 4.1], // Similar vector
        k: 5,
        filters: None,
        threshold: None,
    });

    let search_response = service.search(search_request).await.unwrap();
    assert!(search_response.get_ref().results.len() >= 1);
    assert!(search_response.get_ref().results[0].score > 0.0);

    // Test vector deletion
    let delete_request = Request::new(DeleteVectorRequest {
        collection_identifier: Some(delete_vector_request::CollectionIdentifier::CollectionId(
            collection_id.to_string(),
        )),
        vector_id: vector_id.clone(),
    });

    let delete_response = service.delete_vector(delete_request).await.unwrap();
    assert!(delete_response.get_ref().success);
    assert_eq!(delete_response.get_ref().deleted_count, 1);

    // Verify vector is deleted - should fail to retrieve
    let get_deleted_request = Request::new(GetVectorRequest {
        collection_identifier: Some(get_vector_request::CollectionIdentifier::CollectionId(
            collection_id.to_string(),
        )),
        vector_id: vector_id.clone(),
    });

    let get_deleted_result = service.get_vector(get_deleted_request).await;
    assert!(get_deleted_result.is_err() || get_deleted_result.unwrap().get_ref().record.is_none());

    println!("✅ Vector operations comprehensive tests passed!");
}

#[tokio::test]
async fn test_collection_deletion() {
    let (_temp_dir, service) = create_test_service().await;

    let collection_id = "test_collection_delete";

    // Create collection
    create_test_collection(&service, collection_id, 64)
        .await
        .unwrap();

    // Verify collection exists
    let list_before_request = Request::new(ListCollectionsRequest {});
    let list_before_response = service.list_collections(list_before_request).await.unwrap();
    assert_eq!(list_before_response.get_ref().collections.len(), 1);

    // Delete collection by ID
    let delete_request = Request::new(DeleteCollectionRequest {
        identifier: Some(delete_collection_request::Identifier::CollectionId(
            collection_id.to_string(),
        )),
    });

    let delete_response = service.delete_collection(delete_request).await.unwrap();
    assert!(delete_response.get_ref().success);

    // Verify collection is deleted
    let list_after_request = Request::new(ListCollectionsRequest {});
    let list_after_response = service.list_collections(list_after_request).await.unwrap();
    assert_eq!(list_after_response.get_ref().collections.len(), 0);

    println!("✅ Collection deletion test passed!");
}

#[tokio::test]
async fn test_error_handling() {
    let (_temp_dir, service) = create_test_service().await;

    // Test get non-existent collection
    let get_nonexistent_request = Request::new(GetCollectionRequest {
        identifier: Some(get_collection_request::Identifier::CollectionId(
            "nonexistent_collection".to_string(),
        )),
    });

    let get_nonexistent_result = service.get_collection(get_nonexistent_request).await;
    assert!(get_nonexistent_result.is_err());

    // Test get vector from non-existent collection
    let get_vector_invalid_request = Request::new(GetVectorRequest {
        collection_identifier: Some(get_vector_request::CollectionIdentifier::CollectionId(
            "nonexistent_collection".to_string(),
        )),
        vector_id: "any_id".to_string(),
    });

    let get_vector_invalid_result = service.get_vector(get_vector_invalid_request).await;
    assert!(get_vector_invalid_result.is_err());

    // Test delete non-existent collection
    let delete_nonexistent_request = Request::new(DeleteCollectionRequest {
        identifier: Some(delete_collection_request::Identifier::CollectionId(
            "nonexistent_collection".to_string(),
        )),
    });

    let delete_nonexistent_result = service.delete_collection(delete_nonexistent_request).await;
    assert!(delete_nonexistent_result.is_err());

    // Test search in non-existent collection
    let search_invalid_request = Request::new(SearchRequest {
        collection_id: "nonexistent_collection".to_string(),
        vector: vec![1.0, 2.0, 3.0],
        k: 5,
        filters: None,
        threshold: None,
    });

    let search_invalid_result = service.search(search_invalid_request).await;
    assert!(search_invalid_result.is_err());

    println!("✅ Error handling tests passed!");
}

#[tokio::test]
async fn test_edge_cases() {
    let (_temp_dir, service) = create_test_service().await;

    let collection_id = "test_edge_cases";

    // Create collection with large dimension
    create_test_collection(&service, collection_id, 1536)
        .await
        .unwrap(); // OpenAI embedding size

    // Test with large vector
    let large_vector: Vec<f32> = (0..1536).map(|i| (i as f32) * 0.001).collect();
    let vector_id = insert_test_vector(
        &service,
        collection_id,
        "large_vector",
        large_vector.clone(),
    )
    .await
    .unwrap();

    // Test retrieval of large vector
    let get_large_request = Request::new(GetVectorRequest {
        collection_identifier: Some(get_vector_request::CollectionIdentifier::CollectionId(
            collection_id.to_string(),
        )),
        vector_id: vector_id.clone(),
    });

    let get_large_response = service.get_vector(get_large_request).await.unwrap();
    let retrieved_large = get_large_response.get_ref().record.as_ref().unwrap();
    assert_eq!(retrieved_large.vector.len(), 1536);
    assert_eq!(retrieved_large.vector, large_vector);

    // Test search with large vector
    let search_large_request = Request::new(SearchRequest {
        collection_id: collection_id.to_string(),
        vector: large_vector.clone(),
        k: 1,
        filters: None,
        threshold: None,
    });

    let search_large_response = service.search(search_large_request).await.unwrap();
    assert_eq!(search_large_response.get_ref().results.len(), 1);

    // Test with zero vector
    let zero_vector = vec![0.0; 1536];
    let zero_vector_id =
        insert_test_vector(&service, collection_id, "zero_vector", zero_vector.clone())
            .await
            .unwrap();

    let get_zero_request = Request::new(GetVectorRequest {
        collection_identifier: Some(get_vector_request::CollectionIdentifier::CollectionId(
            collection_id.to_string(),
        )),
        vector_id: zero_vector_id,
    });

    let get_zero_response = service.get_vector(get_zero_request).await.unwrap();
    let retrieved_zero = get_zero_response.get_ref().record.as_ref().unwrap();
    assert_eq!(retrieved_zero.vector, zero_vector);

    println!("✅ Edge cases tests passed!");
}

#[tokio::test]
async fn test_unimplemented_methods() {
    let (_temp_dir, service) = create_test_service().await;

    let collection_id = "test_unimplemented";
    create_test_collection(&service, collection_id, 4)
        .await
        .unwrap();

    // Test update vector (should return unimplemented)
    let update_request = Request::new(UpdateVectorRequest {
        collection_identifier: Some(update_vector_request::CollectionIdentifier::CollectionId(
            collection_id.to_string(),
        )),
        vector_id: "test_id".to_string(),
        record: None,
    });

    let update_result = service.update_vector(update_request).await;
    assert!(update_result.is_err());
    match update_result.err().unwrap().code() {
        tonic::Code::Unimplemented => {} // Expected
        other => panic!("Expected Unimplemented, got {:?}", other),
    }

    // Test batch insert (should return unimplemented)
    let batch_insert_request = Request::new(BatchInsertRequest {
        collection_identifier: Some(batch_insert_request::CollectionIdentifier::CollectionId(
            collection_id.to_string(),
        )),
        records: vec![],
    });

    let batch_insert_result = service.batch_insert(batch_insert_request).await;
    assert!(batch_insert_result.is_err());
    match batch_insert_result.err().unwrap().code() {
        tonic::Code::Unimplemented => {} // Expected
        other => panic!("Expected Unimplemented, got {:?}", other),
    }

    // Test get index stats (should return unimplemented)
    let index_stats_request = Request::new(GetIndexStatsRequest {
        collection_id: collection_id.to_string(),
    });

    let index_stats_result = service.get_index_stats(index_stats_request).await;
    assert!(index_stats_result.is_err());
    match index_stats_result.err().unwrap().code() {
        tonic::Code::Unimplemented => {} // Expected
        other => panic!("Expected Unimplemented, got {:?}", other),
    }

    println!("✅ Unimplemented methods tests passed!");
}

#[tokio::test]
async fn test_multiple_collections() {
    let (_temp_dir, service) = create_test_service().await;

    // Create multiple collections with different dimensions
    let collections = vec![
        ("collection_1", 128),
        ("collection_2", 256),
        ("collection_3", 768),
        ("collection_4", 1536),
    ];

    for (id, dim) in &collections {
        create_test_collection(&service, id, *dim).await.unwrap();
    }

    // Test list collections returns all
    let list_request = Request::new(ListCollectionsRequest {});
    let list_response = service.list_collections(list_request).await.unwrap();
    assert_eq!(list_response.get_ref().collections.len(), collections.len());

    // Test list collection IDs
    let list_ids_request = Request::new(ListCollectionIdsRequest {});
    let list_ids_response = service.list_collection_ids(list_ids_request).await.unwrap();
    assert_eq!(
        list_ids_response.get_ref().collection_ids.len(),
        collections.len()
    );

    // Verify each collection exists and has correct dimension
    for (id, expected_dim) in &collections {
        let get_request = Request::new(GetCollectionRequest {
            identifier: Some(get_collection_request::Identifier::CollectionId(
                id.to_string(),
            )),
        });

        let get_response = service.get_collection(get_request).await.unwrap();
        let collection = get_response.get_ref().collection.as_ref().unwrap();
        assert_eq!(collection.id, *id);
        assert_eq!(collection.dimension, *expected_dim);
    }

    // Add vectors to each collection and test cross-collection isolation
    for (id, dim) in &collections {
        let test_vector: Vec<f32> = (0..*dim).map(|i| i as f32 / *dim as f32).collect();
        insert_test_vector(&service, id, &format!("vector_{}", id), test_vector)
            .await
            .unwrap();
    }

    // Test search in each collection
    for (id, dim) in &collections {
        let query_vector: Vec<f32> = (0..*dim).map(|i| (i as f32 + 0.1) / *dim as f32).collect();

        let search_request = Request::new(SearchRequest {
            collection_id: id.to_string(),
            vector: query_vector,
            k: 1,
            filters: None,
            threshold: None,
        });

        let search_response = service.search(search_request).await.unwrap();
        assert_eq!(search_response.get_ref().results.len(), 1);
    }

    println!("✅ Multiple collections tests passed!");
}

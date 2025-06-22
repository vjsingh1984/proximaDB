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

//! Unit tests for metadata memory indexes functionality

use proximadb::storage::metadata::indexes::MetadataMemoryIndexes;
use proximadb::storage::metadata::backends::filestore_backend::CollectionRecord;

#[tokio::test]
async fn test_uuid_lookup_performance() {
    let indexes = MetadataMemoryIndexes::new();
    
    let record = CollectionRecord {
        uuid: "test-uuid-123".to_string(),
        name: "test-collection".to_string(),
        dimension: 128,
        distance_metric: "cosine".to_string(),
        indexing_algorithm: "hnsw".to_string(),
        storage_engine: "viper".to_string(),
        created_at: 1000,
        updated_at: 1000,
        version: 1,
        vector_count: 100,
        total_size_bytes: 1024,
        config: "{}".to_string(),
        description: None,
        tags: vec!["test".to_string()],
        owner: None,
    };
    
    indexes.upsert_collection(record.clone()).await;
    
    let result = indexes.get_by_uuid("test-uuid-123").await;
    assert!(result.is_some());
    assert_eq!(result.unwrap().name, "test-collection");
    
    let stats = indexes.get_statistics().await;
    assert_eq!(stats.total_collections, 1);
    assert_eq!(stats.uuid_index_hits, 1);
}

#[tokio::test]
async fn test_name_lookup_performance() {
    let indexes = MetadataMemoryIndexes::new();
    
    let record = CollectionRecord {
        uuid: "test-uuid-456".to_string(),
        name: "another-collection".to_string(),
        dimension: 256,
        distance_metric: "euclidean".to_string(),
        indexing_algorithm: "ivf".to_string(),
        storage_engine: "lsm".to_string(),
        created_at: 2000,
        updated_at: 2000,
        version: 1,
        vector_count: 200,
        total_size_bytes: 2048,
        config: "{}".to_string(),
        description: None,
        tags: vec!["production".to_string()],
        owner: None,
    };
    
    indexes.upsert_collection(record.clone()).await;
    
    let result = indexes.get_by_name("another-collection").await;
    assert!(result.is_some());
    assert_eq!(result.unwrap().uuid, "test-uuid-456");
    
    let uuid = indexes.get_uuid_by_name("another-collection").await;
    assert_eq!(uuid.unwrap(), "test-uuid-456");
}

#[tokio::test]
async fn test_prefix_search() {
    let indexes = MetadataMemoryIndexes::new();
    
    let records = vec![
        CollectionRecord {
            uuid: "uuid-1".to_string(),
            name: "user_data_v1".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            storage_engine: "viper".to_string(),
            created_at: 1000,
            updated_at: 1000,
            version: 1,
            vector_count: 100,
            total_size_bytes: 1024,
            config: "{}".to_string(),
            description: None,
            tags: vec![],
            owner: None,
        },
        CollectionRecord {
            uuid: "uuid-2".to_string(),
            name: "user_data_v2".to_string(),
            dimension: 256,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            storage_engine: "viper".to_string(),
            created_at: 2000,
            updated_at: 2000,
            version: 1,
            vector_count: 200,
            total_size_bytes: 2048,
            config: "{}".to_string(),
            description: None,
            tags: vec![],
            owner: None,
        },
        CollectionRecord {
            uuid: "uuid-3".to_string(),
            name: "system_logs".to_string(),
            dimension: 64,
            distance_metric: "euclidean".to_string(),
            indexing_algorithm: "flat".to_string(),
            storage_engine: "lsm".to_string(),
            created_at: 3000,
            updated_at: 3000,
            version: 1,
            vector_count: 50,
            total_size_bytes: 512,
            config: "{}".to_string(),
            description: None,
            tags: vec![],
            owner: None,
        },
    ];
    
    for record in records {
        indexes.upsert_collection(record).await;
    }
    
    let results = indexes.find_by_name_prefix("user_").await;
    assert!(results.len() >= 2); // Should find both user_data_v1 and user_data_v2
    
    let results = indexes.find_by_name_prefix("user_data").await;
    assert!(results.len() >= 2); // Should find both user_data_v1 and user_data_v2
    
    let results = indexes.find_by_name_prefix("system").await;
    assert!(results.len() >= 1); // Should find system_logs
    if !results.is_empty() {
        assert_eq!(results[0].name, "system_logs");
    }
}

#[tokio::test]
async fn test_tag_search() {
    let indexes = MetadataMemoryIndexes::new();
    
    let record1 = CollectionRecord {
        uuid: "uuid-1".to_string(),
        name: "collection1".to_string(),
        dimension: 128,
        distance_metric: "cosine".to_string(),
        indexing_algorithm: "hnsw".to_string(),
        storage_engine: "viper".to_string(),
        created_at: 1000,
        updated_at: 1000,
        version: 1,
        vector_count: 100,
        total_size_bytes: 1024,
        config: "{}".to_string(),
        description: None,
        tags: vec!["production".to_string(), "ml".to_string()],
        owner: None,
    };
    
    let record2 = CollectionRecord {
        uuid: "uuid-2".to_string(),
        name: "collection2".to_string(),
        dimension: 256,
        distance_metric: "euclidean".to_string(),
        indexing_algorithm: "ivf".to_string(),
        storage_engine: "lsm".to_string(),
        created_at: 2000,
        updated_at: 2000,
        version: 1,
        vector_count: 200,
        total_size_bytes: 2048,
        config: "{}".to_string(),
        description: None,
        tags: vec!["testing".to_string(), "ml".to_string()],
        owner: None,
    };
    
    indexes.upsert_collection(record1).await;
    indexes.upsert_collection(record2).await;
    
    let ml_results = indexes.find_by_tag("ml").await;
    assert_eq!(ml_results.len(), 2);
    
    let prod_results = indexes.find_by_tag("production").await;
    assert_eq!(prod_results.len(), 1);
    if !prod_results.is_empty() {
        assert_eq!(prod_results[0].name, "collection1");
    }
}

#[tokio::test]
async fn test_size_range_search() {
    let indexes = MetadataMemoryIndexes::new();
    
    let records = vec![
        CollectionRecord {
            uuid: "small-uuid".to_string(),
            name: "small_collection".to_string(),
            dimension: 64,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "flat".to_string(),
            storage_engine: "viper".to_string(),
            created_at: 1000,
            updated_at: 1000,
            version: 1,
            vector_count: 10,
            total_size_bytes: 100,
            config: "{}".to_string(),
            description: None,
            tags: vec![],
            owner: None,
        },
        CollectionRecord {
            uuid: "medium-uuid".to_string(),
            name: "medium_collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            storage_engine: "viper".to_string(),
            created_at: 2000,
            updated_at: 2000,
            version: 1,
            vector_count: 100,
            total_size_bytes: 1000,
            config: "{}".to_string(),
            description: None,
            tags: vec![],
            owner: None,
        },
        CollectionRecord {
            uuid: "large-uuid".to_string(),
            name: "large_collection".to_string(),
            dimension: 512,
            distance_metric: "euclidean".to_string(),
            indexing_algorithm: "ivf".to_string(),
            storage_engine: "lsm".to_string(),
            created_at: 3000,
            updated_at: 3000,
            version: 1,
            vector_count: 1000,
            total_size_bytes: 10000,
            config: "{}".to_string(),
            description: None,
            tags: vec![],
            owner: None,
        },
    ];
    
    for record in records {
        indexes.upsert_collection(record).await;
    }
    
    let small_results = indexes.find_by_size_range(0, 500).await;
    assert_eq!(small_results.len(), 1);
    if !small_results.is_empty() {
        assert_eq!(small_results[0].name, "small_collection");
    }
    
    let medium_large_results = indexes.find_by_size_range(500, 20000).await;
    assert_eq!(medium_large_results.len(), 2);
}

#[tokio::test]
async fn test_time_range_search() {
    let indexes = MetadataMemoryIndexes::new();
    
    let records = vec![
        CollectionRecord {
            uuid: "old-uuid".to_string(),
            name: "old_collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            storage_engine: "viper".to_string(),
            created_at: 1000,
            updated_at: 1000,
            version: 1,
            vector_count: 100,
            total_size_bytes: 1024,
            config: "{}".to_string(),
            description: None,
            tags: vec![],
            owner: None,
        },
        CollectionRecord {
            uuid: "new-uuid".to_string(),
            name: "new_collection".to_string(),
            dimension: 256,
            distance_metric: "euclidean".to_string(),
            indexing_algorithm: "ivf".to_string(),
            storage_engine: "lsm".to_string(),
            created_at: 5000,
            updated_at: 5000,
            version: 1,
            vector_count: 200,
            total_size_bytes: 2048,
            config: "{}".to_string(),
            description: None,
            tags: vec![],
            owner: None,
        },
    ];
    
    for record in records {
        indexes.upsert_collection(record).await;
    }
    
    let old_results = indexes.find_by_creation_time_range(0, 2000).await;
    assert_eq!(old_results.len(), 1);
    if !old_results.is_empty() {
        assert_eq!(old_results[0].name, "old_collection");
    }
    
    let new_results = indexes.find_by_creation_time_range(4000, 6000).await;
    assert_eq!(new_results.len(), 1);
    if !new_results.is_empty() {
        assert_eq!(new_results[0].name, "new_collection");
    }
    
    let all_results = indexes.find_by_creation_time_range(0, 10000).await;
    assert_eq!(all_results.len(), 2);
}

#[tokio::test]
async fn test_remove_collection() {
    let indexes = MetadataMemoryIndexes::new();
    
    let record = CollectionRecord {
        uuid: "remove-test-uuid".to_string(),
        name: "remove_test".to_string(),
        dimension: 128,
        distance_metric: "cosine".to_string(),
        indexing_algorithm: "hnsw".to_string(),
        storage_engine: "viper".to_string(),
        created_at: 1000,
        updated_at: 1000,
        version: 1,
        vector_count: 100,
        total_size_bytes: 1024,
        config: "{}".to_string(),
        description: None,
        tags: vec!["to_remove".to_string()],
        owner: None,
    };
    
    indexes.upsert_collection(record).await;
    
    // Verify it exists
    assert!(indexes.get_by_uuid("remove-test-uuid").await.is_some());
    assert!(indexes.get_by_name("remove_test").await.is_some());
    
    // Remove it
    indexes.remove_collection("remove-test-uuid").await;
    
    // Verify it's gone
    assert!(indexes.get_by_uuid("remove-test-uuid").await.is_none());
    assert!(indexes.get_by_name("remove_test").await.is_none());
    
    let tag_results = indexes.find_by_tag("to_remove").await;
    assert_eq!(tag_results.len(), 0);
}

#[tokio::test]
async fn test_clear_and_rebuild() {
    let indexes = MetadataMemoryIndexes::new();
    
    let records = vec![
        CollectionRecord {
            uuid: "rebuild-1".to_string(),
            name: "rebuild_collection_1".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            storage_engine: "viper".to_string(),
            created_at: 1000,
            updated_at: 1000,
            version: 1,
            vector_count: 100,
            total_size_bytes: 1024,
            config: "{}".to_string(),
            description: None,
            tags: vec!["rebuild".to_string()],
            owner: None,
        },
        CollectionRecord {
            uuid: "rebuild-2".to_string(),
            name: "rebuild_collection_2".to_string(),
            dimension: 256,
            distance_metric: "euclidean".to_string(),
            indexing_algorithm: "ivf".to_string(),
            storage_engine: "lsm".to_string(),
            created_at: 2000,
            updated_at: 2000,
            version: 1,
            vector_count: 200,
            total_size_bytes: 2048,
            config: "{}".to_string(),
            description: None,
            tags: vec!["rebuild".to_string()],
            owner: None,
        },
    ];
    
    // Add initial records
    for record in &records {
        indexes.upsert_collection(record.clone()).await;
    }
    
    // Verify they exist
    assert_eq!(indexes.list_all().await.len(), 2);
    
    // Clear indexes
    indexes.clear().await;
    assert_eq!(indexes.list_all().await.len(), 0);
    
    // Rebuild from records
    indexes.rebuild_from_records(records).await;
    assert_eq!(indexes.list_all().await.len(), 2);
    
    // Verify functionality after rebuild
    assert!(indexes.get_by_uuid("rebuild-1").await.is_some());
    assert!(indexes.get_by_name("rebuild_collection_2").await.is_some());
    
    let tag_results = indexes.find_by_tag("rebuild").await; 
    assert_eq!(tag_results.len(), 2);
}
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

use proximadb::proto::proximadb_v1::{
    Collection, CollectionConfig, CollectionStats, DistanceMetric, StorageEngine,
};
use proximadb::storage::metadata::indexes::MetadataMemoryIndexes;

fn create_test_collection(id: &str, name: &str) -> Collection {
    let temp_dir = tempfile::tempdir().unwrap();
    Collection {
        id: id.to_string(),
        config: Some(CollectionConfig {
            name: name.to_string(),
            dimension: 128,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            storage_engine: Some(StorageEngine::Viper as i32),
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            primary_index: Some("HNSW".to_string()),
            auto_index_selection: Some(false),
            description: Some("Test collection".to_string()),
            tags: vec![],
            owner: Some("test_user".to_string()),
            embedding_models: vec![],
            storage_config: None,
        }),
        stats: Some(CollectionStats {
            vector_count: 100,
            index_size_bytes: 1024,
            data_size_bytes: 2048,
        }),
        created_at: 1000,
        updated_at: 1000,
        storage_assignment: Some(proximadb::proto::proximadb_v1::StorageAssignment {
            primary_path: format!("{}", temp_dir.path().display()),
            backup_paths: vec![],
            engine: StorageEngine::Viper as i32,
            engine_config: std::collections::HashMap::new(),
            base_location: format!("{}", temp_dir.path().display()),
            assigned_at: chrono::Utc::now().timestamp_micros(),
        }),
    }
}

#[tokio::test]
async fn test_uuid_lookup_performance() {
    let indexes = MetadataMemoryIndexes::new();

    let collection = create_test_collection("test-uuid-123", "test-collection");

    indexes.upsert_collection(collection.clone()).await;

    let result = indexes.get_by_uuid("test-uuid-123").await;
    assert!(result.is_some());
    assert_eq!(result.unwrap().id, "test-uuid-123");

    let stats = indexes.get_statistics().await;
    assert_eq!(stats.total_collections, 1);
    assert_eq!(stats.uuid_index_hits, 1);
}

#[tokio::test]
async fn test_name_lookup_performance() {
    let indexes = MetadataMemoryIndexes::new();

    let collection = create_test_collection("test-uuid-456", "another-collection");

    indexes.upsert_collection(collection.clone()).await;

    let result = indexes.get_by_name("another-collection").await;
    assert!(result.is_some());
    assert_eq!(result.unwrap().id, "test-uuid-456");

    let uuid = indexes.get_uuid_by_name("another-collection").await;
    assert_eq!(uuid.unwrap(), "test-uuid-456");
}

#[tokio::test]
async fn test_prefix_search() {
    let indexes = MetadataMemoryIndexes::new();

    let collections = vec![
        create_test_collection("uuid-1", "user_data_v1"),
        create_test_collection("uuid-2", "user_data_v2"),
        create_test_collection("uuid-3", "user_logs_v1"),
        create_test_collection("uuid-4", "system_config"),
    ];

    for collection in collections {
        indexes.upsert_collection(collection).await;
    }

    // Use find_by_name_prefix method
    let user_results = indexes.find_by_name_prefix("user_").await;
    assert_eq!(user_results.len(), 3);

    let data_results = indexes.find_by_name_prefix("user_data").await;
    assert_eq!(data_results.len(), 2);

    let system_results = indexes.find_by_name_prefix("system").await;
    assert_eq!(system_results.len(), 1);

    let nonexistent_results = indexes.find_by_name_prefix("nonexistent").await;
    assert_eq!(nonexistent_results.len(), 0);
}

#[tokio::test]
async fn test_concurrent_operations() {
    use std::sync::Arc;
    use tokio::task::JoinSet;

    let indexes = Arc::new(MetadataMemoryIndexes::new());
    let mut tasks = JoinSet::new();

    // Spawn concurrent upsert tasks
    for i in 0..10 {
        let indexes_clone = indexes.clone();
        tasks.spawn(async move {
            let collection =
                create_test_collection(&format!("uuid-{}", i), &format!("collection-{}", i));
            indexes_clone.upsert_collection(collection).await;
        });
    }

    // Wait for all tasks to complete
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }

    // Verify all collections were added
    let stats = indexes.get_statistics().await;
    assert_eq!(stats.total_collections, 10);

    // Test concurrent reads
    let mut read_tasks = JoinSet::new();
    for i in 0..10 {
        let indexes_clone = indexes.clone();
        read_tasks.spawn(async move {
            let result = indexes_clone.get_by_uuid(&format!("uuid-{}", i)).await;
            assert!(result.is_some());
            result.unwrap().id.clone()
        });
    }

    while let Some(result) = read_tasks.join_next().await {
        let uuid = result.unwrap();
        assert!(uuid.starts_with("uuid-"));
    }
}

#[tokio::test]
async fn test_rebuild_performance() {
    let indexes = MetadataMemoryIndexes::new();

    // Add initial collections
    for i in 0..100 {
        let collection =
            create_test_collection(&format!("uuid-{:03}", i), &format!("collection-{:03}", i));
        indexes.upsert_collection(collection).await;
    }

    let initial_stats = indexes.get_statistics().await;
    assert_eq!(initial_stats.total_collections, 100);

    // Test rebuild from existing collections
    let collections: Vec<_> = (0..100)
        .map(|i| {
            create_test_collection(
                &format!("new-uuid-{:03}", i),
                &format!("new-collection-{:03}", i),
            )
        })
        .collect();

    // Use rebuild_from_records method
    indexes.rebuild_from_records(collections).await;

    let rebuild_stats = indexes.get_statistics().await;
    assert_eq!(rebuild_stats.total_collections, 100);

    // Verify old collections are replaced
    let old_result = indexes.get_by_uuid("uuid-001").await;
    assert!(old_result.is_none());

    let new_result = indexes.get_by_uuid("new-uuid-001").await;
    assert!(new_result.is_some());
}

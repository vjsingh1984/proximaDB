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

//! Unit tests for unified collection index functionality

use proximadb::proto::proximadb_v1::{
    Collection, CollectionConfig, CollectionStats, DistanceMetric, IndexingAlgorithm, StorageEngine,
};
use proximadb::storage::metadata::unified_index::UnifiedCollectionIndex;

fn create_test_collection(id: &str, name: &str) -> Collection {
    Collection {
        id: id.to_string(),
        config: Some(CollectionConfig {
            name: name.to_string(),
            dimension: 128,
            distance_metric: DistanceMetric::Cosine as i32,
            storage_engine: StorageEngine::Viper as i32,
            filterable_columns: vec![],
            index_configs: vec![],
            quantization: None,
            primary_index: "HNSW".to_string(),
            auto_index_selection: false,
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
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        storage_assignment: None,
    }
}

#[test]
fn test_basic_operations() {
    let index = UnifiedCollectionIndex::new();

    let collection = create_test_collection("uuid-123", "test-collection");

    // Test upsert
    index.upsert_collection(collection.clone());
    assert_eq!(index.count(), 1);

    // Test UUID lookup
    let result = index.get_by_uuid("uuid-123");
    assert!(result.is_some());
    assert_eq!(result.unwrap().id, "uuid-123");

    // Test name lookup
    let result = index.get_by_name("test-collection");
    assert!(result.is_some());
    assert_eq!(result.unwrap().id, "uuid-123");

    // Test UUID by name
    let uuid = index.get_uuid_by_name("test-collection");
    assert_eq!(uuid.unwrap(), "uuid-123");

    // Test existence checks
    assert!(index.exists_by_uuid("uuid-123"));
    assert!(index.exists_by_name("test-collection"));
    assert!(!index.exists_by_uuid("nonexistent"));
    assert!(!index.exists_by_name("nonexistent"));

    // Test removal
    let removed = index.remove_collection("uuid-123");
    assert!(removed.is_some());
    assert_eq!(index.count(), 0);
    assert!(!index.exists_by_uuid("uuid-123"));
    assert!(!index.exists_by_name("test-collection"));
}

#[test]
fn test_concurrent_access() {
    use std::sync::Arc;
    use std::thread;

    let index = Arc::new(UnifiedCollectionIndex::new());
    let mut handles = vec![];

    // Spawn multiple threads doing concurrent operations
    for i in 0..10 {
        let index_clone = index.clone();
        let handle = thread::spawn(move || {
            let collection =
                create_test_collection(&format!("uuid-{}", i), &format!("collection-{}", i));
            index_clone.upsert_collection(collection);

            // Immediate lookup to test consistency
            let result = index_clone.get_by_uuid(&format!("uuid-{}", i));
            assert!(result.is_some());

            let result = index_clone.get_by_name(&format!("collection-{}", i));
            assert!(result.is_some());
        });
        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    // Verify final state
    assert_eq!(index.count(), 10);

    // Test concurrent reads
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let index_clone = index.clone();
            thread::spawn(move || {
                let result = index_clone.get_by_uuid(&format!("uuid-{}", i));
                assert!(result.is_some());
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_rebuild_from_records() {
    let index = UnifiedCollectionIndex::new();

    let records = vec![
        create_test_collection("uuid-1", "collection-1"),
        create_test_collection("uuid-2", "collection-2"),
        create_test_collection("uuid-3", "collection-3"),
    ];

    index.rebuild_from_records(records);

    assert_eq!(index.count(), 3);
    assert!(index.exists_by_uuid("uuid-1"));
    assert!(index.exists_by_name("collection-2"));

    let metrics = index.get_metrics();
    assert!(metrics.last_rebuild_timestamp.is_some());
}

#[test]
fn test_performance_metrics() {
    let index = UnifiedCollectionIndex::new();
    let collection = create_test_collection("uuid-perf", "perf-collection");

    index.upsert_collection(collection);

    // Perform some operations to generate metrics
    index.get_by_uuid("uuid-perf");
    index.get_by_name("perf-collection");
    index.get_by_uuid("nonexistent"); // Cache miss

    let metrics = index.get_metrics();
    assert_eq!(metrics.total_collections, 1);
    assert_eq!(metrics.uuid_lookups, 2);
    assert_eq!(metrics.name_lookups, 1);
    assert_eq!(metrics.cache_hits, 2);
    assert_eq!(metrics.cache_misses, 1);
    assert!(metrics.avg_lookup_time_ns > 0);
}

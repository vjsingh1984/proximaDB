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

//! Unit tests for single collection index functionality

use proximadb::storage::metadata::single_index::SingleCollectionIndex;
use proximadb::storage::metadata::backends::filestore_backend::CollectionRecord;

fn create_test_record(uuid: &str, name: &str) -> CollectionRecord {
    CollectionRecord {
        uuid: uuid.to_string(),
        name: name.to_string(),
        dimension: 128,
        distance_metric: "cosine".to_string(),
        indexing_algorithm: "hnsw".to_string(),
        storage_engine: "viper".to_string(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        version: 1,
        vector_count: 100,
        total_size_bytes: 1024,
        config: "{}".to_string(),
        description: None,
        tags: vec![],
        owner: None,
    }
}

#[test]
fn test_single_index_operations() {
    let index = SingleCollectionIndex::new();
    
    let record = create_test_record("uuid-123", "test-collection");
    
    // Test upsert
    index.upsert_collection(record.clone());
    assert_eq!(index.count(), 1);
    
    // Test UUID lookup - O(1)
    let result = index.get_by_uuid("uuid-123");
    assert!(result.is_some());
    assert_eq!(result.unwrap().name, "test-collection");
    
    // Test name lookup - O(n) but efficient
    let result = index.get_by_name("test-collection");
    assert!(result.is_some());
    assert_eq!(result.unwrap().uuid, "uuid-123");
    
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
fn test_single_index_rebuild() {
    let index = SingleCollectionIndex::new();
    
    let records = vec![
        create_test_record("uuid-1", "collection-1"),
        create_test_record("uuid-2", "collection-2"),
        create_test_record("uuid-3", "collection-3"),
    ];
    
    index.rebuild_from_records(records);
    
    assert_eq!(index.count(), 3);
    assert!(index.exists_by_uuid("uuid-1"));
    assert!(index.exists_by_name("collection-2"));
    
    let metrics = index.get_metrics();
    assert!(metrics.last_rebuild_timestamp.is_some());
    assert_eq!(metrics.total_collections, 3);
}

#[test]
fn test_concurrent_access() {
    use std::sync::Arc;
    use std::thread;
    
    let index = Arc::new(SingleCollectionIndex::new());
    let mut handles = vec![];
    
    // Spawn multiple threads for concurrent operations
    for i in 0..10 {
        let index_clone = index.clone();
        let handle = thread::spawn(move || {
            let record = create_test_record(&format!("uuid-{}", i), &format!("collection-{}", i));
            index_clone.upsert_collection(record);
            
            // Immediate lookup to test consistency
            let uuid_result = index_clone.get_by_uuid(&format!("uuid-{}", i));
            assert!(uuid_result.is_some());
            
            let name_result = index_clone.get_by_name(&format!("collection-{}", i));
            assert!(name_result.is_some());
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
    let handles: Vec<_> = (0..10).map(|i| {
        let index_clone = index.clone();
        thread::spawn(move || {
            let result = index_clone.get_by_uuid(&format!("uuid-{}", i));
            assert!(result.is_some());
        })
    }).collect();
    
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_performance_characteristics() {
    let index = SingleCollectionIndex::new();
    
    // Insert test data
    for i in 0..1000 {
        let record = create_test_record(&format!("uuid-{:04}", i), &format!("collection-{:04}", i));
        index.upsert_collection(record);
    }
    
    // Test UUID lookup performance (should be O(1))
    let start = std::time::Instant::now();
    for i in 0..100 {
        let _result = index.get_by_uuid(&format!("uuid-{:04}", i));
    }
    let uuid_duration = start.elapsed();
    
    // Test name lookup performance (O(n) but should be fast)
    let start = std::time::Instant::now();
    for i in 0..100 {
        let _result = index.get_by_name(&format!("collection-{:04}", i));
    }
    let name_duration = start.elapsed();
    
    println!("UUID lookup (100 ops): {:?}", uuid_duration);
    println!("Name lookup (100 ops): {:?}", name_duration);
    
    // UUID lookups should be significantly faster than name lookups
    // But both should be reasonable for practical use
    assert!(uuid_duration < name_duration);
    
    let metrics = index.get_metrics();
    assert!(metrics.avg_lookup_time_ns > 0);
    assert_eq!(metrics.total_collections, 1000);
}
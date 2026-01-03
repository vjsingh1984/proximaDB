//! # Query Result Cache Integration Tests
//!
//! Integration tests for the query result caching functionality.
//! Tests cache hit/miss behavior, TTL expiration, invalidation on collection changes,
//! and dependency tracking for multi-collection queries.

use std::sync::Arc;
use std::time::Duration;

use arrow::array::{ArrayRef, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};

use proximadb::query::cache::{
    CacheInvalidator, ChangeOperation, InvalidationConfig, InvalidationEvent, QueryKey,
    QueryResultCache, QueryResultCacheConfig,
};
use proximadb::query::federated::ExecutionResult;

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a simple test ExecutionResult with 2 rows
fn create_simple_result() -> ExecutionResult {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, true),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec!["1", "2"])) as ArrayRef,
            Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef,
        ],
    )
    .expect("Failed to create RecordBatch");

    ExecutionResult::from_batch(batch)
}

// =============================================================================
// Cache Hit/Miss Tests
// =============================================================================

/// Test that cache hit returns the cached result
#[test]
fn test_cache_hit_returns_cached_result() {
    let cache = QueryResultCache::with_defaults();
    let key = QueryKey::from_sql("SELECT * FROM products WHERE id = 1");
    let result = create_simple_result();

    // Insert into cache
    cache
        .insert(key.clone(), result, vec!["products".to_string()])
        .expect("Insert should succeed");

    // Verify cache hit
    let cached = cache.get(&key);
    assert!(cached.is_some(), "Cache should return a hit");

    let cached_result = cached.unwrap();
    assert_eq!(cached_result.result.row_count(), 2);

    // Verify stats
    let stats = cache.stats();
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.inserts, 1);
}

/// Test that cache miss for unknown query
#[test]
fn test_cache_miss_for_unknown_query() {
    let cache = QueryResultCache::with_defaults();
    let key = QueryKey::from_sql("SELECT * FROM unknown_table");

    let cached = cache.get(&key);
    assert!(cached.is_none(), "Cache should return miss for unknown query");

    let stats = cache.stats();
    assert_eq!(stats.misses, 1);
    assert_eq!(stats.hits, 0);
}

/// Test multiple cache hits for the same query
#[test]
fn test_multiple_cache_hits() {
    let cache = QueryResultCache::with_defaults();
    let key = QueryKey::from_sql("SELECT name FROM users LIMIT 10");

    cache
        .insert(
            key.clone(),
            create_simple_result(),
            vec!["users".to_string()],
        )
        .expect("Insert should succeed");

    // Multiple hits
    for _ in 0..5 {
        let cached = cache.get(&key);
        assert!(cached.is_some());
    }

    let stats = cache.stats();
    assert_eq!(stats.hits, 5);
    assert_eq!(stats.misses, 0);
}

// =============================================================================
// TTL Expiration Tests
// =============================================================================

/// Test that cached results expire after TTL
#[test]
fn test_ttl_expiration_removes_entries() {
    let config = QueryResultCacheConfig {
        default_ttl: Duration::from_millis(50),
        ..Default::default()
    };
    let cache = QueryResultCache::new(config);

    let key = QueryKey::from_sql("SELECT * FROM products");
    cache
        .insert(
            key.clone(),
            create_simple_result(),
            vec!["products".to_string()],
        )
        .expect("Insert should succeed");

    // Verify entry exists
    assert!(cache.contains(&key));
    assert_eq!(cache.len(), 1);

    // Wait for TTL to expire
    std::thread::sleep(Duration::from_millis(100));

    // Entry should be expired (get returns None and triggers expiration)
    let cached = cache.get(&key);
    assert!(cached.is_none(), "Cached result should be expired");

    let stats = cache.stats();
    assert!(
        stats.expirations > 0 || stats.misses > 0,
        "Should have recorded expiration or miss"
    );
}

/// Test cleanup_expired removes all expired entries
#[test]
fn test_cleanup_expired_batch_removal() {
    let config = QueryResultCacheConfig {
        default_ttl: Duration::from_millis(20),
        ..Default::default()
    };
    let cache = QueryResultCache::new(config);

    // Insert multiple entries
    for i in 0..5 {
        let key = QueryKey::from_sql(&format!("SELECT * FROM table_{}", i));
        cache
            .insert(
                key,
                create_simple_result(),
                vec![format!("table_{}", i)],
            )
            .expect("Insert should succeed");
    }

    assert_eq!(cache.len(), 5);

    // Wait for TTL
    std::thread::sleep(Duration::from_millis(50));

    // Cleanup expired
    let cleaned = cache.cleanup_expired();
    assert_eq!(cleaned, 5, "All 5 entries should be cleaned up");
    assert!(cache.is_empty());
}

/// Test custom TTL per entry
#[test]
fn test_custom_ttl_per_entry() {
    let cache = QueryResultCache::with_defaults();

    // Insert with short TTL
    let short_key = QueryKey::from_sql("SELECT * FROM short_lived");
    cache
        .insert_with_ttl(
            short_key.clone(),
            create_simple_result(),
            vec!["short_lived".to_string()],
            Duration::from_millis(30),
        )
        .expect("Insert should succeed");

    // Insert with long TTL
    let long_key = QueryKey::from_sql("SELECT * FROM long_lived");
    cache
        .insert_with_ttl(
            long_key.clone(),
            create_simple_result(),
            vec!["long_lived".to_string()],
            Duration::from_secs(60),
        )
        .expect("Insert should succeed");

    assert_eq!(cache.len(), 2);

    // Wait for short TTL to expire
    std::thread::sleep(Duration::from_millis(50));

    // Short-lived should be expired
    assert!(cache.get(&short_key).is_none());

    // Long-lived should still exist
    assert!(cache.get(&long_key).is_some());
}

// =============================================================================
// Invalidation Tests
// =============================================================================

/// Test invalidation on collection write removes affected entries
#[test]
fn test_invalidation_on_collection_write() {
    let cache = Arc::new(QueryResultCache::with_defaults());
    let config = InvalidationConfig {
        batch_invalidations: false, // Direct invalidation
        ..Default::default()
    };
    let invalidator = CacheInvalidator::with_config(cache.clone(), config);

    // Insert queries for different collections
    let key1 = QueryKey::from_sql("SELECT * FROM products");
    cache
        .insert(
            key1.clone(),
            create_simple_result(),
            vec!["products".to_string()],
        )
        .expect("Insert should succeed");

    let key2 = QueryKey::from_sql("SELECT * FROM users");
    cache
        .insert(
            key2.clone(),
            create_simple_result(),
            vec!["users".to_string()],
        )
        .expect("Insert should succeed");

    assert_eq!(cache.len(), 2);

    // Trigger invalidation for products collection
    let event = InvalidationEvent::new("products", ChangeOperation::Update);
    let invalidated = invalidator.on_change_event(event);

    assert_eq!(invalidated, 1);
    assert_eq!(cache.len(), 1);
    assert!(!cache.contains(&key1), "products query should be invalidated");
    assert!(cache.contains(&key2), "users query should still be cached");
}

/// Test direct collection invalidation
#[test]
fn test_direct_collection_invalidation() {
    let cache = Arc::new(QueryResultCache::with_defaults());
    let invalidator = CacheInvalidator::new(cache.clone());

    // Insert entries
    for collection in &["orders", "inventory", "shipments"] {
        let key = QueryKey::from_sql(&format!("SELECT * FROM {}", collection));
        cache
            .insert(
                key,
                create_simple_result(),
                vec![collection.to_string()],
            )
            .expect("Insert should succeed");
    }

    assert_eq!(cache.len(), 3);

    // Invalidate orders
    let invalidated = invalidator.invalidate_collection("orders");
    assert_eq!(invalidated, 1);
    assert_eq!(cache.len(), 2);

    // Check stats
    let stats = invalidator.stats();
    assert_eq!(stats.collections_invalidated, 1);
    assert_eq!(stats.entries_invalidated, 1);
}

/// Test invalidation of multiple collections
#[test]
fn test_invalidate_multiple_collections() {
    let cache = Arc::new(QueryResultCache::with_defaults());
    let invalidator = CacheInvalidator::new(cache.clone());

    for i in 0..5 {
        let key = QueryKey::from_sql(&format!("SELECT * FROM collection_{}", i));
        cache
            .insert(
                key,
                create_simple_result(),
                vec![format!("collection_{}", i)],
            )
            .expect("Insert should succeed");
    }

    assert_eq!(cache.len(), 5);

    // Invalidate multiple collections at once
    let invalidated =
        invalidator.invalidate_collections(&["collection_0", "collection_2", "collection_4"]);

    assert_eq!(invalidated, 3);
    assert_eq!(cache.len(), 2);
}

/// Test different change operations trigger invalidation
#[test]
fn test_change_operations_trigger_invalidation() {
    let cache = Arc::new(QueryResultCache::with_defaults());
    let config = InvalidationConfig {
        batch_invalidations: false,
        ..Default::default()
    };
    let invalidator = CacheInvalidator::with_config(cache.clone(), config);

    // Test each operation type
    let operations = vec![
        ("insert_collection", ChangeOperation::Insert),
        ("update_collection", ChangeOperation::Update),
        ("delete_collection", ChangeOperation::Delete),
        ("truncate_collection", ChangeOperation::Truncate),
    ];

    for (collection, _op) in &operations {
        let key = QueryKey::from_sql(&format!("SELECT * FROM {}", collection));
        cache
            .insert(
                key,
                create_simple_result(),
                vec![collection.to_string()],
            )
            .expect("Insert should succeed");
    }

    assert_eq!(cache.len(), 4);

    // Each operation should trigger invalidation
    for (collection, op) in operations {
        let event = InvalidationEvent::new(collection, op);
        let invalidated = invalidator.on_change_event(event);
        assert_eq!(invalidated, 1, "{:?} should trigger invalidation", op);
    }

    assert!(cache.is_empty());
}

// =============================================================================
// Dependency Tracking Tests
// =============================================================================

/// Test multi-collection query invalidation on any dependency change
#[test]
fn test_multi_collection_query_invalidation() {
    let cache = QueryResultCache::with_defaults();

    // Query depends on multiple collections (join query)
    let join_key = QueryKey::from_sql("SELECT * FROM orders o JOIN customers c ON o.customer_id = c.id");
    cache
        .insert(
            join_key.clone(),
            create_simple_result(),
            vec!["orders".to_string(), "customers".to_string()],
        )
        .expect("Insert should succeed");

    // Single-table query
    let single_key = QueryKey::from_sql("SELECT * FROM products");
    cache
        .insert(
            single_key.clone(),
            create_simple_result(),
            vec!["products".to_string()],
        )
        .expect("Insert should succeed");

    assert_eq!(cache.len(), 2);

    // Invalidating 'customers' should remove the join query
    let invalidated = cache.invalidate_collection("customers");
    assert_eq!(invalidated, 1);
    assert!(!cache.contains(&join_key));
    assert!(cache.contains(&single_key));
}

/// Test query with three dependencies invalidates correctly
#[test]
fn test_triple_dependency_invalidation() {
    let cache = QueryResultCache::with_defaults();

    // Query depends on three collections
    let key = QueryKey::from_sql(
        "SELECT * FROM a JOIN b ON a.id = b.a_id JOIN c ON b.id = c.b_id",
    );
    cache
        .insert(
            key.clone(),
            create_simple_result(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        )
        .expect("Insert should succeed");

    assert!(cache.contains(&key));

    // Invalidating any of the three should remove the entry
    cache.invalidate_collection("b");
    assert!(!cache.contains(&key));
}

/// Test that independent queries are not affected by unrelated invalidation
#[test]
fn test_independent_queries_not_affected() {
    let cache = QueryResultCache::with_defaults();

    // Insert independent queries
    let key1 = QueryKey::from_sql("SELECT * FROM table_a");
    cache
        .insert(
            key1.clone(),
            create_simple_result(),
            vec!["table_a".to_string()],
        )
        .expect("Insert should succeed");

    let key2 = QueryKey::from_sql("SELECT * FROM table_b");
    cache
        .insert(
            key2.clone(),
            create_simple_result(),
            vec!["table_b".to_string()],
        )
        .expect("Insert should succeed");

    // Invalidate table_a
    cache.invalidate_collection("table_a");

    // table_b query should still be cached
    assert!(cache.contains(&key2));
    assert!(!cache.contains(&key1));
}

// =============================================================================
// LRU Eviction Tests
// =============================================================================

/// Test LRU eviction when max_entries exceeded
#[test]
fn test_lru_eviction_on_capacity() {
    let config = QueryResultCacheConfig {
        max_entries: 3,
        ..Default::default()
    };
    let cache = QueryResultCache::new(config);

    // Insert 3 entries (at capacity)
    for i in 0..3 {
        let key = QueryKey::from_sql(&format!("SELECT * FROM table_{}", i));
        cache
            .insert(
                key,
                create_simple_result(),
                vec![format!("table_{}", i)],
            )
            .expect("Insert should succeed");
        // Small delay to ensure different creation times
        std::thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(cache.len(), 3);

    // Insert 4th entry - should evict oldest (table_0)
    let key_new = QueryKey::from_sql("SELECT * FROM table_new");
    cache
        .insert(
            key_new.clone(),
            create_simple_result(),
            vec!["table_new".to_string()],
        )
        .expect("Insert should succeed");

    assert_eq!(cache.len(), 3);
    assert!(cache.contains(&key_new));

    // Check that oldest entry was evicted
    let key_oldest = QueryKey::from_sql("SELECT * FROM table_0");
    assert!(!cache.contains(&key_oldest), "Oldest entry should be evicted");

    let stats = cache.stats();
    assert!(stats.evictions > 0);
}

/// Test eviction prioritizes expired entries first
#[test]
fn test_eviction_prioritizes_expired() {
    let config = QueryResultCacheConfig {
        max_entries: 3,
        default_ttl: Duration::from_millis(50),
        ..Default::default()
    };
    let cache = QueryResultCache::new(config);

    // Insert entries
    for i in 0..3 {
        let key = QueryKey::from_sql(&format!("SELECT * FROM table_{}", i));
        cache
            .insert(
                key,
                create_simple_result(),
                vec![format!("table_{}", i)],
            )
            .expect("Insert should succeed");
    }

    // Wait for entries to expire
    std::thread::sleep(Duration::from_millis(100));

    // Insert new entry - should clean up expired entries first
    let key_new = QueryKey::from_sql("SELECT * FROM fresh_table");
    cache
        .insert_with_ttl(
            key_new.clone(),
            create_simple_result(),
            vec!["fresh_table".to_string()],
            Duration::from_secs(60), // Long TTL
        )
        .expect("Insert should succeed");

    // Fresh entry should exist
    assert!(cache.contains(&key_new));

    // Old entries should be expired/removed
    let stats = cache.stats();
    assert!(stats.expirations > 0 || stats.evictions > 0);
}

// =============================================================================
// Query Fingerprint Tests
// =============================================================================

/// Test query fingerprint consistency
#[test]
fn test_query_fingerprint_consistency() {
    let sql1 = "SELECT * FROM products WHERE id = 1";
    let sql2 = "SELECT * FROM products WHERE id = 1";
    let sql3 = "SELECT * FROM products WHERE id = 2";

    let key1 = QueryKey::from_sql(sql1);
    let key2 = QueryKey::from_sql(sql2);
    let key3 = QueryKey::from_sql(sql3);

    // Same query should have same fingerprint
    assert_eq!(
        key1.fingerprint, key2.fingerprint,
        "Identical queries should have same fingerprint"
    );

    // Different query should have different fingerprint
    assert_ne!(
        key1.fingerprint, key3.fingerprint,
        "Different queries should have different fingerprints"
    );
}

/// Test query fingerprint with parameters
#[test]
fn test_query_fingerprint_with_params() {
    let sql = "SELECT * FROM products WHERE id = $1";

    let key1 = QueryKey::from_sql_with_params(sql, &["100"]);
    let key2 = QueryKey::from_sql_with_params(sql, &["100"]);
    let key3 = QueryKey::from_sql_with_params(sql, &["200"]);

    // Same query + same params = same fingerprint
    assert_eq!(key1.fingerprint, key2.fingerprint);

    // Same query + different params = different fingerprint
    assert_ne!(key1.fingerprint, key3.fingerprint);
}

/// Test whitespace does not affect fingerprint (if queries are equivalent)
#[test]
fn test_fingerprint_whitespace_sensitivity() {
    // Note: Current implementation is whitespace-sensitive
    // This test documents the behavior
    let key1 = QueryKey::from_sql("SELECT * FROM users");
    let key2 = QueryKey::from_sql("SELECT  *  FROM  users");

    // Different whitespace = different fingerprint (current behavior)
    // If normalization is implemented, this assertion should change
    assert_ne!(
        key1.fingerprint, key2.fingerprint,
        "Whitespace differences create different fingerprints (current behavior)"
    );
}

// =============================================================================
// Transaction-Aware Invalidation Tests
// =============================================================================

/// Test transaction-aware invalidation defers until commit
#[test]
fn test_transaction_aware_invalidation_deferred() {
    let cache = Arc::new(QueryResultCache::with_defaults());
    let config = InvalidationConfig {
        batch_invalidations: false,
        transaction_aware: true,
        ..Default::default()
    };
    let invalidator = CacheInvalidator::with_config(cache.clone(), config);

    // Insert entry
    let key = QueryKey::from_sql("SELECT * FROM orders");
    cache
        .insert(
            key.clone(),
            create_simple_result(),
            vec!["orders".to_string()],
        )
        .expect("Insert should succeed");

    // Event with transaction ID - should be deferred
    let event = InvalidationEvent::new("orders", ChangeOperation::Update)
        .with_transaction_id("txn_12345");
    let invalidated = invalidator.on_change_event(event);

    assert_eq!(invalidated, 0, "Invalidation should be deferred");
    assert!(cache.contains(&key), "Entry should still exist");
}

/// Test transaction commit triggers deferred invalidation
#[test]
fn test_transaction_commit_triggers_invalidation() {
    let cache = Arc::new(QueryResultCache::with_defaults());
    let config = InvalidationConfig {
        batch_invalidations: false,
        transaction_aware: true,
        ..Default::default()
    };
    let invalidator = CacheInvalidator::with_config(cache.clone(), config);

    // Insert entry
    let key = QueryKey::from_sql("SELECT * FROM inventory");
    cache
        .insert(
            key.clone(),
            create_simple_result(),
            vec!["inventory".to_string()],
        )
        .expect("Insert should succeed");

    // Defer invalidation
    let event = InvalidationEvent::new("inventory", ChangeOperation::Delete)
        .with_transaction_id("txn_commit_test");
    invalidator.on_change_event(event);

    assert!(cache.contains(&key), "Entry should exist before commit");

    // Commit transaction
    let invalidated = invalidator.on_transaction_commit("txn_commit_test");

    assert_eq!(invalidated, 1);
    assert!(!cache.contains(&key), "Entry should be invalidated after commit");
}

/// Test transaction rollback discards pending invalidation
#[test]
fn test_transaction_rollback_discards_invalidation() {
    let cache = Arc::new(QueryResultCache::with_defaults());
    let config = InvalidationConfig {
        batch_invalidations: false,
        transaction_aware: true,
        ..Default::default()
    };
    let invalidator = CacheInvalidator::with_config(cache.clone(), config);

    // Insert entry
    let key = QueryKey::from_sql("SELECT * FROM payments");
    cache
        .insert(
            key.clone(),
            create_simple_result(),
            vec!["payments".to_string()],
        )
        .expect("Insert should succeed");

    // Defer invalidation
    let event = InvalidationEvent::new("payments", ChangeOperation::Insert)
        .with_transaction_id("txn_rollback_test");
    invalidator.on_change_event(event);

    // Rollback transaction
    invalidator.on_transaction_rollback("txn_rollback_test");

    // Entry should still exist (rollback discards invalidation)
    assert!(
        cache.contains(&key),
        "Entry should still exist after rollback"
    );
}

// =============================================================================
// Cache Statistics Tests
// =============================================================================

/// Test cache statistics accuracy
#[test]
fn test_cache_statistics_accuracy() {
    let cache = QueryResultCache::with_defaults();

    // Insert
    let key1 = QueryKey::from_sql("SELECT 1");
    cache
        .insert(
            key1.clone(),
            create_simple_result(),
            vec!["t1".to_string()],
        )
        .expect("Insert should succeed");

    let key2 = QueryKey::from_sql("SELECT 2");
    cache
        .insert(
            key2.clone(),
            create_simple_result(),
            vec!["t2".to_string()],
        )
        .expect("Insert should succeed");

    // Hits
    cache.get(&key1);
    cache.get(&key1);
    cache.get(&key2);

    // Misses
    let missing = QueryKey::from_sql("SELECT missing");
    cache.get(&missing);
    cache.get(&missing);

    let stats = cache.stats();
    assert_eq!(stats.inserts, 2);
    assert_eq!(stats.hits, 3);
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.entries, 2);

    // Verify hit rate calculation
    let expected_hit_rate = 3.0 / 5.0; // 3 hits / 5 total
    assert!(
        (stats.hit_rate - expected_hit_rate).abs() < 0.001,
        "Hit rate should be 0.6, got {}",
        stats.hit_rate
    );
}

/// Test cache clear functionality
#[test]
fn test_cache_clear() {
    let cache = QueryResultCache::with_defaults();

    // Insert multiple entries
    for i in 0..10 {
        let key = QueryKey::from_sql(&format!("SELECT * FROM t{}", i));
        cache
            .insert(key, create_simple_result(), vec![format!("t{}", i)])
            .expect("Insert should succeed");
    }

    assert_eq!(cache.len(), 10);
    assert!(!cache.is_empty());

    // Clear cache
    cache.clear();

    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());
}

// =============================================================================
// Concurrent Access Tests
// =============================================================================

/// Test concurrent cache access
#[test]
fn test_concurrent_cache_access() {
    use std::thread;

    let cache = Arc::new(QueryResultCache::with_defaults());
    let mut handles = vec![];

    // Spawn multiple threads doing inserts
    for i in 0..4 {
        let cache_clone = cache.clone();
        handles.push(thread::spawn(move || {
            for j in 0..25 {
                let key = QueryKey::from_sql(&format!("SELECT * FROM thread_{}_query_{}", i, j));
                let _ = cache_clone.insert(
                    key,
                    create_simple_result(),
                    vec![format!("thread_{}_table_{}", i, j)],
                );
            }
        }));
    }

    // Spawn threads doing reads
    for _ in 0..2 {
        let cache_clone = cache.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                let key = QueryKey::from_sql("SELECT * FROM thread_0_query_0");
                let _ = cache_clone.get(&key);
            }
        }));
    }

    // Wait for all threads
    for handle in handles {
        handle.join().expect("Thread should complete");
    }

    // Cache should have entries (exact count depends on timing)
    assert!(cache.len() > 0);

    let stats = cache.stats();
    assert!(stats.inserts > 0);
}

/// Test concurrent invalidation
#[test]
fn test_concurrent_invalidation() {
    use std::thread;

    let cache = Arc::new(QueryResultCache::with_defaults());
    let config = InvalidationConfig {
        batch_invalidations: false,
        ..Default::default()
    };

    // Pre-populate cache
    for i in 0..20 {
        let key = QueryKey::from_sql(&format!("SELECT * FROM collection_{}", i));
        cache
            .insert(
                key,
                create_simple_result(),
                vec![format!("collection_{}", i)],
            )
            .expect("Insert should succeed");
    }

    assert_eq!(cache.len(), 20);

    let invalidator = Arc::new(CacheInvalidator::with_config(cache.clone(), config));
    let mut handles = vec![];

    // Spawn threads doing invalidations
    for i in 0..4 {
        let inv_clone = invalidator.clone();
        handles.push(thread::spawn(move || {
            for j in 0..5 {
                let collection = format!("collection_{}", i * 5 + j);
                inv_clone.invalidate_collection(&collection);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("Thread should complete");
    }

    // All entries should be invalidated
    assert!(cache.is_empty());
}

// =============================================================================
// Edge Cases
// =============================================================================

/// Test empty query string
#[test]
fn test_empty_query_string() {
    let cache = QueryResultCache::with_defaults();
    let key = QueryKey::from_sql("");

    cache
        .insert(key.clone(), create_simple_result(), vec![])
        .expect("Insert should succeed");

    assert!(cache.contains(&key));
    assert!(cache.get(&key).is_some());
}

/// Test very long query string
#[test]
fn test_long_query_string() {
    let cache = QueryResultCache::with_defaults();

    // Create a very long query
    let mut long_query = "SELECT * FROM t WHERE ".to_string();
    for i in 0..100 {
        if i > 0 {
            long_query.push_str(" AND ");
        }
        long_query.push_str(&format!("col_{} = 'value_{}'", i, i));
    }

    let key = QueryKey::from_sql(&long_query);
    cache
        .insert(key.clone(), create_simple_result(), vec!["t".to_string()])
        .expect("Insert should succeed");

    assert!(cache.contains(&key));
}

/// Test cache with no dependencies
#[test]
fn test_query_with_no_dependencies() {
    let cache = QueryResultCache::with_defaults();

    let key = QueryKey::from_sql("SELECT 1 + 1");
    cache
        .insert(key.clone(), create_simple_result(), vec![])
        .expect("Insert should succeed with empty dependencies");

    assert!(cache.contains(&key));

    // Invalidating any collection should not affect this entry
    cache.invalidate_collection("any_collection");
    assert!(cache.contains(&key));
}

/// Test cache remove functionality
#[test]
fn test_cache_remove() {
    let cache = QueryResultCache::with_defaults();

    let key = QueryKey::from_sql("SELECT * FROM removable");
    cache
        .insert(
            key.clone(),
            create_simple_result(),
            vec!["removable".to_string()],
        )
        .expect("Insert should succeed");

    assert!(cache.contains(&key));

    let removed = cache.remove(&key);
    assert!(removed, "Remove should return true");
    assert!(!cache.contains(&key), "Entry should be removed");

    // Second remove should return false
    let removed_again = cache.remove(&key);
    assert!(!removed_again, "Second remove should return false");
}

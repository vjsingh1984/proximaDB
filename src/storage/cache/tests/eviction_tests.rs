use crate::storage::cache::eviction::{
    ARCStrategy, CacheState, EvictionStrategy, LFUStrategy, LRUStrategy,
};

#[test]
fn test_lru_eviction() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut strategy = LRUStrategy::<String>::new();

    // Insert some keys
    strategy.update_on_insert(&"key1".to_string(), 100);
    strategy.update_on_insert(&"key2".to_string(), 100);
    strategy.update_on_insert(&"key3".to_string(), 100);

    // Access key1 and key3 (making key2 the LRU)
    strategy.update_on_access(&"key1".to_string());
    strategy.update_on_access(&"key3".to_string());

    // Key2 should be selected for eviction
    let cache_state = CacheState {
        total_capacity: 300,
        current_size: 300,
        entry_count: 3,
    };

    let victim = strategy.select_victim(&cache_state);
    assert_eq!(victim, Some("key2".to_string()));
}

#[test]
fn test_lfu_eviction() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut strategy = LFUStrategy::<String>::new();

    // Insert keys
    strategy.update_on_insert(&"key1".to_string(), 100);
    strategy.update_on_insert(&"key2".to_string(), 100);
    strategy.update_on_insert(&"key3".to_string(), 100);

    // Access key2 and key3 multiple times
    strategy.update_on_access(&"key2".to_string());
    strategy.update_on_access(&"key2".to_string());
    strategy.update_on_access(&"key3".to_string());
    strategy.update_on_access(&"key3".to_string());
    strategy.update_on_access(&"key3".to_string());

    // Key1 has the lowest frequency and should be evicted
    let cache_state = CacheState {
        total_capacity: 300,
        current_size: 300,
        entry_count: 3,
    };

    let victim = strategy.select_victim(&cache_state);
    assert_eq!(victim, Some("key1".to_string()));
}

#[test]
fn test_arc_eviction() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut strategy = ARCStrategy::<String>::new(4);

    // Insert keys
    strategy.update_on_insert(&"key1".to_string(), 100);
    strategy.update_on_insert(&"key2".to_string(), 100);
    strategy.update_on_insert(&"key3".to_string(), 100);
    strategy.update_on_insert(&"key4".to_string(), 100);

    // Access pattern that promotes key1 to T2 (frequent)
    strategy.update_on_access(&"key1".to_string());
    strategy.update_on_access(&"key1".to_string());

    let cache_state = CacheState {
        total_capacity: 400,
        current_size: 400,
        entry_count: 4,
    };

    // Should evict from T1 (recent but not frequent)
    let victim = strategy.select_victim(&cache_state);
    assert!(victim.is_some());

    // Check statistics
    let stats = strategy.stats();
    assert_eq!(stats.total_accesses, 2);
}

#[test]
fn test_eviction_stats() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut strategy = LRUStrategy::<u64>::new();

    // Perform operations
    strategy.update_on_insert(&1, 100);
    strategy.update_on_insert(&2, 100);
    strategy.update_on_access(&1);
    strategy.update_on_access(&2);
    strategy.update_on_evict(&1);

    let stats = strategy.stats();
    assert_eq!(stats.total_accesses, 2);
    assert_eq!(stats.total_evictions, 1);
}

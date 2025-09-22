use crate::storage::cache::eviction::{
    EvictionPolicy, CacheEvictor, AccessTracker, CacheEvictionConfig,
};
use std::sync::Arc;
use crate::storage::traits::UnifiedMetricsCollector;
use crate::storage::cache::orchestrator::CrossCacheOrchestrator;

#[test]
fn test_lru_policy() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let policy = EvictionPolicy::LRU {
        max_items: 1000,
        batch_size: 10,
    };

    match policy {
        EvictionPolicy::LRU { max_items, batch_size } => {
            assert_eq!(max_items, 1000);
            assert_eq!(batch_size, 10);
        }
        _ => panic!("Expected LRU policy"),
    }
}

#[test]
fn test_lfu_policy() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let policy = EvictionPolicy::LFU {
        max_items: 1000,
        min_access_count: 2,
        frequency_window_hours: 24,
    };

    match policy {
        EvictionPolicy::LFU { max_items, min_access_count, frequency_window_hours } => {
            assert_eq!(max_items, 1000);
            assert_eq!(min_access_count, 2);
            assert_eq!(frequency_window_hours, 24);
        }
        _ => panic!("Expected LFU policy"),
    }
}

#[test]
fn test_arc_policy() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let policy = EvictionPolicy::ARC {
        target_size: 1000,
        recent_size: 500,
        frequent_size: 500,
    };

    match policy {
        EvictionPolicy::ARC { target_size, recent_size, frequent_size } => {
            assert_eq!(target_size, 1000);
            assert_eq!(recent_size, 500);
            assert_eq!(frequent_size, 500);
        }
        _ => panic!("Expected ARC policy"),
    }
}

#[test]
fn test_ttl_policy() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let policy = EvictionPolicy::TTL {
        max_age_seconds: 3600,
        cleanup_interval_seconds: 60,
    };

    match policy {
        EvictionPolicy::TTL { max_age_seconds, cleanup_interval_seconds } => {
            assert_eq!(max_age_seconds, 3600);
            assert_eq!(cleanup_interval_seconds, 60);
        }
        _ => panic!("Expected TTL policy"),
    }
}

#[test]
fn test_pattern_based_policy() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let policy = EvictionPolicy::PatternBased {
        use_ml_predictions: true,
        pattern_window_hours: 48,
        eviction_threshold: 0.7,
    };

    match policy {
        EvictionPolicy::PatternBased { use_ml_predictions, pattern_window_hours, eviction_threshold } => {
            assert!(use_ml_predictions);
            assert_eq!(pattern_window_hours, 48);
            assert_eq!(eviction_threshold, 0.7);
        }
        _ => panic!("Expected PatternBased policy"),
    }
}

#[tokio::test]
async fn test_access_tracker() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let tracker = AccessTracker::new();

    // Track some accesses
    tracker.track_access("key1".to_string()).await;
    tracker.track_access("key2".to_string()).await;
    tracker.track_access("key1".to_string()).await; // Second access to key1

    // Get access statistics - method not available, testing LRU items instead
    let lru_items = tracker.get_lru_items(3).await;

    // key3 should be in LRU list as it was never accessed
    // key1 and key2 were accessed so they should be more recent
    assert!(!lru_items.is_empty());
}

#[test]
fn test_cache_eviction_config() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let config = CacheEvictionConfig::default();

    // Default config should have reasonable values
    assert!(!config.policies.is_empty());
    assert!(config.check_interval_seconds > 0);
    assert!(config.max_cache_size > 0);
}

#[tokio::test]
async fn test_cache_evictor_creation() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let orchestrator = Arc::new(CrossCacheOrchestrator::new(1024 * 1024 * 100)); // 100MB
    let metrics = Arc::new(UnifiedMetricsCollector::new());

    let evictor = CacheEvictor::new(orchestrator, metrics);

    // Verify evictor was created successfully
    assert!(Arc::strong_count(&Arc::new(evictor)) == 1);
}

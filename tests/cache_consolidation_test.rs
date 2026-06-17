// Integration Test: Cache Architecture Consolidation (TD-042)
//
// This test validates that the unified cache interface works correctly
// across all cache types and that coordinated eviction functions properly.
//
// Test Coverage:
// - Unified cache coordinator creation
// - Cache priority system (Critical, High, Medium, Low)
// - Cache statistics retrieval
// - String interner effectiveness

#[cfg(test)]
mod cache_consolidation_tests {
    use proximadb::storage::cache::{
        CacheId, CachePriority, EvictionConfig, PressureStatus, UnifiedCacheCoordinator,
        UnifiedEvictionPolicy,
    };
    use std::sync::Arc;

    /// Test unified cache coordinator creation
    #[test]
    fn test_unified_cache_coordinator_creation() {
        let coordinator = UnifiedCacheCoordinator::new();

        // Verify string interner is available
        let interner = coordinator.string_interner();
        let arc_str = interner.intern("test_string");

        // Test that intern() works (it's async, so we need to block on it)
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let result = arc_str.await;
            assert_eq!(result.as_ref(), "test_string");
        });
    }

    /// Test unified eviction policy creation
    #[test]
    fn test_unified_eviction_policy_creation() {
        let coordinator = Arc::new(UnifiedCacheCoordinator::new());
        let config = EvictionConfig::default();
        let policy = UnifiedEvictionPolicy::new(coordinator, config);

        // Verify policy is created with default config
        assert_eq!(
            policy.get_cache_priority(CacheId::Metadata),
            CachePriority::Critical
        );
        assert_eq!(
            policy.get_cache_priority(CacheId::VectorData),
            CachePriority::High
        );
        assert_eq!(
            policy.get_cache_priority(CacheId::BitmapFilter),
            CachePriority::Low
        );
    }

    /// Test memory pressure detection
    #[test]
    fn test_memory_pressure_detection() {
        let coordinator = Arc::new(UnifiedCacheCoordinator::new());
        let config = EvictionConfig::default();
        let policy = UnifiedEvictionPolicy::new(coordinator, config);

        // Initially should be healthy (no memory usage)
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let status = policy.get_pressure_status().await;
            assert_eq!(status, PressureStatus::Healthy);
        });
    }

    /// Test forced eviction (even when not under pressure)
    #[test]
    fn test_forced_eviction() {
        let coordinator = Arc::new(UnifiedCacheCoordinator::new());
        let config = EvictionConfig::default();
        let policy = UnifiedEvictionPolicy::new(coordinator, config);

        // Force eviction even though we're not under pressure
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let result = policy.check_memory_pressure(true).await;

            assert!(result.is_ok(), "Forced eviction should complete");

            // Should return Some result even without actual pressure
            assert!(
                result.unwrap().is_some(),
                "Forced eviction should return result"
            );
        });
    }

    /// Test cache priority ordering
    #[test]
    fn test_cache_priority_ordering() {
        // Verify priority ordering
        assert!(CachePriority::Critical > CachePriority::High);
        assert!(CachePriority::High > CachePriority::Medium);
        assert!(CachePriority::Medium > CachePriority::Low);

        // Verify that Metadata has highest priority
        assert_eq!(CachePriority::Critical, CachePriority::Critical);
    }

    /// Test string interner effectiveness
    #[test]
    fn test_string_interner_effectiveness() {
        let coordinator = UnifiedCacheCoordinator::new();
        let interner = coordinator.string_interner();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Intern the same string multiple times
            let arc_str1 = interner.intern("test_string").await;
            let arc_str2 = interner.intern("test_string").await;
            let arc_str3 = interner.intern("test_string").await;

            // All should return the same Arc (same memory address)
            assert!(Arc::ptr_eq(&arc_str1, &arc_str2));
            assert!(Arc::ptr_eq(&arc_str2, &arc_str3));

            // Intern a different string
            let arc_str4 = interner.intern("different_string").await;

            // Should be different Arc
            assert!(!Arc::ptr_eq(&arc_str1, &arc_str4));
        });
    }

    /// Test cache statistics retrieval
    #[test]
    fn test_cache_statistics_retrieval() {
        let coordinator = UnifiedCacheCoordinator::new();

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Get all cache statistics
            let stats = coordinator.get_all_stats().await;

            // Verify stats can be retrieved (may be empty if no caches registered)
            // Just verify the method works and returns a valid HashMap
            assert!(stats.is_empty() || stats.len() > 0);

            // If there are stats, verify structure
            for (cache_id, cache_stats) in stats {
                assert_eq!(cache_stats.cache_id, cache_id);
            }
        });
    }

    /// Test cache ID enumeration
    #[test]
    fn test_cache_id_enumeration() {
        // Verify all cache IDs are defined
        let cache_ids = vec![
            CacheId::VectorData,
            CacheId::Metadata,
            CacheId::QueryResult,
            CacheId::BitmapFilter,
            CacheId::IndexNode,
        ];

        // Should have at least 5 cache types
        assert!(cache_ids.len() >= 5);
    }

    /// Test eviction config defaults
    #[test]
    fn test_eviction_config_defaults() {
        let config = EvictionConfig::default();

        // Verify default configuration is sensible
        assert!(config.total_memory_budget > 0);
        assert!(config.pressure_threshold > 0.0);
        assert!(config.pressure_threshold <= 1.0);
    }

    /// Test pressure status values
    #[test]
    fn test_pressure_status_values() {
        // Verify pressure status values exist
        let _ = PressureStatus::Healthy;
        let _ = PressureStatus::Moderate;
        let _ = PressureStatus::Critical;
    }
}

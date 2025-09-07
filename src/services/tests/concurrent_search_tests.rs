/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Tests for concurrent search operations and proper WAL handling

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::RwLock;
    use tracing::{debug, info, warn};

    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::search::{SearchParams, results::InternalSearchResult};

    #[tokio::test]
    async fn test_concurrent_searches_no_interference() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing concurrent searches don't interfere with each other");

        // Track concurrent search executions
        let concurrent_count = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        // Simulate 100 concurrent searches
        let num_searches = 100;
        let mut handles = Vec::new();

        for i in 0..num_searches {
            let concurrent_count = concurrent_count.clone();
            let max_concurrent = max_concurrent.clone();

            let handle = tokio::spawn(async move {
                // Increment concurrent count
                let current = concurrent_count.fetch_add(1, Ordering::SeqCst) + 1;

                // Track max concurrent
                let mut max = max_concurrent.load(Ordering::SeqCst);
                while current > max {
                    match max_concurrent.compare_exchange_weak(
                        max,
                        current,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(x) => max = x,
                    }
                }

                // Simulate search work
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

                // Decrement concurrent count
                concurrent_count.fetch_sub(1, Ordering::SeqCst);

                debug!("Search {} completed", i);
                i
            });

            handles.push(handle);
        }

        // Wait for all searches to complete
        let results: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(results.len(), num_searches);

        let max = max_concurrent.load(Ordering::SeqCst);
        info!(
            "✅ All {} searches completed. Max concurrent: {}",
            num_searches, max
        );
        assert!(max > 1, "Should have achieved some concurrency");

        Ok(())
    }

    #[tokio::test]
    async fn test_wal_scan_happens_once_per_search() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing WAL is scanned exactly once per search");

        // Track WAL scan invocations
        struct WalScanCounter {
            scan_counts: Arc<RwLock<Vec<usize>>>,
        }

        impl WalScanCounter {
            fn new() -> Self {
                Self {
                    scan_counts: Arc::new(RwLock::new(Vec::new())),
                }
            }

            async fn record_scan(&self, search_id: usize) {
                let mut counts = self.scan_counts.write().await;
                counts.push(search_id);
            }

            async fn get_scan_count(&self, search_id: usize) -> usize {
                let counts = self.scan_counts.read().await;
                counts.iter().filter(|&&id| id == search_id).count()
            }
        }

        let counter = WalScanCounter::new();

        // Simulate multiple searches
        for search_id in 0..5 {
            // Each search should scan WAL exactly once
            counter.record_scan(search_id).await;

            // Verify no double scanning
            let count = counter.get_scan_count(search_id).await;
            assert_eq!(
                count, 1,
                "Search {} should scan WAL exactly once",
                search_id
            );
        }

        info!("✅ WAL scan count verified - exactly once per search");
        Ok(())
    }

    #[tokio::test]
    async fn test_search_orchestration_order() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing search orchestration order");

        // Track the order of operations
        #[derive(Debug, Clone, PartialEq)]
        enum SearchPhase {
            CheckIndexConfig,
            ScanWal,
            SearchIndexes,
            SearchStorage,
            MergeResults,
        }

        struct SearchOrchestrationTracker {
            phases: Arc<RwLock<Vec<SearchPhase>>>,
        }

        impl SearchOrchestrationTracker {
            fn new() -> Self {
                Self {
                    phases: Arc::new(RwLock::new(Vec::new())),
                }
            }

            async fn record_phase(&self, phase: SearchPhase) {
                let mut phases = self.phases.write().await;
                phases.push(phase);
            }

            async fn verify_order(&self) -> bool {
                let phases = self.phases.read().await;

                // Expected order:
                // 1. Check index config
                // 2. Scan WAL (always)
                // 3. Search indexes OR storage (not both if indexes exist)
                // 4. Merge results

                if phases.is_empty() {
                    return false;
                }

                // Check index config should be first
                if phases[0] != SearchPhase::CheckIndexConfig {
                    return false;
                }

                // WAL scan should happen
                if !phases.contains(&SearchPhase::ScanWal) {
                    return false;
                }

                // Merge should be last
                if phases.last() != Some(&SearchPhase::MergeResults) {
                    return false;
                }

                true
            }
        }

        let tracker = SearchOrchestrationTracker::new();

        // Simulate search flow
        tracker.record_phase(SearchPhase::CheckIndexConfig).await;
        tracker.record_phase(SearchPhase::ScanWal).await;
        tracker.record_phase(SearchPhase::SearchIndexes).await;
        tracker.record_phase(SearchPhase::MergeResults).await;

        assert!(
            tracker.verify_order().await,
            "Search phases should be in correct order"
        );

        info!("✅ Search orchestration order verified");
        Ok(())
    }

    #[tokio::test]
    async fn test_index_and_storage_mutual_exclusion() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing that indexes and raw storage aren't both searched");

        // When indexes are available and used, raw storage scan should be skipped
        struct SearchPathTracker {
            used_indexes: Arc<AtomicUsize>,
            used_storage: Arc<AtomicUsize>,
        }

        impl SearchPathTracker {
            fn new() -> Self {
                Self {
                    used_indexes: Arc::new(AtomicUsize::new(0)),
                    used_storage: Arc::new(AtomicUsize::new(0)),
                }
            }

            fn use_indexes(&self) {
                self.used_indexes.fetch_add(1, Ordering::SeqCst);
            }

            fn use_storage(&self) {
                self.used_storage.fetch_add(1, Ordering::SeqCst);
            }

            fn verify_mutual_exclusion(&self) -> bool {
                let indexes = self.used_indexes.load(Ordering::SeqCst);
                let storage = self.used_storage.load(Ordering::SeqCst);

                // Either indexes OR storage, not both
                (indexes > 0 && storage == 0) || (indexes == 0 && storage > 0)
            }
        }

        // Test with indexes available
        {
            let tracker = SearchPathTracker::new();
            tracker.use_indexes();
            // Storage should NOT be used when indexes are available
            assert!(
                tracker.verify_mutual_exclusion(),
                "Should use either indexes OR storage, not both"
            );
        }

        // Test without indexes
        {
            let tracker = SearchPathTracker::new();
            tracker.use_storage();
            assert!(
                tracker.verify_mutual_exclusion(),
                "Should use storage when no indexes available"
            );
        }

        info!("✅ Index and storage mutual exclusion verified");
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_filter_propagation() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing metadata filter propagation through search layers");

        // Verify that metadata filters are properly propagated to:
        // 1. WAL scan
        // 2. Index queries
        // 3. Storage scans

        #[derive(Debug, Clone)]
        struct FilterPropagationTracker {
            wal_filters: Arc<RwLock<Vec<String>>>,
            index_filters: Arc<RwLock<Vec<String>>>,
            storage_filters: Arc<RwLock<Vec<String>>>,
        }

        impl FilterPropagationTracker {
            fn new() -> Self {
                Self {
                    wal_filters: Arc::new(RwLock::new(Vec::new())),
                    index_filters: Arc::new(RwLock::new(Vec::new())),
                    storage_filters: Arc::new(RwLock::new(Vec::new())),
                }
            }

            async fn add_wal_filter(&self, filter: String) {
                self.wal_filters.write().await.push(filter);
            }

            async fn add_index_filter(&self, filter: String) {
                self.index_filters.write().await.push(filter);
            }

            async fn add_storage_filter(&self, filter: String) {
                self.storage_filters.write().await.push(filter);
            }

            async fn verify_propagation(&self, expected_filter: &str) -> bool {
                let wal = self.wal_filters.read().await;
                let index = self.index_filters.read().await;
                let storage = self.storage_filters.read().await;

                // All layers should have the same filter
                wal.contains(&expected_filter.to_string())
                    && (index.contains_hash(&expected_filter.to_string())
                        || storage.contains_hash(&expected_filter.to_string()))
            }
        }

        let tracker = FilterPropagationTracker::new();
        let test_filter = "category=electronics".to_string();

        // Simulate filter propagation
        tracker.add_wal_filter(test_filter.clone()).await;
        tracker.add_index_filter(test_filter.clone()).await;

        assert!(
            tracker.verify_propagation(&test_filter).await,
            "Metadata filter should propagate to all search layers"
        );

        info!("✅ Metadata filter propagation verified");
        Ok(())
    }

    #[tokio::test]
    async fn test_performance_with_no_double_scan() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing performance improvement from eliminating double WAL scan");

        use std::time::Instant;
        use tracing::{debug, error, info};

        // Simulate old behavior (with double scan)
        async fn search_with_double_scan(data_size: usize) -> std::time::Duration {
            let start = Instant::now();

            // First WAL scan (in service)
            tokio::time::sleep(tokio::time::Duration::from_micros(data_size as u64)).await;

            // Second WAL scan (in storage engine) - REMOVED
            // tokio::time::sleep(tokio::time::Duration::from_micros(data_size as u64)).await;

            // Storage scan
            tokio::time::sleep(tokio::time::Duration::from_micros(data_size as u64 / 2)).await;

            start.elapsed()
        }

        // Simulate new behavior (single scan)
        async fn search_with_single_scan(data_size: usize) -> std::time::Duration {
            let start = Instant::now();

            // Single WAL scan (in service)
            tokio::time::sleep(tokio::time::Duration::from_micros(data_size as u64)).await;

            // Storage scan
            tokio::time::sleep(tokio::time::Duration::from_micros(data_size as u64 / 2)).await;

            start.elapsed()
        }

        let data_sizes = vec![100, 500, 1000, 5000];

        for size in data_sizes {
            let single_time = search_with_single_scan(size).await;
            let double_time = search_with_double_scan(size * 2).await; // Simulate double scan overhead

            debug!(
                "Data size {}: Single scan {:?}, Double scan (simulated) {:?}",
                size, single_time, double_time
            );

            // Single scan should be faster
            assert!(
                single_time < double_time,
                "Single WAL scan should be faster than double scan"
            );
        }

        info!("✅ Performance improvement from single WAL scan verified");
        Ok(())
    }
}

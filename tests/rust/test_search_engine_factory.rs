//! Comprehensive Unit Tests for SearchEngineFactory and Multi-Tier Deduplication
//!
//! This test suite validates:
//! - Storage-aware polymorphic search engine selection
//! - Multi-tier deduplication across WAL, flushed, and compacted storage
//! - Search result aggregation and ranking
//! - Performance optimization through engine selection
//! - Cross-tier consistency and correctness

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use proximadb::{
    core::{
        CollectionId, SearchResult, VectorRecord, MetadataFilter, FieldCondition,
        SearchStrategy, SearchContext, SearchDebugInfo,
    },
    core::search::{
        SearchEngineFactory, SearchEngineType, MultiTierSearchCoordinator,
        DeduplicationStrategy, SearchTier, TierSearchResult,
    },
    storage::{
        engines::viper::core::ViperCoreEngine,
        engines::lsm::LsmTree,
        persistence::wal::{WalManager, WalStrategy},
        StorageEngine,
    },
    compute::distance::DistanceMetric,
};

/// Test fixture for SearchEngineFactory testing
struct SearchEngineFactoryTestFixture {
    factory: SearchEngineFactory,
    coordinator: MultiTierSearchCoordinator,
    collection_id: String,
    test_data: TestData,
}

/// Test data distributed across multiple storage tiers
struct TestData {
    wal_vectors: Vec<VectorRecord>,      // Unflushed data in WAL
    flushed_vectors: Vec<VectorRecord>,  // Recently flushed data
    compacted_vectors: Vec<VectorRecord>, // Long-term compacted data
}

impl SearchEngineFactoryTestFixture {
    /// Create a new test fixture with multi-tier test data
    async fn new() -> Result<Self> {
        let collection_id = format!("test_search_collection_{}", Uuid::new_v4());
        
        // Create search engine factory
        let factory = SearchEngineFactory::new().await?;
        
        // Create multi-tier search coordinator
        let coordinator = MultiTierSearchCoordinator::new(
            Self::create_mock_wal_strategy().await?,
            Self::create_mock_viper_engine().await?,
            Self::create_mock_lsm_engine().await?,
        ).await?;
        
        // Create test data across tiers
        let test_data = Self::create_multi_tier_test_data(&collection_id);
        
        Ok(Self {
            factory,
            coordinator,
            collection_id,
            test_data,
        })
    }
    
    /// Create mock WAL strategy for testing
    async fn create_mock_wal_strategy() -> Result<Arc<dyn WalStrategy>> {
        // Implementation would create a mock WAL strategy
        // For testing, we'll use a simplified mock
        todo!("Create mock WAL strategy")
    }
    
    /// Create mock VIPER engine for testing
    async fn create_mock_viper_engine() -> Result<Arc<ViperCoreEngine>> {
        // Implementation would create a mock VIPER engine
        todo!("Create mock VIPER engine")
    }
    
    /// Create mock LSM engine for testing
    async fn create_mock_lsm_engine() -> Result<Arc<LsmTree>> {
        // Implementation would create a mock LSM engine
        todo!("Create mock LSM engine")
    }
    
    /// Create test data distributed across storage tiers
    fn create_multi_tier_test_data(collection_id: &str) -> TestData {
        let base_time = chrono::Utc::now();
        
        // WAL vectors (most recent, unflushed)
        let wal_vectors: Vec<VectorRecord> = (0..10)
            .map(|i| VectorRecord {
                id: format!("wal_vector_{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![1.0 + i as f32, 2.0 + i as f32, 3.0 + i as f32, 4.0 + i as f32],
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("tier".to_string(), serde_json::Value::String("wal".to_string()));
                    meta.insert("timestamp".to_string(), serde_json::Value::Number(serde_json::Number::from(i)));
                    meta
                },
                timestamp_utc: base_time + chrono::Duration::minutes(i as i64),
                ..Default::default()
            })
            .collect();
        
        // Flushed vectors (recently flushed from WAL)
        let flushed_vectors: Vec<VectorRecord> = (10..25)
            .map(|i| VectorRecord {
                id: format!("flushed_vector_{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![1.5 + i as f32, 2.5 + i as f32, 3.5 + i as f32, 4.5 + i as f32],
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("tier".to_string(), serde_json::Value::String("flushed".to_string()));
                    meta.insert("timestamp".to_string(), serde_json::Value::Number(serde_json::Number::from(i)));
                    meta
                },
                timestamp_utc: base_time + chrono::Duration::hours(i as i64),
                ..Default::default()
            })
            .collect();
        
        // Compacted vectors (long-term storage)
        let compacted_vectors: Vec<VectorRecord> = (25..50)
            .map(|i| VectorRecord {
                id: format!("compacted_vector_{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![2.0 + i as f32, 3.0 + i as f32, 4.0 + i as f32, 5.0 + i as f32],
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("tier".to_string(), serde_json::Value::String("compacted".to_string()));
                    meta.insert("timestamp".to_string(), serde_json::Value::Number(serde_json::Number::from(i)));
                    meta
                },
                timestamp_utc: base_time + chrono::Duration::days(i as i64),
                ..Default::default()
            })
            .collect();
        
        TestData {
            wal_vectors,
            flushed_vectors,
            compacted_vectors,
        }
    }
    
    /// Create overlapping data for deduplication testing
    fn create_overlapping_test_data(&self) -> (Vec<VectorRecord>, Vec<VectorRecord>, Vec<VectorRecord>) {
        let base_time = chrono::Utc::now();
        
        // Same vector ID exists in multiple tiers (different versions)
        let vector_id = "duplicate_vector_123";
        let collection_id = &self.collection_id;
        
        // Version 1 in compacted storage (oldest)
        let compacted_version = VectorRecord {
            id: vector_id.to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![1.0, 1.0, 1.0, 1.0],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("version".to_string(), serde_json::Value::Number(serde_json::Number::from(1)));
                meta.insert("tier".to_string(), serde_json::Value::String("compacted".to_string()));
                meta
            },
            timestamp_utc: base_time - chrono::Duration::hours(24),
            ..Default::default()
        };
        
        // Version 2 in flushed storage (middle)
        let flushed_version = VectorRecord {
            id: vector_id.to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![2.0, 2.0, 2.0, 2.0],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("version".to_string(), serde_json::Value::Number(serde_json::Number::from(2)));
                meta.insert("tier".to_string(), serde_json::Value::String("flushed".to_string()));
                meta
            },
            timestamp_utc: base_time - chrono::Duration::hours(2),
            ..Default::default()
        };
        
        // Version 3 in WAL (newest - should be the one returned)
        let wal_version = VectorRecord {
            id: vector_id.to_string(),
            collection_id: collection_id.to_string(),
            vector: vec![3.0, 3.0, 3.0, 3.0],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("version".to_string(), serde_json::Value::Number(serde_json::Number::from(3)));
                meta.insert("tier".to_string(), serde_json::Value::String("wal".to_string()));
                meta
            },
            timestamp_utc: base_time,
            ..Default::default()
        };
        
        (vec![compacted_version], vec![flushed_version], vec![wal_version])
    }
}

#[tokio::test]
async fn test_search_engine_factory_initialization() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Verify factory is properly initialized
    assert!(!fixture.collection_id.is_empty());
    
    println!("✅ SearchEngineFactory initialized successfully");
    Ok(())
}

#[tokio::test]
async fn test_storage_aware_engine_selection() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Test engine selection for different storage strategies
    
    // Test VIPER engine selection
    let viper_engine = fixture.factory.create_engine(
        &fixture.collection_id,
        SearchEngineType::VIPER,
        &SearchContext::default(),
    ).await?;
    
    assert_eq!(viper_engine.engine_type(), SearchEngineType::VIPER);
    println!("✅ VIPER engine selection verified");
    
    // Test LSM engine selection
    let lsm_engine = fixture.factory.create_engine(
        &fixture.collection_id,
        SearchEngineType::LSM,
        &SearchContext::default(),
    ).await?;
    
    assert_eq!(lsm_engine.engine_type(), SearchEngineType::LSM);
    println!("✅ LSM engine selection verified");
    
    // Test WAL engine selection
    let wal_engine = fixture.factory.create_engine(
        &fixture.collection_id,
        SearchEngineType::WAL,
        &SearchContext::default(),
    ).await?;
    
    assert_eq!(wal_engine.engine_type(), SearchEngineType::WAL);
    println!("✅ WAL engine selection verified");
    
    // Test automatic engine selection based on data distribution
    let auto_engine = fixture.factory.create_optimal_engine(
        &fixture.collection_id,
        &SearchContext::default(),
    ).await?;
    
    // Should select the most appropriate engine based on data characteristics
    assert!(matches!(auto_engine.engine_type(), 
        SearchEngineType::VIPER | SearchEngineType::LSM | SearchEngineType::WAL));
    println!("✅ Automatic engine selection verified: {:?}", auto_engine.engine_type());
    
    Ok(())
}

#[tokio::test]
async fn test_multi_tier_search_coordination() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Prepare test data across tiers
    fixture.coordinator.setup_test_data(
        &fixture.collection_id,
        &fixture.test_data.wal_vectors,
        &fixture.test_data.flushed_vectors,
        &fixture.test_data.compacted_vectors,
    ).await?;
    
    // Perform multi-tier search
    let query_vector = vec![15.0, 16.0, 17.0, 18.0]; // Should match vectors across tiers
    let search_results = fixture.coordinator.search_across_tiers(
        &fixture.collection_id,
        &query_vector,
        10, // k
        &DistanceMetric::Cosine,
        None, // no metadata filter
    ).await?;
    
    // Verify results come from multiple tiers
    assert!(search_results.len() > 0, "Should return search results");
    assert!(search_results.len() <= 10, "Should respect k limit");
    
    // Check that results span multiple tiers
    let tier_sources: std::collections::HashSet<String> = search_results
        .iter()
        .filter_map(|r| r.metadata.get("tier"))
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect();
    
    assert!(tier_sources.len() > 1, "Results should come from multiple tiers, found: {:?}", tier_sources);
    
    // Verify results are properly ranked by similarity
    let scores: Vec<f32> = search_results.iter().map(|r| r.score).collect();
    let mut sorted_scores = scores.clone();
    sorted_scores.sort_by(|a, b| b.partial_cmp(a).unwrap()); // Descending order
    
    assert_eq!(scores, sorted_scores, "Results should be ranked by similarity score");
    
    println!("✅ Multi-tier search coordination verified: {} results from {} tiers", 
             search_results.len(), tier_sources.len());
    
    Ok(())
}

#[tokio::test]
async fn test_deduplication_across_tiers() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Create overlapping data across tiers
    let (compacted_data, flushed_data, wal_data) = fixture.create_overlapping_test_data();
    
    // Setup overlapping test data
    fixture.coordinator.setup_test_data(
        &fixture.collection_id,
        &wal_data,
        &flushed_data,
        &compacted_data,
    ).await?;
    
    // Perform search that should find the duplicate across tiers
    let query_vector = vec![3.0, 3.0, 3.0, 3.0]; // Matches the latest version in WAL
    let search_results = fixture.coordinator.search_across_tiers(
        &fixture.collection_id,
        &query_vector,
        10,
        &DistanceMetric::Cosine,
        None,
    ).await?;
    
    // Find results for our duplicate vector
    let duplicate_results: Vec<&SearchResult> = search_results
        .iter()
        .filter(|r| r.id == "duplicate_vector_123")
        .collect();
    
    // Should have exactly one result (the latest version from WAL)
    assert_eq!(duplicate_results.len(), 1, "Should have exactly one deduplicated result");
    
    let result = duplicate_results[0];
    
    // Verify it's the latest version (from WAL)
    assert_eq!(result.metadata.get("version").unwrap().as_u64().unwrap(), 3);
    assert_eq!(result.metadata.get("tier").unwrap().as_str().unwrap(), "wal");
    
    // Verify the vector data matches the latest version
    assert_eq!(result.vector, Some(vec![3.0, 3.0, 3.0, 3.0]));
    
    println!("✅ Deduplication verified: Latest version (v3) returned from WAL tier");
    
    Ok(())
}

#[tokio::test]
async fn test_search_with_metadata_filtering() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Setup test data
    fixture.coordinator.setup_test_data(
        &fixture.collection_id,
        &fixture.test_data.wal_vectors,
        &fixture.test_data.flushed_vectors,
        &fixture.test_data.compacted_vectors,
    ).await?;
    
    // Create metadata filter to only return results from specific tier
    let metadata_filter = MetadataFilter {
        conditions: vec![FieldCondition {
            field: "tier".to_string(),
            operator: "equals".to_string(),
            value: serde_json::Value::String("flushed".to_string()),
        }],
        logic: "AND".to_string(),
    };
    
    // Perform filtered search
    let query_vector = vec![15.0, 16.0, 17.0, 18.0];
    let search_results = fixture.coordinator.search_across_tiers(
        &fixture.collection_id,
        &query_vector,
        20,
        &DistanceMetric::Cosine,
        Some(metadata_filter),
    ).await?;
    
    // Verify all results are from the flushed tier
    for result in &search_results {
        let tier = result.metadata.get("tier").unwrap().as_str().unwrap();
        assert_eq!(tier, "flushed", "All results should be from flushed tier");
    }
    
    // Verify we got results from the flushed tier
    assert!(search_results.len() > 0, "Should find results in flushed tier");
    assert!(search_results.len() <= 15, "Should only include flushed vectors (10-25)");
    
    println!("✅ Metadata filtering verified: {} results from flushed tier only", search_results.len());
    
    Ok(())
}

#[tokio::test]
async fn test_performance_optimization() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Test scenarios for performance optimization
    
    // Scenario 1: Recent data (should prefer WAL search)
    let recent_context = SearchContext {
        recent_data_preference: true,
        max_age_hours: Some(1),
        ..Default::default()
    };
    
    let recent_engine = fixture.factory.create_optimal_engine(
        &fixture.collection_id,
        &recent_context,
    ).await?;
    
    // Should select WAL engine for recent data
    assert_eq!(recent_engine.engine_type(), SearchEngineType::WAL);
    println!("✅ Recent data optimization: WAL engine selected");
    
    // Scenario 2: Large result set (should prefer VIPER for better throughput)
    let large_result_context = SearchContext {
        expected_result_size: Some(1000),
        performance_priority: "throughput".to_string(),
        ..Default::default()
    };
    
    let throughput_engine = fixture.factory.create_optimal_engine(
        &fixture.collection_id,
        &large_result_context,
    ).await?;
    
    // Should select VIPER engine for large result sets
    assert_eq!(throughput_engine.engine_type(), SearchEngineType::VIPER);
    println!("✅ Throughput optimization: VIPER engine selected");
    
    // Scenario 3: Low latency requirement (should prefer LSM for consistent performance)
    let low_latency_context = SearchContext {
        performance_priority: "latency".to_string(),
        max_latency_ms: Some(10),
        ..Default::default()
    };
    
    let latency_engine = fixture.factory.create_optimal_engine(
        &fixture.collection_id,
        &low_latency_context,
    ).await?;
    
    // Should select LSM engine for low latency
    assert_eq!(latency_engine.engine_type(), SearchEngineType::LSM);
    println!("✅ Latency optimization: LSM engine selected");
    
    Ok(())
}

#[tokio::test]
async fn test_search_result_aggregation() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Setup test data with known similarities
    fixture.coordinator.setup_test_data(
        &fixture.collection_id,
        &fixture.test_data.wal_vectors,
        &fixture.test_data.flushed_vectors,
        &fixture.test_data.compacted_vectors,
    ).await?;
    
    // Perform search with specific k value
    let query_vector = vec![10.0, 11.0, 12.0, 13.0];
    let k = 5;
    let search_results = fixture.coordinator.search_across_tiers(
        &fixture.collection_id,
        &query_vector,
        k,
        &DistanceMetric::Euclidean,
        None,
    ).await?;
    
    // Verify result aggregation properties
    assert_eq!(search_results.len(), k, "Should return exactly k results");
    
    // Verify all results have required fields
    for result in &search_results {
        assert!(!result.id.is_empty(), "Result should have valid ID");
        assert!(result.score >= 0.0, "Result should have valid score");
        assert!(!result.metadata.is_empty(), "Result should have metadata");
    }
    
    // Verify results are globally ranked (best from all tiers)
    let scores: Vec<f32> = search_results.iter().map(|r| r.score).collect();
    
    // For Euclidean distance, smaller scores are better
    for i in 1..scores.len() {
        assert!(scores[i] >= scores[i-1], "Results should be ranked by distance (ascending for Euclidean)");
    }
    
    // Verify no duplicate IDs in results
    let mut ids = std::collections::HashSet::new();
    for result in &search_results {
        assert!(ids.insert(&result.id), "Should not have duplicate IDs in results");
    }
    
    println!("✅ Result aggregation verified: {} unique, ranked results", search_results.len());
    
    Ok(())
}

#[tokio::test]
async fn test_tier_search_isolation() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Test individual tier search to verify isolation
    
    // Setup test data
    fixture.coordinator.setup_test_data(
        &fixture.collection_id,
        &fixture.test_data.wal_vectors,
        &fixture.test_data.flushed_vectors,
        &fixture.test_data.compacted_vectors,
    ).await?;
    
    let query_vector = vec![5.0, 6.0, 7.0, 8.0];
    
    // Search each tier individually
    let wal_results = fixture.coordinator.search_tier(
        SearchTier::WAL,
        &fixture.collection_id,
        &query_vector,
        10,
        &DistanceMetric::Cosine,
        None,
    ).await?;
    
    let flushed_results = fixture.coordinator.search_tier(
        SearchTier::Flushed,
        &fixture.collection_id,
        &query_vector,
        10,
        &DistanceMetric::Cosine,
        None,
    ).await?;
    
    let compacted_results = fixture.coordinator.search_tier(
        SearchTier::Compacted,
        &fixture.collection_id,
        &query_vector,
        10,
        &DistanceMetric::Cosine,
        None,
    ).await?;
    
    // Verify tier isolation
    
    // WAL results should only contain WAL vectors
    for result in &wal_results {
        assert!(result.id.starts_with("wal_vector_"), "WAL results should only contain WAL vectors");
    }
    
    // Flushed results should only contain flushed vectors
    for result in &flushed_results {
        assert!(result.id.starts_with("flushed_vector_"), "Flushed results should only contain flushed vectors");
    }
    
    // Compacted results should only contain compacted vectors
    for result in &compacted_results {
        assert!(result.id.starts_with("compacted_vector_"), "Compacted results should only contain compacted vectors");
    }
    
    // Verify each tier has results
    assert!(wal_results.len() > 0, "WAL tier should have results");
    assert!(flushed_results.len() > 0, "Flushed tier should have results");
    assert!(compacted_results.len() > 0, "Compacted tier should have results");
    
    println!("✅ Tier isolation verified: WAL={}, Flushed={}, Compacted={} results", 
             wal_results.len(), flushed_results.len(), compacted_results.len());
    
    Ok(())
}

#[tokio::test]
async fn test_search_debug_information() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Setup test data
    fixture.coordinator.setup_test_data(
        &fixture.collection_id,
        &fixture.test_data.wal_vectors,
        &fixture.test_data.flushed_vectors,
        &fixture.test_data.compacted_vectors,
    ).await?;
    
    // Perform search with debug information
    let query_vector = vec![12.0, 13.0, 14.0, 15.0];
    let search_context = SearchContext {
        enable_debug: true,
        ..Default::default()
    };
    
    let (search_results, debug_info) = fixture.coordinator.search_with_debug(
        &fixture.collection_id,
        &query_vector,
        8,
        &DistanceMetric::Cosine,
        None,
        &search_context,
    ).await?;
    
    // Verify debug information is populated
    assert!(debug_info.total_search_time_ms > 0, "Should record search time");
    assert!(debug_info.tier_search_times.len() > 0, "Should record tier search times");
    assert!(debug_info.candidates_examined > 0, "Should record candidates examined");
    assert!(debug_info.deduplication_removed > 0, "Should record deduplication count");
    
    // Verify tier-specific debug info
    assert!(debug_info.tier_search_times.contains_key(&SearchTier::WAL), "Should have WAL timing");
    assert!(debug_info.tier_search_times.contains_key(&SearchTier::Flushed), "Should have Flushed timing");
    assert!(debug_info.tier_search_times.contains_key(&SearchTier::Compacted), "Should have Compacted timing");
    
    // Verify search results are still correct
    assert_eq!(search_results.len(), 8, "Should return requested number of results");
    
    println!("✅ Debug information verified:");
    println!("   Total time: {}ms", debug_info.total_search_time_ms);
    println!("   Candidates examined: {}", debug_info.candidates_examined);
    println!("   Deduplication removed: {}", debug_info.deduplication_removed);
    println!("   Tier times: {:?}", debug_info.tier_search_times);
    
    Ok(())
}

#[tokio::test]
async fn test_concurrent_multi_tier_searches() -> Result<()> {
    let fixture = SearchEngineFactoryTestFixture::new().await?;
    
    // Setup test data
    fixture.coordinator.setup_test_data(
        &fixture.collection_id,
        &fixture.test_data.wal_vectors,
        &fixture.test_data.flushed_vectors,
        &fixture.test_data.compacted_vectors,
    ).await?;
    
    // Perform multiple concurrent searches
    let mut handles = vec![];
    
    for i in 0..5 {
        let coordinator = fixture.coordinator.clone();
        let collection_id = fixture.collection_id.clone();
        let query_vector = vec![i as f32, (i + 1) as f32, (i + 2) as f32, (i + 3) as f32];
        
        let handle = tokio::spawn(async move {
            coordinator.search_across_tiers(
                &collection_id,
                &query_vector,
                5,
                &DistanceMetric::Cosine,
                None,
            ).await
        });
        
        handles.push(handle);
    }
    
    // Wait for all searches to complete
    let mut all_results = vec![];
    for handle in handles {
        let result = handle.await.unwrap()?;
        all_results.push(result);
    }
    
    // Verify all searches completed successfully
    assert_eq!(all_results.len(), 5, "All concurrent searches should complete");
    
    for (i, results) in all_results.iter().enumerate() {
        assert!(results.len() > 0, "Search {} should return results", i);
        assert!(results.len() <= 5, "Search {} should respect k limit", i);
    }
    
    println!("✅ Concurrent multi-tier searches verified: {} successful searches", all_results.len());
    
    Ok(())
}
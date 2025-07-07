//! Unit tests for multi-tier deduplication scenarios
//! 
//! Tests cover:
//! - Multiple upserts in different unflushed batches
//! - Cross-tier deduplication (unflushed > flushed > compacted)
//! - Same-tier ordering by sequence/version/timestamp
//! - Metadata filtering during deduplication
//! - Records without IDs (no deduplication)
//! - Cross-engine scenarios (LSM vs VIPER)

use std::collections::HashMap;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use proximadb::core::search::{
    MultiTierDeduplicator, StorageTier, StorageEngine, TieredSearchResult, MetadataFilter
};
use proximadb::core::VectorRecord;

/// Helper function to create a test vector record
fn create_test_vector(
    id: &str,
    collection_id: &str,
    vector: Vec<f32>,
    version: u64,
    timestamp_ms: i64,
    metadata: HashMap<String, serde_json::Value>,
) -> VectorRecord {
    VectorRecord {
        id: id.to_string(),
        collection_id: collection_id.to_string(),
        vector,
        metadata,
        timestamp: timestamp_ms,
        created_at: timestamp_ms,
        updated_at: timestamp_ms,
        expires_at: None,
        version,
        rank: None,
        score: None,
        distance: None,
    }
}

/// Helper function to create a tiered search result
fn create_tiered_result(
    vector_record: VectorRecord,
    score: f32,
    tier: StorageTier,
    engine: StorageEngine,
    sequence: u64,
    timestamp: DateTime<Utc>,
    file_path: Option<String>,
) -> TieredSearchResult {
    TieredSearchResult {
        vector_record,
        score,
        tier,
        engine,
        timestamp,
        sequence,
        file_path,
    }
}

#[test]
fn test_multiple_unflushed_upserts_same_batch() {
    let mut deduplicator = MultiTierDeduplicator::new();
    let base_time = Utc::now();
    
    // Simulate multiple upserts of same vector in single unflushed batch
    let results = vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.0, 2.0], 1, base_time.timestamp_millis(), HashMap::new()),
            0.5, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time, None
        ),
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.1, 2.1], 2, (base_time + Duration::milliseconds(100)).timestamp_millis(), HashMap::new()),
            0.4, StorageTier::Unflushed, StorageEngine::WAL, 101, base_time + Duration::milliseconds(100), None
        ),
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.2, 2.2], 3, (base_time + Duration::milliseconds(200)).timestamp_millis(), HashMap::new()),
            0.3, StorageTier::Unflushed, StorageEngine::WAL, 102, base_time + Duration::milliseconds(200), None
        ),
    ];
    
    deduplicator.add_tier_results(results);
    let final_results = deduplicator.get_final_results(10);
    
    assert_eq!(final_results.len(), 1);
    assert_eq!(final_results[0].vector_record.id, "vec_1");
    assert_eq!(final_results[0].vector_record.version, 3); // Latest version
    assert_eq!(final_results[0].sequence, 102); // Latest sequence
    assert_eq!(final_results[0].score, 0.3);
    assert_eq!(final_results[0].vector_record.vector, vec![1.2, 2.2]);
}

#[test]
fn test_multiple_unflushed_upserts_different_batches() {
    let mut deduplicator = MultiTierDeduplicator::new();
    let base_time = Utc::now();
    
    // Simulate multiple batches with same vector ID (e.g., concurrent operations)
    // Batch 1
    deduplicator.add_tier_results(vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.0, 2.0], 1, base_time.timestamp_millis(), HashMap::new()),
            0.5, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time, None
        ),
    ]);
    
    // Batch 2 (concurrent)
    deduplicator.add_tier_results(vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.1, 2.1], 2, (base_time + Duration::milliseconds(50)).timestamp_millis(), HashMap::new()),
            0.4, StorageTier::Unflushed, StorageEngine::WAL, 101, base_time + Duration::milliseconds(50), None
        ),
    ]);
    
    // Batch 3 (latest)
    deduplicator.add_tier_results(vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.2, 2.2], 3, (base_time + Duration::milliseconds(100)).timestamp_millis(), HashMap::new()),
            0.3, StorageTier::Unflushed, StorageEngine::WAL, 102, base_time + Duration::milliseconds(100), None
        ),
    ]);
    
    let final_results = deduplicator.get_final_results(10);
    
    assert_eq!(final_results.len(), 1);
    assert_eq!(final_results[0].vector_record.version, 3); // Latest version wins
    assert_eq!(final_results[0].sequence, 102); // Latest sequence wins
    assert_eq!(final_results[0].vector_record.vector, vec![1.2, 2.2]);
}

#[test]
fn test_cross_tier_deduplication() {
    let mut deduplicator = MultiTierDeduplicator::new();
    let base_time = Utc::now();
    
    // Add compacted result (oldest)
    deduplicator.add_tier_results(vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.0, 2.0], 1, base_time.timestamp_millis(), HashMap::new()),
            0.9, StorageTier::Compacted, StorageEngine::VIPER, 50, base_time, Some("/data/compacted.parquet".to_string())
        ),
    ]);
    
    // Add flushed result (medium priority)
    deduplicator.add_tier_results(vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.1, 2.1], 2, (base_time + Duration::milliseconds(100)).timestamp_millis(), HashMap::new()),
            0.6, StorageTier::Flushed, StorageEngine::LSM, 75, base_time + Duration::milliseconds(100), Some("/data/flushed.sst".to_string())
        ),
    ]);
    
    // Add unflushed result (highest priority)
    deduplicator.add_tier_results(vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.2, 2.2], 3, (base_time + Duration::milliseconds(200)).timestamp_millis(), HashMap::new()),
            0.3, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time + Duration::milliseconds(200), None
        ),
    ]);
    
    let final_results = deduplicator.get_final_results(10);
    
    assert_eq!(final_results.len(), 1);
    assert_eq!(final_results[0].tier, StorageTier::Unflushed); // Unflushed wins
    assert_eq!(final_results[0].vector_record.version, 3);
    assert_eq!(final_results[0].score, 0.3);
    assert_eq!(final_results[0].vector_record.vector, vec![1.2, 2.2]);
}

#[test]
fn test_same_tier_version_ordering() {
    let mut deduplicator = MultiTierDeduplicator::new();
    let base_time = Utc::now();
    
    // Two unflushed results with same sequence but different versions
    let results = vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.0, 2.0], 1, base_time.timestamp_millis(), HashMap::new()),
            0.5, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time, None
        ),
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.1, 2.1], 3, base_time.timestamp_millis(), HashMap::new()), // Higher version
            0.4, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time, None  // Same sequence
        ),
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.2, 2.2], 2, base_time.timestamp_millis(), HashMap::new()), // Medium version
            0.6, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time, None  // Same sequence
        ),
    ];
    
    deduplicator.add_tier_results(results);
    let final_results = deduplicator.get_final_results(10);
    
    assert_eq!(final_results.len(), 1);
    assert_eq!(final_results[0].vector_record.version, 3); // Highest version wins
    assert_eq!(final_results[0].score, 0.4);
    assert_eq!(final_results[0].vector_record.vector, vec![1.1, 2.1]);
}

#[test]
fn test_same_tier_timestamp_tiebreaker() {
    let mut deduplicator = MultiTierDeduplicator::new();
    let base_time = Utc::now();
    
    // Two unflushed results with same sequence and version but different timestamps
    let results = vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.0, 2.0], 1, base_time.timestamp_millis(), HashMap::new()),
            0.5, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time, None
        ),
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.1, 2.1], 1, base_time.timestamp_millis(), HashMap::new()), // Same version
            0.4, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time + Duration::milliseconds(100), None  // Later timestamp
        ),
    ];
    
    deduplicator.add_tier_results(results);
    let final_results = deduplicator.get_final_results(10);
    
    assert_eq!(final_results.len(), 1);
    assert_eq!(final_results[0].timestamp, base_time + Duration::milliseconds(100)); // Later timestamp wins
    assert_eq!(final_results[0].score, 0.4);
    assert_eq!(final_results[0].vector_record.vector, vec![1.1, 2.1]);
}

#[test]
fn test_metadata_filtering() {
    let mut metadata_filters = HashMap::new();
    metadata_filters.insert("category".to_string(), json!("important"));
    metadata_filters.insert("status".to_string(), json!("active"));
    
    let mut deduplicator = MultiTierDeduplicator::with_filters(metadata_filters);
    let base_time = Utc::now();
    
    // Create metadata that matches filter
    let mut matching_metadata = HashMap::new();
    matching_metadata.insert("category".to_string(), json!("important"));
    matching_metadata.insert("status".to_string(), json!("active"));
    
    // Create metadata that doesn't match filter
    let mut non_matching_metadata = HashMap::new();
    non_matching_metadata.insert("category".to_string(), json!("unimportant"));
    non_matching_metadata.insert("status".to_string(), json!("inactive"));
    
    let results = vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.0, 2.0], 1, base_time.timestamp_millis(), matching_metadata),
            0.3, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time, None
        ),
        create_tiered_result(
            create_test_vector("vec_2", "collection_a", vec![2.0, 3.0], 1, base_time.timestamp_millis(), non_matching_metadata),
            0.2, StorageTier::Unflushed, StorageEngine::WAL, 101, base_time, None
        ),
    ];
    
    deduplicator.add_tier_results(results);
    let final_results = deduplicator.get_final_results(10);
    
    assert_eq!(final_results.len(), 1); // Only matching vector
    assert_eq!(final_results[0].vector_record.id, "vec_1");
    assert_eq!(final_results[0].score, 0.3);
}

#[test]
fn test_records_without_ids() {
    let mut deduplicator = MultiTierDeduplicator::new();
    let base_time = Utc::now();
    
    // Mix of records with and without IDs
    let results = vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.0, 2.0], 1, base_time.timestamp_millis(), HashMap::new()),
            0.5, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time, None
        ),
        create_tiered_result(
            create_test_vector("", "collection_a", vec![2.0, 3.0], 1, base_time.timestamp_millis(), HashMap::new()), // No ID
            0.4, StorageTier::Unflushed, StorageEngine::WAL, 101, base_time, None
        ),
        create_tiered_result(
            create_test_vector("", "collection_a", vec![3.0, 4.0], 1, base_time.timestamp_millis(), HashMap::new()), // No ID
            0.3, StorageTier::Unflushed, StorageEngine::WAL, 102, base_time, None
        ),
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.1, 2.1], 2, (base_time + Duration::milliseconds(100)).timestamp_millis(), HashMap::new()), // Same ID, newer version
            0.2, StorageTier::Unflushed, StorageEngine::WAL, 103, base_time + Duration::milliseconds(100), None
        ),
    ];
    
    deduplicator.add_tier_results(results);
    let final_results = deduplicator.get_final_results(10);
    
    assert_eq!(final_results.len(), 3); // 1 deduplicated ID + 2 without IDs
    
    // Check that the vector with ID was deduplicated to latest version
    let vec_1_result = final_results.iter().find(|r| r.vector_record.id == "vec_1").unwrap();
    assert_eq!(vec_1_result.vector_record.version, 2); // Latest version
    assert_eq!(vec_1_result.vector_record.vector, vec![1.1, 2.1]);
    
    // Check that vectors without IDs are both present
    let no_id_results: Vec<_> = final_results.iter().filter(|r| r.vector_record.id.is_empty()).collect();
    assert_eq!(no_id_results.len(), 2);
}

#[test]
fn test_cross_engine_scenarios() {
    let mut deduplicator = MultiTierDeduplicator::new();
    let base_time = Utc::now();
    
    // Same vector across different storage engines
    let results = vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.0, 2.0], 1, base_time.timestamp_millis(), HashMap::new()),
            0.7, StorageTier::Compacted, StorageEngine::VIPER, 50, base_time, Some("/data/viper.parquet".to_string())
        ),
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.1, 2.1], 2, (base_time + Duration::milliseconds(100)).timestamp_millis(), HashMap::new()),
            0.5, StorageTier::Flushed, StorageEngine::LSM, 75, base_time + Duration::milliseconds(100), Some("/data/lsm.sst".to_string())
        ),
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.2, 2.2], 3, (base_time + Duration::milliseconds(200)).timestamp_millis(), HashMap::new()),
            0.3, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time + Duration::milliseconds(200), None
        ),
    ];
    
    deduplicator.add_tier_results(results);
    let final_results = deduplicator.get_final_results(10);
    
    assert_eq!(final_results.len(), 1);
    assert_eq!(final_results[0].engine, StorageEngine::WAL); // WAL (unflushed) wins
    assert_eq!(final_results[0].tier, StorageTier::Unflushed);
    assert_eq!(final_results[0].vector_record.version, 3);
    assert_eq!(final_results[0].file_path, None); // WAL doesn't have file path
}

#[test]
fn test_deduplication_stats() {
    let mut deduplicator = MultiTierDeduplicator::new();
    let base_time = Utc::now();
    
    let results = vec![
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.0, 2.0], 1, base_time.timestamp_millis(), HashMap::new()),
            0.5, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time, None
        ),
        create_tiered_result(
            create_test_vector("vec_1", "collection_a", vec![1.1, 2.1], 2, (base_time + Duration::milliseconds(100)).timestamp_millis(), HashMap::new()),
            0.4, StorageTier::Unflushed, StorageEngine::WAL, 101, base_time + Duration::milliseconds(100), None
        ),
        create_tiered_result(
            create_test_vector("", "collection_a", vec![2.0, 3.0], 1, base_time.timestamp_millis(), HashMap::new()), // No ID
            0.3, StorageTier::Unflushed, StorageEngine::WAL, 102, base_time, None
        ),
        create_tiered_result(
            create_test_vector("vec_2", "collection_a", vec![3.0, 4.0], 1, base_time.timestamp_millis(), HashMap::new()),
            0.2, StorageTier::Unflushed, StorageEngine::WAL, 103, base_time, None
        ),
    ];
    
    deduplicator.add_tier_results(results);
    let stats = deduplicator.get_stats();
    
    assert_eq!(stats.unique_ids, 2); // vec_1 and vec_2
    assert_eq!(stats.records_without_id, 1); // One record without ID
    assert_eq!(stats.total_records, 3); // 2 unique IDs + 1 without ID
    
    let final_results = deduplicator.get_final_results(10);
    assert_eq!(final_results.len(), 3);
}

#[test]
fn test_complex_concurrent_scenario() {
    let mut deduplicator = MultiTierDeduplicator::new();
    let base_time = Utc::now();
    
    // Simulate a complex scenario with multiple vectors, tiers, and concurrent operations
    
    // Batch 1: Initial compacted data
    deduplicator.add_tier_results(vec![
        create_tiered_result(
            create_test_vector("user_123", "users", vec![0.1, 0.2, 0.3], 1, base_time.timestamp_millis(), HashMap::new()),
            0.8, StorageTier::Compacted, StorageEngine::VIPER, 10, base_time, Some("/data/users_compacted.parquet".to_string())
        ),
        create_tiered_result(
            create_test_vector("doc_456", "documents", vec![0.4, 0.5, 0.6], 1, base_time.timestamp_millis(), HashMap::new()),
            0.7, StorageTier::Compacted, StorageEngine::LSM, 11, base_time, Some("/data/docs_compacted.sst".to_string())
        ),
    ]);
    
    // Batch 2: Some flushed updates
    deduplicator.add_tier_results(vec![
        create_tiered_result(
            create_test_vector("user_123", "users", vec![0.11, 0.21, 0.31], 2, (base_time + Duration::milliseconds(1000)).timestamp_millis(), HashMap::new()),
            0.6, StorageTier::Flushed, StorageEngine::LSM, 50, base_time + Duration::milliseconds(1000), Some("/data/users_flushed.sst".to_string())
        ),
    ]);
    
    // Batch 3: Latest unflushed operations (concurrent)
    deduplicator.add_tier_results(vec![
        create_tiered_result(
            create_test_vector("user_123", "users", vec![0.12, 0.22, 0.32], 3, (base_time + Duration::milliseconds(2000)).timestamp_millis(), HashMap::new()),
            0.4, StorageTier::Unflushed, StorageEngine::WAL, 100, base_time + Duration::milliseconds(2000), None
        ),
        create_tiered_result(
            create_test_vector("doc_456", "documents", vec![0.41, 0.51, 0.61], 2, (base_time + Duration::milliseconds(2100)).timestamp_millis(), HashMap::new()),
            0.3, StorageTier::Unflushed, StorageEngine::WAL, 101, base_time + Duration::milliseconds(2100), None
        ),
        create_tiered_result(
            create_test_vector("new_item", "items", vec![0.7, 0.8, 0.9], 1, (base_time + Duration::milliseconds(2200)).timestamp_millis(), HashMap::new()),
            0.2, StorageTier::Unflushed, StorageEngine::WAL, 102, base_time + Duration::milliseconds(2200), None
        ),
    ]);
    
    let final_results = deduplicator.get_final_results(10);
    
    assert_eq!(final_results.len(), 3); // user_123, doc_456, new_item
    
    // Check user_123 - should be latest unflushed version
    let user_result = final_results.iter().find(|r| r.vector_record.id == "user_123").unwrap();
    assert_eq!(user_result.tier, StorageTier::Unflushed);
    assert_eq!(user_result.vector_record.version, 3);
    assert_eq!(user_result.vector_record.vector, vec![0.12, 0.22, 0.32]);
    
    // Check doc_456 - should be latest unflushed version
    let doc_result = final_results.iter().find(|r| r.vector_record.id == "doc_456").unwrap();
    assert_eq!(doc_result.tier, StorageTier::Unflushed);
    assert_eq!(doc_result.vector_record.version, 2);
    assert_eq!(doc_result.vector_record.vector, vec![0.41, 0.51, 0.61]);
    
    // Check new_item - only exists in unflushed
    let new_result = final_results.iter().find(|r| r.vector_record.id == "new_item").unwrap();
    assert_eq!(new_result.tier, StorageTier::Unflushed);
    assert_eq!(new_result.vector_record.version, 1);
    assert_eq!(new_result.vector_record.vector, vec![0.7, 0.8, 0.9]);
}
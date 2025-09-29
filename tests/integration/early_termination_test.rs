use anyhow::Result;
use chrono::Utc;
use proximadb::core::VectorRecord;
use proximadb::core::search::multi_tier_deduplication::{
    DataFreshnessTier, DeduplicationStorageEngine, MultiTierDeduplicator, TieredSearchCandidate,
};

#[test]
fn test_early_termination_logic() -> Result<()> {
    // Test 1: With ordering required (like REST/gRPC search) - no early termination
    {
        let mut dedup = MultiTierDeduplicator::with_k(2);
        dedup.set_requires_ordering(true); // REST/gRPC always need ordering

        let now = Utc::now();
        let mut candidates = vec![];

        // Create 5 candidates with varying scores
        for i in 0..5 {
            candidates.push(TieredSearchCandidate {
                vector_record: VectorRecord {
                    id: format!("vec_{}", i),
                    vector: vec![i as f32],
                    metadata: std::collections::HashMap::new(),
                    timestamp: now.timestamp_millis(),
                    updated_at: Some(now.timestamp_millis()),
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
                similarity: (5 - i) as f32, // Best scores come last
                tier: DataFreshnessTier::Unflushed,
                engine: DeduplicationStorageEngine::WAL,
                timestamp: now,
                sequence: i as u64,
                file_path: None,
            });
        }

        // Add all candidates - should process all despite having k=2
        dedup.add_tier_results(candidates);

        let results = dedup.get_final_results(2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].similarity, 5.0); // Best score (highest)
        assert_eq!(results[1].similarity, 4.0); // Second best
    }

    // Test 2: Without ordering (SQL without ORDER BY) - early termination enabled
    {
        let mut dedup = MultiTierDeduplicator::with_k(2);
        dedup.set_requires_ordering(false); // SQL without ORDER BY

        let now = Utc::now();
        let mut candidates = vec![];

        // Create candidates
        for i in 0..5 {
            candidates.push(TieredSearchCandidate {
                vector_record: VectorRecord {
                    id: format!("vec_{}", i),
                    vector: vec![i as f32],
                    metadata: std::collections::HashMap::new(),
                    timestamp: now.timestamp_millis(),
                    updated_at: Some(now.timestamp_millis()),
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
                similarity: i as f32,
                tier: DataFreshnessTier::Unflushed,
                engine: DeduplicationStorageEngine::WAL,
                timestamp: now,
                sequence: i as u64,
                file_path: None,
            });
        }

        // Add first batch
        dedup.add_tier_results(candidates[0..2].to_vec());
        assert!(dedup.is_early_terminated());

        // Try to add more - should be skipped
        dedup.add_tier_results(candidates[2..5].to_vec());

        let results = dedup.get_final_results(10); // Ask for more than we have
        assert_eq!(results.len(), 2); // Only got 2 due to early termination
    }

    Ok(())
}

#[test]
fn test_sql_query_behavior() -> Result<()> {
    // Simulate SQL query patterns

    // Case 1: SQL with ORDER BY VECTOR_SIMILARITY - requires ordering
    {
        let mut dedup = MultiTierDeduplicator::with_k(10);
        dedup.set_requires_ordering(true); // ORDER BY present
        assert!(!dedup.can_terminate_early());
    }

    // Case 2: SQL without ORDER BY - can terminate early
    {
        let mut dedup = MultiTierDeduplicator::with_k(10);
        dedup.set_requires_ordering(false); // No ORDER BY
        assert!(dedup.can_terminate_early());
    }

    // Case 3: SQL with metadata filter only - can terminate early
    {
        let mut dedup = MultiTierDeduplicator::with_k(100);
        dedup.set_requires_ordering(false);
        assert!(dedup.can_terminate_early());
    }

    Ok(())
}

#[test]
fn test_grpc_rest_always_ordered() -> Result<()> {
    // gRPC and REST endpoints always expect ordered results
    let mut dedup = MultiTierDeduplicator::with_k(25);
    dedup.set_requires_ordering(true); // Always true for gRPC/REST

    assert!(!dedup.can_terminate_early());
    // Field is private, verify through behavior

    Ok(())
}

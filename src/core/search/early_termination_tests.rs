#[cfg(test)]
mod tests {
    use super::super::multi_tier_deduplication::{
        DataFreshnessTier, DeduplicationStorageEngine, MultiTierDeduplicator, TieredSearchCandidate,
    };

    use crate::proto::proximadb_v1::VectorRecord;
    use chrono::Utc;

    fn create_test_vector_record(id: &str, _similarity: f32) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: std::collections::HashMap::new(),
            timestamp: 12345,
            updated_at: Some(12345),
            expires_at: None,
            version: Some(1),
            ..Default::default()
        }
    }

    #[test]
    fn test_early_termination_disabled_with_ordering() {
        let k = 3;
        let mut deduplicator = MultiTierDeduplicator::with_k(k);
        deduplicator.set_requires_ordering(true); // Ordering required - no early termination

        // Add candidates from different tiers
        let candidates = vec![
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec1", 0.9),
                tier: DataFreshnessTier::Unflushed,
                similarity: 0.9,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 10,
                file_path: None,
            },
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec2", 0.8),
                tier: DataFreshnessTier::Flushed,
                similarity: 0.8,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 8,
                file_path: None,
            },
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec3", 0.7),
                tier: DataFreshnessTier::Compacted,
                similarity: 0.7,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 5,
                file_path: Some("file1.sstable".to_string()),
            },
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec4", 0.6),
                tier: DataFreshnessTier::Compacted,
                similarity: 0.6,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 4,
                file_path: Some("file2.sstable".to_string()),
            },
        ];

        deduplicator.add_tier_results(candidates);

        // Should NOT be early terminated since ordering is required
        assert!(!deduplicator.is_early_terminated());

        let results = deduplicator.get_final_results(k);
        assert_eq!(results.len(), 3);

        // Results should be ordered by score
        assert_eq!(results[0].vector_record.id, "vec1");
        assert_eq!(results[1].vector_record.id, "vec2");
        assert_eq!(results[2].vector_record.id, "vec3");
    }

    #[test]
    fn test_early_termination_enabled_without_ordering() {
        let k = 3;
        let mut deduplicator = MultiTierDeduplicator::with_k(k);
        deduplicator.set_requires_ordering(false); // No ordering required - early termination possible

        // Add exactly k candidates
        let candidates = vec![
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec1", 0.9),
                tier: DataFreshnessTier::Unflushed,
                similarity: 0.9,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 10,
                file_path: None,
            },
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec2", 0.8),
                tier: DataFreshnessTier::Flushed,
                similarity: 0.8,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 8,
                file_path: None,
            },
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec3", 0.7),
                tier: DataFreshnessTier::Compacted,
                similarity: 0.7,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 5,
                file_path: Some("file1.sstable".to_string()),
            },
        ];

        deduplicator.add_tier_results(candidates);

        // Should be early terminated since we have k results and no ordering required
        assert!(deduplicator.is_early_terminated());

        let results = deduplicator.get_final_results(k);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_early_termination_with_duplicates() {
        let k = 2;
        let mut deduplicator = MultiTierDeduplicator::with_k(k);
        deduplicator.set_requires_ordering(false); // No ordering required

        // Add duplicates across tiers
        let candidates = vec![
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec1", 0.9),
                tier: DataFreshnessTier::Unflushed,
                similarity: 0.9,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 10,
                file_path: None,
            },
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec1", 0.85), // Duplicate ID
                tier: DataFreshnessTier::Flushed,
                similarity: 0.85,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 8,
                file_path: None,
            },
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec2", 0.7),
                tier: DataFreshnessTier::Compacted,
                similarity: 0.7,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 5,
                file_path: Some("file1.sstable".to_string()),
            },
        ];

        deduplicator.add_tier_results(candidates);

        // Should be early terminated with k unique results
        assert!(deduplicator.is_early_terminated());

        let results = deduplicator.get_final_results(k);
        assert_eq!(results.len(), 2);

        // Should have kept the higher tier version of vec1
        assert_eq!(results[0].vector_record.id, "vec1");
        assert_eq!(results[0].similarity, 0.9); // From Unflushed tier
        assert_eq!(results[1].vector_record.id, "vec2");
    }

    #[test]
    fn test_no_early_termination_when_insufficient_results() {
        let k = 5;
        let mut deduplicator = MultiTierDeduplicator::with_k(k);
        deduplicator.set_requires_ordering(false); // No ordering required

        // Add fewer than k candidates
        let candidates = vec![
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec1", 0.9),
                tier: DataFreshnessTier::Unflushed,
                similarity: 0.9,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 10,
                file_path: None,
            },
            TieredSearchCandidate {
                vector_record: create_test_vector_record("vec2", 0.8),
                tier: DataFreshnessTier::Flushed,
                similarity: 0.8,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: 8,
                file_path: None,
            },
        ];

        deduplicator.add_tier_results(candidates);

        // Should NOT be early terminated since we don't have k results
        assert!(!deduplicator.is_early_terminated());

        let results = deduplicator.get_final_results(k);
        assert_eq!(results.len(), 2); // Only 2 results available
    }
}

//! Unit tests for multi-tier deduplication system

use chrono::{Duration, Utc};
use proximadb::core::VectorRecord;
use proximadb::core::search::multi_tier_deduplication::{
    DeduplicationStorageEngine, MultiTierDeduplicator, StorageTier, TieredSearchCandidate,
};
use proximadb::proto::proximadb::MetadataItem;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn test_storage_tier_ordering() {
    // Verify tier priority ordering
    assert!(StorageTier::Unflushed > StorageTier::Flushed);
    assert!(StorageTier::Flushed > StorageTier::Compacted);
    assert!(StorageTier::Unflushed > StorageTier::Compacted);

    // Verify numeric values
    assert_eq!(StorageTier::Compacted as u8, 0);
    assert_eq!(StorageTier::Flushed as u8, 1);
    assert_eq!(StorageTier::Unflushed as u8, 2);
}

#[test]
fn test_basic_deduplication() {
    let mut deduplicator = MultiTierDeduplicator::new();

    // Create a base vector record
    let base_record = VectorRecord {
        id: Some("vec1".to_string()),
        vector: vec![1.0, 0.0, 0.0],
        metadata: vec![MetadataItem {
            key: "type".to_string(),
            value: Some(
                proximadb::proto::proximadb::metadata_item::Value::StringValue("test".to_string()),
            ),
        }],
        timestamp: Utc::now().timestamp() as u32,
        updated_at: Some(Utc::now().timestamp() as u32),
        expires_at: None,
        version: Some(1),
        rank: None,
        score: None,
        distance: None,
    };

    // Add same vector from different tiers
    let results = vec![
        TieredSearchCandidate {
            vector_record: base_record.clone(),
            score: 0.8,
            tier: StorageTier::Compacted,
            engine: DeduplicationStorageEngine::SST,
            timestamp: Utc::now() - Duration::hours(2),
            sequence: 100,
            file_path: Some("/data/compacted.db".to_string()),
        },
        TieredSearchCandidate {
            vector_record: {
                let mut rec = base_record.clone();
                rec.version = Some(2);
                rec
            },
            score: 0.85,
            tier: StorageTier::Flushed,
            engine: DeduplicationStorageEngine::SST,
            timestamp: Utc::now() - Duration::hours(1),
            sequence: 200,
            file_path: Some("/data/flushed.db".to_string()),
        },
    ];

    deduplicator.add_tier_results(results);
    let merged = deduplicator.get_final_results(10);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].vector_record.version, Some(2)); // Should get the newer version
    assert_eq!(merged[0].score, 0.85);
}

#[test]
fn test_deduplication_without_ids() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let mut deduplicator = MultiTierDeduplicator::new();

    // Create vectors without IDs (immutable vectors)
    let results = vec![
        TieredSearchCandidate {
            vector_record: VectorRecord {
                id: None,
                vector: vec![1.0, 0.0, 0.0],
                metadata: vec![],
                timestamp: Utc::now().timestamp() as u32,
                updated_at: Some(Utc::now().timestamp() as u32),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            },
            score: 0.9,
            tier: StorageTier::Flushed,
            engine: DeduplicationStorageEngine::VIPER,
            timestamp: Utc::now(),
            sequence: 100,
            file_path: Some("/data/viper/vectors.parquet".to_string()),
        },
        TieredSearchCandidate {
            vector_record: VectorRecord {
                id: None,
                vector: vec![0.0, 1.0, 0.0],
                metadata: vec![],
                timestamp: Utc::now().timestamp() as u32,
                updated_at: Some(Utc::now().timestamp() as u32),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            },
            score: 0.85,
            tier: StorageTier::Flushed,
            engine: DeduplicationStorageEngine::VIPER,
            timestamp: Utc::now(),
            sequence: 101,
            file_path: Some("/data/viper/vectors.parquet".to_string()),
        },
    ];

    deduplicator.add_tier_results(results);
    let merged = deduplicator.get_final_results(10);

    // Both vectors should be included (no deduplication for ID-less vectors)
    // Results are sorted by score in descending order (highest score first)
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].score, 0.9); // Highest score comes first
    assert_eq!(merged[1].score, 0.85); // Lower score comes second
}

#[test]
fn test_metadata_filtering() {
    let mut deduplicator = MultiTierDeduplicator::with_filters({
        let mut filters = HashMap::new();
        filters.insert("category".to_string(), json!("science"));
        filters.insert("published".to_string(), json!("true")); // String comparison
        filters
    });

    let records = vec![
        VectorRecord {
            id: Some("doc1".to_string()),
            vector: vec![1.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            "science".to_string(),
                        ),
                    ),
                },
                MetadataItem {
                    key: "published".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            "true".to_string(),
                        ),
                    ),
                },
            ],
            timestamp: Utc::now().timestamp() as u32,
            updated_at: Some(Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        },
        VectorRecord {
            id: Some("doc2".to_string()),
            vector: vec![0.0, 1.0],
            metadata: vec![
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            "history".to_string(),
                        ),
                    ),
                },
                MetadataItem {
                    key: "published".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            "true".to_string(),
                        ),
                    ),
                },
            ],
            timestamp: Utc::now().timestamp() as u32,
            updated_at: Some(Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        },
    ];

    let results: Vec<TieredSearchCandidate> = records
        .into_iter()
        .enumerate()
        .map(|(i, record)| TieredSearchCandidate {
            vector_record: record,
            score: 0.9 - (i as f32 * 0.1),
            tier: StorageTier::Flushed,
            engine: DeduplicationStorageEngine::SST,
            timestamp: Utc::now(),
            sequence: i as u64,
            file_path: None,
        })
        .collect();

    deduplicator.add_tier_results(results);
    let merged = deduplicator.get_final_results(10);

    // Only doc1 should match the filters
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].vector_record.id, Some("doc1".to_string()));
}

#[test]
fn test_simple_metadata_query() {
    // Test with simple filters only (logical queries can be tested separately)
    let mut deduplicator = MultiTierDeduplicator::with_filters({
        let mut filters = HashMap::new();
        filters.insert("language".to_string(), json!("en"));
        filters
    });

    let records = vec![
        VectorRecord {
            id: Some("doc1".to_string()),
            vector: vec![1.0, 0.0],
            metadata: vec![
                MetadataItem {
                    key: "language".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            "en".to_string(),
                        ),
                    ),
                },
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            "tech".to_string(),
                        ),
                    ),
                },
            ],
            timestamp: Utc::now().timestamp() as u32,
            updated_at: Some(Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        },
        VectorRecord {
            id: Some("doc2".to_string()),
            vector: vec![0.0, 1.0],
            metadata: vec![
                MetadataItem {
                    key: "language".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            "fr".to_string(),
                        ),
                    ),
                },
                MetadataItem {
                    key: "category".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            "tech".to_string(),
                        ),
                    ),
                },
            ],
            timestamp: Utc::now().timestamp() as u32,
            updated_at: Some(Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        },
    ];

    let results: Vec<TieredSearchCandidate> = records
        .into_iter()
        .enumerate()
        .map(|(i, record)| TieredSearchCandidate {
            vector_record: record,
            score: 0.9 - (i as f32 * 0.1),
            tier: StorageTier::Flushed,
            engine: DeduplicationStorageEngine::VIPER,
            timestamp: Utc::now(),
            sequence: i as u64,
            file_path: None,
        })
        .collect();

    deduplicator.add_tier_results(results);
    let merged = deduplicator.get_final_results(10);

    // Only doc1 should match (language=en)
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].vector_record.id, Some("doc1".to_string()));
}

#[test]
fn test_mixed_engine_deduplication() {
    let mut deduplicator = MultiTierDeduplicator::new();

    let base_record = VectorRecord {
        id: Some("vec1".to_string()),
        vector: vec![1.0, 0.0, 0.0],
        metadata: vec![],
        timestamp: Utc::now().timestamp() as u32,
        updated_at: Some(Utc::now().timestamp() as u32),
        expires_at: None,
        version: Some(1),
        rank: None,
        score: None,
        distance: None,
    };

    // Add results from different engines
    let results = vec![
        TieredSearchCandidate {
            vector_record: base_record.clone(),
            score: 0.8,
            tier: StorageTier::Compacted,
            engine: DeduplicationStorageEngine::SST,
            timestamp: Utc::now() - Duration::hours(2),
            sequence: 100,
            file_path: Some("/data/lsm/compacted.db".to_string()),
        },
        TieredSearchCandidate {
            vector_record: {
                let mut rec = base_record.clone();
                rec.version = Some(2);
                rec
            },
            score: 0.85,
            tier: StorageTier::Compacted,
            engine: DeduplicationStorageEngine::VIPER,
            timestamp: Utc::now() - Duration::hours(1),
            sequence: 200,
            file_path: Some("/data/viper/cluster.parquet".to_string()),
        },
        TieredSearchCandidate {
            vector_record: {
                let mut rec = base_record.clone();
                rec.version = Some(3);
                rec
            },
            score: 0.9,
            tier: StorageTier::Unflushed,
            engine: DeduplicationStorageEngine::WAL,
            timestamp: Utc::now(),
            sequence: 300,
            file_path: None,
        },
    ];

    deduplicator.add_tier_results(results);
    let merged = deduplicator.get_final_results(10);

    // Should get the unflushed WAL version (highest priority)
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].vector_record.version, Some(3));
    assert_eq!(merged[0].score, 0.9);
}

#[test]
fn test_k_limit_enforcement() {
    let mut deduplicator = MultiTierDeduplicator::new();

    // Add 20 unique results
    let mut results = Vec::new();
    for i in 0..20 {
        results.push(TieredSearchCandidate {
            vector_record: VectorRecord {
                id: Some(format!("vec{}", i)),
                vector: vec![i as f32, 0.0, 0.0],
                metadata: vec![],
                timestamp: Utc::now().timestamp() as u32,
                updated_at: Some(Utc::now().timestamp() as u32),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            },
            score: (i as f32 * 0.01), // Increasing scores (ascending order)
            tier: StorageTier::Flushed,
            engine: DeduplicationStorageEngine::SST,
            timestamp: Utc::now(),
            sequence: i as u64,
            file_path: Some(format!("/data/file_{}.db", i)),
        });
    }

    deduplicator.add_tier_results(results);

    // Request only top 10
    let merged = deduplicator.get_final_results(10);

    assert_eq!(merged.len(), 10);
    // Results are sorted by score in descending order (highest score first)
    // Top 10 results should be vec19 (0.19) to vec10 (0.10)
    assert_eq!(merged[0].vector_record.id, Some("vec19".to_string())); // Highest score (0.19)
    assert_eq!(
        merged[merged.len() - 1].vector_record.id,
        Some("vec10".to_string())
    ); // 10th highest score (0.10)
}

#[test]
fn test_complex_deduplication_scenario() {
    let mut deduplicator = MultiTierDeduplicator::new();

    // Scenario: Multiple versions of same vectors across different tiers
    let mut results = Vec::new();

    // Vector A: versions in all tiers
    for (version, tier, engine, hours_ago) in vec![
        (
            1,
            StorageTier::Compacted,
            DeduplicationStorageEngine::SST,
            24,
        ),
        (2, StorageTier::Flushed, DeduplicationStorageEngine::SST, 12),
        (
            3,
            StorageTier::Unflushed,
            DeduplicationStorageEngine::WAL,
            0,
        ),
    ] {
        results.push(TieredSearchCandidate {
            vector_record: VectorRecord {
                id: Some("vecA".to_string()),
                vector: vec![1.0, 0.0, 0.0],
                metadata: vec![MetadataItem {
                    key: "version".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            version.to_string(),
                        ),
                    ),
                }],
                timestamp: Utc::now().timestamp() as u32,
                updated_at: Some(Utc::now().timestamp() as u32),
                expires_at: None,
                version: Some(version),
                rank: None,
                score: None,
                distance: None,
            },
            score: 0.95,
            tier,
            engine,
            timestamp: Utc::now() - Duration::hours(hours_ago),
            sequence: version as u64 * 100,
            file_path: Some(format!("/data/tier_{}.db", version)),
        });
    }

    // Vector B: only in compacted and flushed
    for (version, tier, engine, hours_ago) in vec![
        (
            1,
            StorageTier::Compacted,
            DeduplicationStorageEngine::VIPER,
            20,
        ),
        (
            2,
            StorageTier::Flushed,
            DeduplicationStorageEngine::VIPER,
            8,
        ),
    ] {
        results.push(TieredSearchCandidate {
            vector_record: VectorRecord {
                id: Some("vecB".to_string()),
                vector: vec![0.0, 1.0, 0.0],
                metadata: vec![MetadataItem {
                    key: "version".to_string(),
                    value: Some(
                        proximadb::proto::proximadb::metadata_item::Value::StringValue(
                            version.to_string(),
                        ),
                    ),
                }],
                timestamp: Utc::now().timestamp() as u32,
                updated_at: Some(Utc::now().timestamp() as u32),
                expires_at: None,
                version: Some(version),
                rank: None,
                score: None,
                distance: None,
            },
            score: 0.90,
            tier,
            engine,
            timestamp: Utc::now() - Duration::hours(hours_ago),
            sequence: version as u64 * 100 + 50,
            file_path: Some(format!("/data/viper_{}.parquet", version)),
        });
    }

    // Vector C: no ID (immutable)
    results.push(TieredSearchCandidate {
        vector_record: VectorRecord {
            id: None,
            vector: vec![0.0, 0.0, 1.0],
            metadata: vec![],
            timestamp: Utc::now().timestamp() as u32,
            updated_at: Some(Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        },
        score: 0.85,
        tier: StorageTier::Flushed,
        engine: DeduplicationStorageEngine::VIPER,
        timestamp: Utc::now() - Duration::hours(4),
        sequence: 1000,
        file_path: Some("/data/immutable.parquet".to_string()),
    });

    deduplicator.add_tier_results(results);
    let merged = deduplicator.get_final_results(10);

    // Should get:
    // - vecA version 3 (unflushed)
    // - vecB version 2 (flushed)
    // - vecC (no ID)
    assert_eq!(merged.len(), 3);

    // Verify vecA is version 3
    let vec_a = merged
        .iter()
        .find(|r| r.vector_record.id == Some("vecA".to_string()))
        .unwrap();
    assert_eq!(vec_a.vector_record.version, Some(3));

    // Verify vecB is version 2
    let vec_b = merged
        .iter()
        .find(|r| r.vector_record.id == Some("vecB".to_string()))
        .unwrap();
    assert_eq!(vec_b.vector_record.version, Some(2));

    // Verify vecC is included
    let vec_c = merged
        .iter()
        .find(|r| r.vector_record.id.is_none())
        .unwrap();
    assert_eq!(vec_c.score, 0.85);
}

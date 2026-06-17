// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! HMGI Integration Tests
//!
//! End-to-end tests for Hierarchical Multi-modality Graph Indexing.

use std::collections::HashMap;
use std::sync::Arc;

use proximadb::index::axis::hmgi::{
    ClusterNode, CollectionTransition, DistributedPartitionLocator, EnablementReason,
    HmgiMigrationEngine, HmgiMigrationPhase, HmgiPartitionKey, HmgiQueryCoordinator, HmgiRegistry,
    HmgiRouter, HmgiTierPolicy, MigrationConfig, MockNetworkService, ModalityDetector,
    ModalityExtractor, NodeState, VectorRecordSample,
};
use proximadb::index::axis::indexes::hnsw_index::AxisHnswConfig;
use proximadb::index::axis::management::{
    HybridQuery, MetadataFilter, ScoredResult, VectorQuery, manager::AxisManager,
};
use proximadb::infrastructure::tier_policy_engine::InfrastructureTier;
use serde_json::json;

fn default_hnsw_config() -> AxisHnswConfig {
    AxisHnswConfig::default()
}

fn nvme_tier() -> InfrastructureTier {
    InfrastructureTier::NvmeSsd {
        mount_path: "/data/nvme".to_string(),
    }
}

fn vector_record(
    id: &str,
    vector: Vec<f32>,
    modality: &str,
) -> proximadb::proto::proximadb_v1::VectorRecord {
    use proximadb::proto::proximadb_v1::{SqlValue, sql_value};

    let mut metadata = HashMap::new();
    metadata.insert(
        "_modality".to_string(),
        SqlValue {
            value: Some(sql_value::Value::StringValue(modality.to_string())),
        },
    );

    proximadb::proto::proximadb_v1::VectorRecord {
        id: id.to_string(),
        vector,
        metadata,
        timestamp: None,
        updated_at: None,
        expires_at: None,
        version: None,
        source: None,
    }
}

/// Test end-to-end HMGI workflow
#[tokio::test]
async fn test_hmgi_end_to_end_workflow() {
    // Create registry and extractor
    let registry = Arc::new(HmgiRegistry::new());
    let extractor = Arc::new(ModalityExtractor::with_config(
        "_modality".to_string(),
        "default".to_string(),
    ));

    // Create router
    let _router = HmgiRouter::new(registry.clone(), extractor.clone());

    // Create partition keys
    let text_key = HmgiPartitionKey::new(123, 1, "text".to_string(), None);
    let image_key = HmgiPartitionKey::new(123, 1, "image".to_string(), None);

    // Get or create partitions
    let _text_partition = registry
        .get_or_create_partition(text_key.clone(), default_hnsw_config(), 3)
        .await
        .unwrap();
    let _image_partition = registry
        .get_or_create_partition(image_key.clone(), default_hnsw_config(), 3)
        .await
        .unwrap();

    // Verify partitions were created
    assert!(registry.get_partition(&text_key).await.is_some());
    assert!(registry.get_partition(&image_key).await.is_some());

    // Check collection partitions
    registry
        .register_collection_partition("test_collection", text_key.clone())
        .await;
    registry
        .register_collection_partition("test_collection", image_key.clone())
        .await;
    let partitions = registry
        .get_partitions_for_collection("test_collection")
        .await;
    assert!(partitions.contains(&text_key));

    // Drop collection partitions
    registry
        .drop_collection_partitions("test_collection")
        .await
        .unwrap();
    assert!(registry.get_partition(&text_key).await.is_none());
}

/// Test modality detection for auto-enablement
#[tokio::test]
async fn test_hmgi_modality_detection() {
    let detector = ModalityDetector::default_config();

    // Single modality - should not enable HMGI
    let single_modality = vec![
        VectorRecordSample::with_modality("text"),
        VectorRecordSample::with_modality("text"),
        VectorRecordSample::with_modality("text"),
    ];

    let result = detector
        .detect_modalities("single_collection", &single_modality)
        .await;

    assert_eq!(result.distinct_modalities, 1);
    assert!(!result.should_enable_hmgi);
    assert_eq!(result.reason, EnablementReason::SingleModality);

    // Multi-modality - should enable HMGI
    let multi_modality = vec![
        VectorRecordSample::with_modality("text"),
        VectorRecordSample::with_modality("image"),
        VectorRecordSample::with_modality("video"),
    ];

    let result = detector
        .detect_modalities("multi_collection", &multi_modality)
        .await;

    assert_eq!(result.distinct_modalities, 3);
    assert!(result.should_enable_hmgi);
    assert_eq!(result.reason, EnablementReason::MultipleModalities);
    assert!(result.confidence > 0.9);
}

/// Test collection transition detection
#[tokio::test]
async fn test_hmgi_collection_transition() {
    let detector = ModalityDetector::default_config();

    // Initially single modality
    let vectors = vec![VectorRecordSample::with_modality("text")];
    let transition = detector
        .recheck_collection("transition_collection", &vectors)
        .await
        .unwrap();

    assert_eq!(transition, CollectionTransition::SingleModality);

    // Transition to multi-modality
    let vectors = vec![
        VectorRecordSample::with_modality("text"),
        VectorRecordSample::with_modality("image"),
    ];

    let transition = detector
        .recheck_collection("transition_collection", &vectors)
        .await
        .unwrap();

    match transition {
        CollectionTransition::MultiModality {
            recommended,
            modalities,
        } => {
            assert!(recommended);
            assert!(modalities.contains(&"text".to_string()));
            assert!(modalities.contains(&"image".to_string()));
        }
        _ => panic!("Expected MultiModality transition"),
    }
}

/// Test modality extraction with fallback
#[tokio::test]
async fn test_hmgi_modality_extraction() {
    let extractor = ModalityExtractor::with_config("_modality".to_string(), "default".to_string());

    // Explicit modality tag
    let mut metadata = HashMap::new();
    metadata.insert("_modality".to_string(), json!("text"));
    assert_eq!(extractor.extract_modality(&metadata), "text");

    // Missing tag - use fallback
    let metadata = HashMap::new();
    assert_eq!(extractor.extract_modality(&metadata), "default");

    // Custom fallback
    let extractor = ModalityExtractor::with_config("_modality".to_string(), "unknown".to_string());
    let metadata = HashMap::new();
    assert_eq!(extractor.extract_modality(&metadata), "unknown");
}

/// Test partition key properties
#[test]
fn test_hmgi_partition_key() {
    let key1 = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
    let key2 = HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456));
    let key3 = HmgiPartitionKey::new(123, 1, "image".to_string(), Some(456));

    assert_eq!(key1, key2);
    assert_ne!(key1, key3);

    // Test hashing for consistent routing
    assert_eq!(key1.routing_hash(), key2.routing_hash());
    assert_ne!(key1.routing_hash(), key3.routing_hash());
}

/// Test partition set filtering
#[test]
fn test_hmgi_partition_set() {
    use proximadb::index::axis::hmgi::PartitionSet;

    let mut set = PartitionSet::new();

    set.insert(HmgiPartitionKey::new(123, 1, "text".to_string(), Some(456)));
    set.insert(HmgiPartitionKey::new(
        123,
        1,
        "image".to_string(),
        Some(456),
    ));
    set.insert(HmgiPartitionKey::new(
        123,
        1,
        "video".to_string(),
        Some(456),
    ));

    assert_eq!(set.len(), 3);

    // Filter by modality
    let text_only = set.for_modality("text");
    assert_eq!(text_only.len(), 1);

    // Filter by multiple modalities
    let text_and_image = set.for_modalities(&["text".to_string(), "image".to_string()]);
    assert_eq!(text_and_image.len(), 2);

    // Filter by tenant
    let tenant456 = set.for_tenant(Some(456));
    assert_eq!(tenant456.len(), 3);

    let tenant789 = set.for_tenant(Some(789));
    assert_eq!(tenant789.len(), 0);
}

/// Test AxisManager HMGI enablement
#[tokio::test]
async fn test_axismanager_hmgi_enablement() {
    use proximadb::index::axis::types::AxisConfig;

    let config = AxisConfig::default();
    let mut manager = AxisManager::new(config).await.unwrap();

    // Initialize HMGI
    manager.init_hmgi(Some("_modality".to_string())).unwrap();

    // Enable HMGI for a collection
    manager
        .enable_hmgi("test_collection", Some("_modality".to_string()), 123)
        .await
        .unwrap();

    // Verify HMGI is enabled
    assert!(manager.is_hmgi_enabled("test_collection").await);

    // Disable HMGI
    manager.disable_hmgi("test_collection").await.unwrap();

    assert!(!manager.is_hmgi_enabled("test_collection").await);
}

/// Test AxisManager HMGI insert
#[tokio::test]
async fn test_axismanager_hmgi_insert() {
    use proximadb::index::axis::types::AxisConfig;

    let config = AxisConfig::default();
    let mut manager = AxisManager::new(config).await.unwrap();

    // Initialize and enable HMGI
    manager.init_hmgi(Some("_modality".to_string())).unwrap();
    manager
        .enable_hmgi("test_collection", Some("_modality".to_string()), 123)
        .await
        .unwrap();

    // Create a vector record with modality metadata
    let record = vector_record("vec1", vec![0.1, 0.2, 0.3], "text");

    // Insert with HMGI (this will route to the text partition)
    let result = manager.insert_hmgi("test_collection", record, None).await;

    // Should succeed and return the partition key
    assert!(result.is_ok());
    let partition_key = result.unwrap();
    assert_eq!(partition_key.modality_tag, "text");
}

/// Test AxisManager uses HMGI as the canonical dense vector index path.
#[tokio::test]
async fn test_axismanager_hmgi_auto_enable_on_dense_insert() {
    use proximadb::index::axis::types::AxisConfig;

    let manager = AxisManager::new(AxisConfig::default()).await.unwrap();
    assert!(!manager.is_hmgi_enabled("auto_collection").await);

    manager
        .insert(
            "auto_collection",
            &vector_record("text_vec", vec![1.0, 0.0, 0.0], "text"),
        )
        .await
        .unwrap();
    assert!(manager.is_hmgi_enabled("auto_collection").await);

    manager
        .insert(
            "auto_collection",
            &vector_record("image_vec", vec![0.0, 1.0, 0.0], "image"),
        )
        .await
        .unwrap();

    assert!(manager.is_hmgi_enabled("auto_collection").await);
    let partitions = manager
        .hmgi_registry()
        .unwrap()
        .get_partitions_for_collection("auto_collection")
        .await;
    assert_eq!(partitions.len(), 2);
    assert!(partitions.iter().any(|p| p.modality_tag == "text"));
    assert!(partitions.iter().any(|p| p.modality_tag == "image"));
}

/// Test canonical HMGI handles a single-modality collection without monolithic HNSW/IVF.
#[tokio::test]
async fn test_axismanager_hmgi_single_modality_query_without_manual_enable() {
    use proximadb::index::axis::types::AxisConfig;

    let manager = AxisManager::new(AxisConfig::default()).await.unwrap();
    manager
        .insert(
            "single_modality_collection",
            &vector_record("text_vec", vec![1.0, 0.0, 0.0], "text"),
        )
        .await
        .unwrap();

    let result = manager
        .query(HybridQuery {
            collection_id: "single_modality_collection".to_string(),
            vector_query: Some(VectorQuery::Dense {
                vector: vec![1.0, 0.0, 0.0],
                similarity_threshold: 0.0,
            }),
            metadata_filters: Vec::new(),
            id_filters: Vec::new(),
            top_k: 10,
            include_expired: false,
            ann_filtering_mode: Default::default(),
            ann_filtering_policy: None,
            estimated_selectivity: None,
            search_effort: None,
        })
        .await
        .unwrap();

    assert!(manager.is_hmgi_enabled("single_modality_collection").await);
    assert_eq!(result.results.len(), 1);
    assert_eq!(result.results[0].vector_id, "text_vec");
}

/// Test AXIS dense deletes remove vectors from HMGI partitions.
#[tokio::test]
async fn test_axismanager_hmgi_delete_removes_partition_vector() {
    use proximadb::index::axis::types::AxisConfig;

    let manager = AxisManager::new(AxisConfig::default()).await.unwrap();
    manager
        .insert(
            "delete_collection",
            &vector_record("delete_vec", vec![1.0, 0.0, 0.0], "text"),
        )
        .await
        .unwrap();

    manager
        .delete("delete_collection", "delete_vec".to_string())
        .await
        .unwrap();

    let result = manager
        .query(HybridQuery {
            collection_id: "delete_collection".to_string(),
            vector_query: Some(VectorQuery::Dense {
                vector: vec![1.0, 0.0, 0.0],
                similarity_threshold: 0.0,
            }),
            metadata_filters: Vec::new(),
            id_filters: Vec::new(),
            top_k: 10,
            include_expired: false,
            ann_filtering_mode: Default::default(),
            ann_filtering_policy: None,
            estimated_selectivity: None,
            search_effort: None,
        })
        .await
        .unwrap();

    assert!(result.results.is_empty());
}

/// Test collection deletion cleans up HMGI state with other AXIS indexes.
#[tokio::test]
async fn test_axismanager_drop_collection_removes_hmgi_partitions() {
    use proximadb::index::axis::types::AxisConfig;

    let manager = AxisManager::new(AxisConfig::default()).await.unwrap();
    manager
        .insert(
            "drop_collection",
            &vector_record("drop_vec", vec![1.0, 0.0, 0.0], "text"),
        )
        .await
        .unwrap();

    assert!(manager.is_hmgi_enabled("drop_collection").await);
    assert!(
        !manager
            .hmgi_registry()
            .unwrap()
            .get_partitions_for_collection("drop_collection")
            .await
            .is_empty()
    );

    manager.drop_collection("drop_collection").await.unwrap();

    assert!(!manager.is_hmgi_enabled("drop_collection").await);
    assert!(
        manager
            .hmgi_registry()
            .unwrap()
            .get_partitions_for_collection("drop_collection")
            .await
            .is_empty()
    );
}

/// Test AxisManager routes HMGI-safe vector queries to modality partitions.
#[tokio::test]
async fn test_axismanager_hmgi_query_routes_to_modality_partition() {
    use proximadb::index::axis::management::FilterOperator;
    use proximadb::index::axis::types::AxisConfig;

    let mut manager = AxisManager::new(AxisConfig::default()).await.unwrap();
    manager.init_hmgi(Some("_modality".to_string())).unwrap();
    manager
        .enable_hmgi("query_collection", Some("_modality".to_string()), 123)
        .await
        .unwrap();

    manager
        .insert_hmgi(
            "query_collection",
            vector_record("text_vec", vec![1.0, 0.0, 0.0], "text"),
            None,
        )
        .await
        .unwrap();
    manager
        .insert_hmgi(
            "query_collection",
            vector_record("image_vec", vec![0.0, 1.0, 0.0], "image"),
            None,
        )
        .await
        .unwrap();

    let result = manager
        .query(HybridQuery {
            collection_id: "query_collection".to_string(),
            vector_query: Some(VectorQuery::Dense {
                vector: vec![1.0, 0.0, 0.0],
                similarity_threshold: 0.0,
            }),
            metadata_filters: vec![MetadataFilter {
                field: "_modality".to_string(),
                operator: FilterOperator::Equals,
                value: json!("text"),
            }],
            id_filters: Vec::new(),
            top_k: 10,
            include_expired: false,
            ann_filtering_mode: Default::default(),
            ann_filtering_policy: None,
            estimated_selectivity: None,
            search_effort: None,
        })
        .await
        .unwrap();

    assert_eq!(result.results.len(), 1);
    assert_eq!(result.results[0].vector_id, "text_vec");
}

/// Test HMGI query routing
#[tokio::test]
async fn test_hmgi_query_routing() {
    let registry = Arc::new(HmgiRegistry::new());
    let extractor = Arc::new(ModalityExtractor::with_config(
        "_modality".to_string(),
        "default".to_string(),
    ));
    let router = HmgiRouter::new(registry.clone(), extractor);

    // Create partitions
    let text_key = HmgiPartitionKey::new(123, 1, "text".to_string(), None);
    let image_key = HmgiPartitionKey::new(123, 1, "image".to_string(), None);
    registry
        .get_or_create_partition(text_key.clone(), default_hnsw_config(), 3)
        .await
        .unwrap();
    registry
        .get_or_create_partition(image_key.clone(), default_hnsw_config(), 3)
        .await
        .unwrap();

    // Register collection
    registry
        .register_collection_partition("test_collection", text_key.clone())
        .await;
    registry
        .register_collection_partition("test_collection", image_key.clone())
        .await;

    // Route query with modality filter
    let query = HybridQuery {
        collection_id: "test_collection".to_string(),
        vector_query: Some(VectorQuery::Dense {
            vector: vec![0.1, 0.2, 0.3],
            similarity_threshold: 0.0,
        }),
        metadata_filters: vec![MetadataFilter {
            field: "_modality".to_string(),
            operator: proximadb::index::axis::management::FilterOperator::Equals,
            value: json!("text"),
        }],
        id_filters: Vec::new(),
        top_k: 10,
        include_expired: false,
        ann_filtering_mode: Default::default(),
        ann_filtering_policy: None,
        estimated_selectivity: None,
        search_effort: None,
    };

    use proximadb::index::axis::hmgi::PartitionSet;
    let mut all_partitions = PartitionSet::new();
    all_partitions.insert(text_key);
    all_partitions.insert(image_key);

    let partitions = router
        .route_query("test_collection", &query, all_partitions)
        .await
        .unwrap();

    // Should only route to text partition
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions.iter().next().unwrap().modality_tag, "text");
}

/// Test HMGI modality-aware tier policy
#[tokio::test]
async fn test_hmgi_tier_policy_recommends_hot_and_cold_modalities() {
    let policy = HmgiTierPolicy::default();

    for _ in 0..150 {
        policy.record_access("hot_text", true).await;
    }

    for _ in 0..5 {
        policy.record_access("cold_audio", true).await;
    }

    let hot_tier = policy.select_tier_for_modality("hot_text").await;
    assert!(matches!(hot_tier, InfrastructureTier::Memory));

    let cold_tier = policy.select_tier_for_modality("cold_audio").await;
    assert!(matches!(
        cold_tier,
        InfrastructureTier::CloudStandard { .. }
    ));

    let default_tier = policy.select_tier_for_modality("new_modality").await;
    assert_eq!(default_tier, nvme_tier());
}

/// Test HMGI partition migration updates the shared tier policy
#[tokio::test]
async fn test_hmgi_partition_migration_updates_tier_policy() {
    let registry = Arc::new(HmgiRegistry::new());
    let tier_policy = Arc::new(HmgiTierPolicy::default());
    let engine = HmgiMigrationEngine::with_config(
        registry.clone(),
        tier_policy.clone(),
        MigrationConfig {
            auto_cleanup_source: false,
            ..Default::default()
        },
    );

    let partition_key = HmgiPartitionKey::new(123, 1, "text".to_string(), None);
    registry
        .get_or_create_partition(partition_key.clone(), default_hnsw_config(), 3)
        .await
        .unwrap();

    let result = engine
        .migrate_partition(partition_key.clone(), InfrastructureTier::Memory)
        .await
        .unwrap();

    assert!(result.success);
    assert_eq!(result.partition_key, partition_key);
    assert!(matches!(
        result.from_tier,
        InfrastructureTier::NvmeSsd { .. }
    ));
    assert!(matches!(result.to_tier, InfrastructureTier::Memory));

    let state = engine
        .get_migration_state(&partition_key.to_string())
        .await
        .unwrap();
    assert_eq!(state.phase, HmgiMigrationPhase::Completed);
    assert_eq!(state.progress, 1.0);

    let updated_tier = tier_policy.select_tier_for_modality("text").await;
    assert!(matches!(updated_tier, InfrastructureTier::Memory));
}

/// Test distributed locator maps hash slots to real active cluster node IDs.
#[tokio::test]
async fn test_hmgi_distributed_locator_uses_cluster_node_ids() {
    let locator = DistributedPartitionLocator::new(3, 20);

    for node_id in [10, 20, 30] {
        locator
            .add_node(ClusterNode {
                id: node_id,
                address: format!("127.0.0.1:{}", 7000 + node_id),
                capacity: 1_000,
                load: 0,
                state: NodeState::Active,
            })
            .await
            .unwrap();
    }

    let partitions = vec![
        HmgiPartitionKey::new(123, 1, "text".to_string(), None),
        HmgiPartitionKey::new(123, 1, "image".to_string(), None),
        HmgiPartitionKey::new(123, 1, "video".to_string(), None),
    ];

    let grouped = locator.group_partitions_by_node(&partitions).await;
    assert!(!grouped.is_empty());
    assert!(grouped.keys().all(|node_id| [10, 20, 30].contains(node_id)));

    let first_partition = partitions[0].clone();
    locator
        .register_local_partition(first_partition.clone())
        .await
        .unwrap();

    let (local, remote) = locator.split_local_remote(partitions).await;
    assert!(local.contains(&first_partition));
    assert!(remote.keys().all(|node_id| [10, 20, 30].contains(node_id)));
}

/// Test distributed coordinator merges remote node results into top-k order.
#[tokio::test]
async fn test_hmgi_distributed_coordinator_merges_remote_results() {
    let locator = Arc::new(DistributedPartitionLocator::new(2, 999));
    for node_id in [10, 20] {
        locator
            .add_node(ClusterNode {
                id: node_id,
                address: format!("127.0.0.1:{}", 7100 + node_id),
                capacity: 1_000,
                load: 0,
                state: NodeState::Active,
            })
            .await
            .unwrap();
    }

    let registry = Arc::new(HmgiRegistry::new());
    let mut network = MockNetworkService::default();
    network.mock_results.insert(
        10,
        vec![
            ScoredResult {
                vector_id: "remote_10".to_string(),
                similarity: 0.70,
                expires_at: None,
            },
            ScoredResult {
                vector_id: "remote_10_b".to_string(),
                similarity: 0.50,
                expires_at: None,
            },
        ],
    );
    network.mock_results.insert(
        20,
        vec![
            ScoredResult {
                vector_id: "remote_20_a".to_string(),
                similarity: 0.95,
                expires_at: None,
            },
            ScoredResult {
                vector_id: "remote_20_b".to_string(),
                similarity: 0.60,
                expires_at: None,
            },
        ],
    );

    let partitions: Vec<HmgiPartitionKey> = (0..16)
        .map(|i| HmgiPartitionKey::new(123, 1, format!("modality_{}", i), None))
        .collect();
    let grouped = locator.group_partitions_by_node(&partitions).await;
    assert_eq!(grouped.len(), 2);

    let coordinator = HmgiQueryCoordinator::new(locator, registry, Arc::new(network));

    let results = coordinator
        .distributed_search(
            partitions,
            &VectorQuery::Dense {
                vector: vec![0.1, 0.2, 0.3],
                similarity_threshold: 0.0,
            },
            2,
        )
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].vector_id, "remote_20_a");
    assert!(results[0].similarity >= results[1].similarity);
}

/// Test HMGI sampling with large collections
#[tokio::test]
async fn test_hmgi_large_collection_sampling() {
    let detector = ModalityDetector::new(100, 2); // Sample size of 100

    // Create 10000 vectors with alternating modalities
    let vectors: Vec<VectorRecordSample> = (0..10000)
        .map(|i| {
            let modality = if i % 3 == 0 {
                "text"
            } else if i % 3 == 1 {
                "image"
            } else {
                "video"
            };
            VectorRecordSample::with_modality(modality)
        })
        .collect();

    let result = detector
        .detect_modalities("large_collection", &vectors)
        .await;

    // Should detect all 3 modalities despite sampling
    assert_eq!(result.distinct_modalities, 3);
    assert!(result.should_enable_hmgi);
}

/// Test HMGI threshold configuration
#[tokio::test]
async fn test_hmgi_threshold_configuration() {
    // Threshold of 3 modalities
    let detector = ModalityDetector::new(100, 3);

    let vectors = vec![
        VectorRecordSample::with_modality("text"),
        VectorRecordSample::with_modality("image"),
    ];

    let result = detector.detect_modalities("threshold_test", &vectors).await;

    assert_eq!(result.distinct_modalities, 2);
    assert!(!result.should_enable_hmgi); // Below threshold

    // Add third modality
    let vectors = vec![
        VectorRecordSample::with_modality("text"),
        VectorRecordSample::with_modality("image"),
        VectorRecordSample::with_modality("video"),
    ];

    let result = detector.detect_modalities("threshold_test", &vectors).await;

    assert_eq!(result.distinct_modalities, 3);
    assert!(result.should_enable_hmgi); // At threshold
}

/// Test HMGI empty collection handling
#[tokio::test]
async fn test_hmgi_empty_collection() {
    let detector = ModalityDetector::default_config();

    let vectors: Vec<VectorRecordSample> = vec![];
    let result = detector
        .detect_modalities("empty_collection", &vectors)
        .await;

    assert_eq!(result.distinct_modalities, 0);
    assert!(!result.should_enable_hmgi);
    assert_eq!(result.reason, EnablementReason::InsufficientData);
    assert_eq!(result.confidence, 0.0);
}

/// Test HMGI modality counts
#[tokio::test]
async fn test_hmgi_modality_counts() {
    let detector = ModalityDetector::default_config();

    let vectors = vec![
        VectorRecordSample::with_modality("text"),
        VectorRecordSample::with_modality("text"),
        VectorRecordSample::with_modality("text"),
        VectorRecordSample::with_modality("image"),
        VectorRecordSample::with_modality("image"),
        VectorRecordSample::with_modality("video"),
    ];

    let result = detector.detect_modalities("counts_test", &vectors).await;

    assert_eq!(result.distinct_modalities, 3);
    assert_eq!(result.modality_counts.get("text"), Some(&3));
    assert_eq!(result.modality_counts.get("image"), Some(&2));
    assert_eq!(result.modality_counts.get("video"), Some(&1));
}

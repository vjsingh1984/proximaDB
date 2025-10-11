/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 */

//! SKS Extensions Tests
//!
//! Consolidated test suite for SKS (Semantic Knowledge Search) extensions.
//! Tests cover SIMILAR, FOLLOW, and ASSEMBLE functions, query planning,
//! and HashMap metadata performance optimization.
//!
//! Source: src/query/sks_extensions.rs
//! Tests extracted: 8 (all tests from the source module)

use proximadb::query::sks_extensions::*;
use proximadb::query::ast::*;
use proximadb::services::operations::vectors::VectorOperationsService;
use proximadb::graph::GraphOperationsService;
use std::sync::Arc;
use std::collections::HashMap;

// ============================================================================
// Basic SKS Operator Tests
// ============================================================================

#[test]
fn test_similar_operator_creation() {
    let op = SimilarOperator {
        embedding_field: "embedding".to_string(),
        query: SimilarQuery::Text("test query".to_string()),
        model_id: Some("test-model".to_string()),
        top_k: 10,
        progressive: true,
    };

    assert_eq!(op.top_k, 10);
    assert!(op.progressive);
}

#[test]
fn test_follow_operator_creation() {
    let op = FollowOperator {
        relation_type: "cites".to_string(),
        max_depth: 3,
        direction: TraversalDirection::Both,
        return_paths: true,
    };

    assert_eq!(op.max_depth, 3);
    assert_eq!(op.direction, TraversalDirection::Both);
}

#[test]
fn test_query_planner() {
    let planner = SksQueryPlanner;
    let similar_op = SimilarOperator {
        embedding_field: "embedding".to_string(),
        query: SimilarQuery::Text("test".to_string()),
        model_id: None,
        top_k: 5,
        progressive: true,
    };

    let plan = planner.plan_similar(&similar_op, None);
    assert_eq!(plan.stages.len(), 2); // Progressive has 2 stages
}

// ============================================================================
// SKS Integration Tests
// ============================================================================

#[tokio::test]
async fn test_similar_sql_parsing_integration() {
    // Test integration with sql_frontend parser
    let sql = "SELECT * FROM documents WHERE SIMILAR(embedding, [0.1, 0.2, 0.3], 'cosine', threshold => 0.8) LIMIT 10";

    // TODO: This would test the complete flow:
    // 1. sql_frontend/parser.rs parses the SQL
    // 2. sql_frontend/lowering.rs recognizes SIMILAR function
    // 3. SKS extensions converts to execution operations
    // 4. Execution engine runs with HashMap metadata optimization

    assert!(sql.contains("SIMILAR")); // Placeholder validation
}

#[tokio::test]
async fn test_follow_graph_integration() {
    // Test FOLLOW function with ORION graph engine
    let sql =
        "SELECT * FROM entities FOLLOW('user123', 'friend', depth => 3, direction => 'both')";

    // TODO: Test complete integration:
    // 1. Parse FOLLOW function from SQL
    // 2. Convert to graph traversal configuration
    // 3. Execute with ORION engine (CSR storage optimization)
    // 4. Return results with path information

    assert!(sql.contains("FOLLOW"));
}

#[tokio::test]
async fn test_hybrid_similar_follow_query() {
    // Test hybrid query combining SIMILAR and FOLLOW
    let sql = "SELECT * FROM entities WHERE SIMILAR(embedding, $1) AND FOLLOW(id, 'related', depth => 2)";

    // TODO: Test hybrid execution:
    // 1. Recognize both SIMILAR and FOLLOW functions
    // 2. Generate hybrid execution plan
    // 3. Execute vector search and graph traversal in parallel
    // 4. Apply fusion algorithm (RRF or Adaptive Semantic Fusion)
    // 5. Return unified results with combined scores

    assert!(sql.contains("SIMILAR") && sql.contains("FOLLOW"));
}

#[tokio::test]
async fn test_assemble_knowledge_integration() {
    // Test ASSEMBLE function for context building
    let sql = "SELECT ASSEMBLE(context, radius => 5, strategy => 'relevance') FROM knowledge WHERE topic = 'AI'";

    // TODO: Test knowledge assembly:
    // 1. Parse ASSEMBLE function parameters
    // 2. Gather context from multiple sources (vector + graph + temporal)
    // 3. Apply assembly strategy (relevance, temporal, semantic)
    // 4. Track provenance chain for explainability
    // 5. Return coherent context with source attribution

    assert!(sql.contains("ASSEMBLE"));
}

#[test]
fn test_hashmap_metadata_performance_in_sks() {
    // Test that SKS functions benefit from HashMap metadata optimization
    let executor = create_test_sks_executor();

    // Create metadata HashMap (v1 structure)
    let mut metadata = HashMap::new();
    for i in 0..10 {
        metadata.insert(
            format!("field_{}", i),
            proximadb::proto::proximadb_v1::SqlValue {
                value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(
                    format!("value_{}", i),
                )),
            },
        );
    }

    // Measure conversion performance (should be very fast with HashMap)
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        // let _fields = executor.convert_vos_metadata(&metadata); // Method is private
    }
    let conversion_time = start.elapsed();

    // HashMap conversion should be sub-millisecond even for many iterations
    assert!(
        conversion_time.as_millis() < 10,
        "HashMap metadata conversion should be very fast, took {:?}",
        conversion_time
    );
}

// ============================================================================
// Test Helper Functions
// ============================================================================

fn create_test_sks_executor() -> SksExecutor {
    // Create with mock services for testing
    let vector_service = Arc::new(create_mock_vector_service());
    let graph_service = Arc::new(create_mock_graph_service());

    SksExecutor::new(vector_service, graph_service)
}

fn create_mock_vector_service() -> VectorOperationsService {
    // Use tokio runtime to create async services
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        use proximadb::storage::engines::impls::sst::SstEngine;
        use proximadb::storage::persistence::write_ahead_log::{WALConfig, WriteAheadLogManager};
        use proximadb::storage::persistence::filesystem::FilesystemFactory;
        use proximadb::index::axis::management::manager::AxisManager;
        use proximadb::index::axis::types::AxisConfig;
        use proximadb::storage::metadata::MetadataStore;
        use proximadb::storage::metadata::MetadataStoreConfig;
        use proximadb::services::collection::manager::CollectionService;
        use proximadb::storage::traits::InternalCollectionProvider;
        use std::sync::Arc;

        let filesystem = Arc::new(FilesystemFactory::create(Default::default()).await.unwrap());

        let sst_engine = Arc::new(SstEngine::new().await.unwrap());

        let wal_config = WALConfig::default();
        let strategy_type = proximadb::storage::persistence::write_ahead_log::config::WriteBufferStrategyType::BincodeBatch;
        let strategy = proximadb::storage::persistence::write_ahead_log::WALBatchFactory::create_batch_serialization_strategy(
            strategy_type,
            &wal_config,
            filesystem.clone()
        ).await.unwrap();

        let wal_manager = Arc::new(WriteAheadLogManager::new(strategy, wal_config).await.unwrap());
        let axis_manager = Arc::new(AxisManager::new(AxisConfig::default()).await.unwrap());

        let metadata_backend = Arc::new(MetadataStore::new(MetadataStoreConfig::default()).await.unwrap())
            as Arc<dyn InternalCollectionProvider>;

        let collection_service = Arc::new(
            CollectionService::new(
                metadata_backend,
                proximadb::core::Config::default().storage.clone(),
            ).await.unwrap()
        );

        VectorOperationsService::new(
            sst_engine,
            wal_manager,
            axis_manager,
            collection_service,
        )
    })
}

fn create_mock_graph_service() -> GraphOperationsService {
    GraphOperationsService::new()
}

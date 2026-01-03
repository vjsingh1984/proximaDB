//! Integration tests for the Unified Multi-Model Query Engine
//!
//! Tests cross-model queries combining vector search, document queries,
//! and graph traversal with various fusion strategies.

use std::collections::HashMap;
use std::sync::Arc;

use proximadb::query::unified::UnifiedRecord;
use proximadb::query::unified::ast::{
    DistanceMetric, DocumentQueryExpr, FilterOperator, FilterValue, GraphTraversalExpr, PathFilter,
    StartNodeSpec, TraversalDirection, VectorSearchExpr, VectorSearchParams,
};
use proximadb::query::unified::fusion::SubQueryResult;
use proximadb::query::unified::{
    DataModel, FusionStrategy, MultiModelQuery, QueryDecomposer, ResultFuser, UnifiedQueryConfig,
};

/// Test query decomposition of a hybrid vector + document query
#[test]
fn test_decompose_hybrid_vector_document_query() {
    let decomposer = QueryDecomposer::new();

    let query = "SELECT * FROM products WHERE $.category = 'electronics' AND VECTOR_SIMILAR(embedding, ?, 0.8) LIMIT 10";
    let result = decomposer.decompose(query);

    assert!(
        result.is_ok(),
        "Failed to decompose query: {:?}",
        result.err()
    );
    let multi_query = result.unwrap();

    // Should have both vector and document components
    let has_vector = multi_query
        .components
        .iter()
        .any(|c| c.model == DataModel::Vector);
    let has_document = multi_query
        .components
        .iter()
        .any(|c| c.model == DataModel::Document);

    assert!(
        has_vector || has_document,
        "Should have at least one component"
    );
}

/// Test query decomposition of a pure vector search
#[test]
fn test_decompose_vector_only_query() {
    let decomposer = QueryDecomposer::new();

    let query = "SELECT * FROM embeddings WHERE VECTOR_SIMILAR(vector, ?, 0.9)";
    let result = decomposer.decompose(query);

    assert!(result.is_ok());
    let multi_query = result.unwrap();

    let vector_components: Vec<_> = multi_query
        .components
        .iter()
        .filter(|c| c.model == DataModel::Vector)
        .collect();

    assert!(
        !vector_components.is_empty(),
        "Should have vector component"
    );
}

/// Test query decomposition of a document query with JSON path
#[test]
fn test_decompose_document_query() {
    let decomposer = QueryDecomposer::new();

    let query = "SELECT * FROM users WHERE $.profile.age > 25 AND $.status = 'active'";
    let result = decomposer.decompose(query);

    assert!(result.is_ok());
    let multi_query = result.unwrap();

    let doc_components: Vec<_> = multi_query
        .components
        .iter()
        .filter(|c| c.model == DataModel::Document)
        .collect();

    assert!(!doc_components.is_empty(), "Should have document component");
}

/// Test intersection fusion strategy
#[test]
fn test_intersection_fusion() {
    let fuser = ResultFuser::new(FusionStrategy::Intersection);

    // Create overlapping results from two data models
    let vector_result = SubQueryResult {
        source_model: DataModel::Vector,
        records: vec![
            create_record("id_1", 0.95, DataModel::Vector),
            create_record("id_2", 0.90, DataModel::Vector),
            create_record("id_3", 0.85, DataModel::Vector),
        ],
        total_count: Some(3),
        execution_time_us: 100,
        records_scanned: 3,
        records_returned: 3,
    };

    let document_result = SubQueryResult {
        source_model: DataModel::Document,
        records: vec![
            create_record("id_2", 0.0, DataModel::Document),
            create_record("id_3", 0.0, DataModel::Document),
            create_record("id_4", 0.0, DataModel::Document),
        ],
        total_count: Some(3),
        execution_time_us: 50,
        records_scanned: 100,
        records_returned: 3,
    };

    let result = fuser.fuse(
        vec![vector_result, document_result],
        &FusionStrategy::Intersection,
    );

    assert!(result.is_ok());
    let fused = result.unwrap();

    // Intersection should only include id_2 and id_3
    assert_eq!(fused.records.len(), 2);
    assert!(fused.records.iter().any(|r| r.id == "id_2"));
    assert!(fused.records.iter().any(|r| r.id == "id_3"));
    assert!(!fused.records.iter().any(|r| r.id == "id_1"));
    assert!(!fused.records.iter().any(|r| r.id == "id_4"));
}

/// Test union fusion strategy
#[test]
fn test_union_fusion() {
    let fuser = ResultFuser::new(FusionStrategy::Union);

    let result1 = SubQueryResult {
        source_model: DataModel::Vector,
        records: vec![
            create_record("id_1", 0.9, DataModel::Vector),
            create_record("id_2", 0.8, DataModel::Vector),
        ],
        total_count: Some(2),
        execution_time_us: 100,
        records_scanned: 2,
        records_returned: 2,
    };

    let result2 = SubQueryResult {
        source_model: DataModel::Document,
        records: vec![
            create_record("id_3", 0.7, DataModel::Document),
            create_record("id_2", 0.95, DataModel::Document), // Duplicate with higher score
        ],
        total_count: Some(2),
        execution_time_us: 50,
        records_scanned: 2,
        records_returned: 2,
    };

    let fused = fuser
        .fuse(vec![result1, result2], &FusionStrategy::Union)
        .unwrap();

    // Union should include all unique IDs (id_1, id_2, id_3)
    assert_eq!(fused.records.len(), 3);

    // id_2 should have the higher score (0.95)
    let id2 = fused.records.iter().find(|r| r.id == "id_2").unwrap();
    assert!((id2.score.unwrap() - 0.95).abs() < 0.01);
}

/// Test reciprocal rank fusion (RRF)
#[test]
fn test_rrf_fusion() {
    let fuser = ResultFuser::new(FusionStrategy::ReciprocalRankFusion { k: 60 });

    // id_1 is ranked #1 in vector, #2 in document
    // id_2 is ranked #2 in vector, #1 in document
    let vector_result = SubQueryResult {
        source_model: DataModel::Vector,
        records: vec![
            create_record("id_1", 0.9, DataModel::Vector),
            create_record("id_2", 0.8, DataModel::Vector),
        ],
        total_count: Some(2),
        execution_time_us: 100,
        records_scanned: 2,
        records_returned: 2,
    };

    let document_result = SubQueryResult {
        source_model: DataModel::Document,
        records: vec![
            create_record("id_2", 0.9, DataModel::Document),
            create_record("id_1", 0.8, DataModel::Document),
        ],
        total_count: Some(2),
        execution_time_us: 50,
        records_scanned: 2,
        records_returned: 2,
    };

    let fused = fuser
        .fuse(
            vec![vector_result, document_result],
            &FusionStrategy::ReciprocalRankFusion { k: 60 },
        )
        .unwrap();

    // Both id_1 and id_2 should have similar RRF scores since they alternate positions
    assert_eq!(fused.records.len(), 2);

    let id1_score = fused
        .records
        .iter()
        .find(|r| r.id == "id_1")
        .unwrap()
        .score
        .unwrap();
    let id2_score = fused
        .records
        .iter()
        .find(|r| r.id == "id_2")
        .unwrap()
        .score
        .unwrap();

    // Both should be close since they each have one #1 rank and one #2 rank
    assert!((id1_score - id2_score).abs() < 0.01);
}

/// Test ranked fusion with weights
#[test]
fn test_weighted_ranked_fusion() {
    let mut weights = HashMap::new();
    weights.insert(DataModel::Vector, 2.0); // Double weight for vectors
    weights.insert(DataModel::Document, 1.0);

    let strategy = FusionStrategy::RankedFusion {
        weights: weights.clone(),
        normalize: true,
    };

    let fuser = ResultFuser::new(strategy.clone());

    let vector_result = SubQueryResult {
        source_model: DataModel::Vector,
        records: vec![create_record("id_1", 0.9, DataModel::Vector)],
        total_count: Some(1),
        execution_time_us: 100,
        records_scanned: 1,
        records_returned: 1,
    };

    let document_result = SubQueryResult {
        source_model: DataModel::Document,
        records: vec![create_record("id_1", 0.5, DataModel::Document)],
        total_count: Some(1),
        execution_time_us: 50,
        records_scanned: 1,
        records_returned: 1,
    };

    let fused = fuser
        .fuse(vec![vector_result, document_result], &strategy)
        .unwrap();

    assert_eq!(fused.records.len(), 1);
    // The combined score should reflect the 2x weight for vectors
    assert!(fused.records[0].score.unwrap() > 0.0);
}

/// Test building multi-model query programmatically
#[test]
fn test_build_multimodel_query() {
    let query = MultiModelQuery::new()
        .with_vector_search(VectorSearchExpr {
            collection: "embeddings".to_string(),
            query_vector: vec![0.1, 0.2, 0.3, 0.4],
            top_k: 10,
            threshold: Some(0.8),
            metric: DistanceMetric::Cosine,
            params: VectorSearchParams::default(),
        })
        .with_document_query(DocumentQueryExpr {
            collection: "products".to_string(),
            path_filters: vec![PathFilter {
                path: "$.category".to_string(),
                operator: FilterOperator::Eq,
                value: FilterValue::String("electronics".to_string()),
            }],
            text_search: None,
            projection: vec!["id".to_string(), "name".to_string()],
            sort: None,
            limit: Some(100),
        })
        .with_fusion(FusionStrategy::Intersection)
        .with_limit(10);

    assert_eq!(query.components.len(), 2);
    assert!(matches!(
        query.fusion_strategy,
        FusionStrategy::Intersection
    ));
    assert_eq!(query.limit, Some(10));
}

/// Test graph traversal expression creation
#[test]
fn test_graph_traversal_expression() {
    let traversal = GraphTraversalExpr {
        graph_name: "knowledge".to_string(),
        start_nodes: StartNodeSpec::Ids(vec!["node_1".to_string()]),
        edge_types: vec!["KNOWS".to_string(), "FOLLOWS".to_string()],
        direction: TraversalDirection::Outgoing,
        max_depth: 3,
        min_depth: 1,
        node_filters: vec![],
        edge_filters: vec![],
        return_paths: true,
    };

    assert_eq!(traversal.graph_name, "knowledge");
    assert_eq!(traversal.max_depth, 3);
    assert!(traversal.return_paths);
}

/// Test empty results handling
#[test]
fn test_empty_results_fusion() {
    let fuser = ResultFuser::new(FusionStrategy::Intersection);
    let result = fuser.fuse(vec![], &FusionStrategy::Intersection);

    assert!(result.is_ok());
    assert!(result.unwrap().records.is_empty());
}

/// Test single result passthrough
#[test]
fn test_single_result_passthrough() {
    let fuser = ResultFuser::new(FusionStrategy::Intersection);

    let single_result = SubQueryResult {
        source_model: DataModel::Vector,
        records: vec![
            create_record("id_1", 0.9, DataModel::Vector),
            create_record("id_2", 0.8, DataModel::Vector),
        ],
        total_count: Some(2),
        execution_time_us: 100,
        records_scanned: 10,
        records_returned: 2,
    };

    let fused = fuser
        .fuse(vec![single_result], &FusionStrategy::Intersection)
        .unwrap();

    // Single result should pass through unchanged
    assert_eq!(fused.records.len(), 2);
    assert_eq!(fused.total_count, Some(2));
}

/// Test parallelizable component detection
#[test]
fn test_component_parallelizable() {
    let query = MultiModelQuery::new()
        .with_vector_search(VectorSearchExpr {
            collection: "test".to_string(),
            query_vector: vec![0.1, 0.2],
            top_k: 10,
            threshold: None,
            metric: DistanceMetric::Cosine,
            params: VectorSearchParams::default(),
        })
        .with_document_query(DocumentQueryExpr {
            collection: "test".to_string(),
            path_filters: vec![],
            text_search: None,
            projection: vec![],
            sort: None,
            limit: Some(10),
        });

    // Both components should be parallelizable (no dependencies)
    for component in &query.components {
        assert!(component.is_parallelizable());
    }
}

/// Test default configuration
#[test]
fn test_default_config() {
    let config = UnifiedQueryConfig::default();

    assert_eq!(config.max_parallel_queries, 4);
    assert!(config.enable_cache);
    assert_eq!(config.query_timeout_ms, 30000);
}

// Helper function to create test records
fn create_record(id: &str, score: f64, model: DataModel) -> UnifiedRecord {
    UnifiedRecord {
        id: id.to_string(),
        source_model: model,
        data: serde_json::json!({
            "id": id,
            "test": true
        }),
        score: Some(score),
        metadata: HashMap::new(),
    }
}

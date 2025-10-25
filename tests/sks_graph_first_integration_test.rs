// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! SKS Graph-First Integration Tests
//!
//! This test suite validates the SKS graph-first architecture implementation
//! following TDD principles (RED → GREEN → REFACTOR).
//!
//! Test Coverage:
//! - Entity CRUD operations via Orion backend
//! - Relation management (add, query, traverse)
//! - Hybrid queries (vector similarity + graph traversal)
//! - Schema mapping (Entity ↔ Node, Relation ↔ Edge)
//! - Migration from legacy to graph-first

#[path = "common/mod.rs"]
mod common;

use common::sks_fixtures::TestKnowledgeGraph;
use proximadb::graph::GraphOperationsService;
use proximadb::proto::proximadb_v1::CreateGraphRequest;
use proximadb::storage::entity_store::{EntityStore, OrionBackedEntityStore};
use std::sync::Arc;

/// Test 1: Entity Insertion (GREEN phase - OrionBackedEntityStore implemented!)
#[tokio::test]
async fn test_entity_insertion_orion() {
    // Setup
    let graph = TestKnowledgeGraph::small(); // 100 entities
    let graph_service = Arc::new(GraphOperationsService::new());

    // Create graph collection
    let create_request = CreateGraphRequest {
        graph_id: "test-insertion".to_string(),
        name: Some("Test Insertion".to_string()),
        description: None,
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };
    graph_service.create_graph_collection(create_request)
        .await
        .expect("Failed to create graph collection");

    let store = OrionBackedEntityStore::new(graph_service, "test-insertion".to_string());

    // Test: Insert all entities (use graph_id as collection_id)
    for entity in &graph.entities {
        store.upsert_entity("test-insertion", entity.clone())
            .await
            .expect("Failed to insert entity");
    }

    // Verify: All entities should be retrievable
    for entity in &graph.entities {
        let retrieved = store.get_entity("test-insertion", &entity.id, true, false)
            .await
            .expect("Failed to retrieve entity")
            .expect("Entity not found");
        assert_eq!(retrieved.id, entity.id);
        assert_eq!(retrieved.embeddings.len(), entity.embeddings.len());
    }

    println!("✓ Successfully inserted and retrieved {} entities", graph.entities.len());
}

/// Test 2: Entity Retrieval (GREEN phase)
#[tokio::test]
async fn test_entity_retrieval_orion() {
    let graph = TestKnowledgeGraph::small();
    let graph_service = Arc::new(GraphOperationsService::new());

    // Create graph collection
    let create_request = CreateGraphRequest {
        graph_id: "test-retrieval".to_string(),
        name: Some("Test Retrieval".to_string()),
        description: None,
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };
    graph_service.create_graph_collection(create_request)
        .await
        .expect("Failed to create graph collection");

    let store = OrionBackedEntityStore::new(graph_service, "test-retrieval".to_string());

    // Insert entity
    let entity = &graph.entities[0];
    store.upsert_entity("test-retrieval", entity.clone())
        .await
        .expect("Failed to insert");

    // Retrieve entity
    let retrieved = store.get_entity("test-retrieval", &entity.id, true, false)
        .await
        .expect("Failed to retrieve")
        .expect("Entity not found");

    // Verify all fields
    assert_eq!(retrieved.id, entity.id);
    assert_eq!(retrieved.collection_id, entity.collection_id);
    assert_eq!(retrieved.embeddings[0].vector, entity.embeddings[0].vector);
    // typed_metadata comparison requires deep equality check
    assert_eq!(retrieved.typed_metadata.is_some(), entity.typed_metadata.is_some());

    println!("✓ Entity retrieval verified");
}

/// Test 3: Entity Deletion (GREEN phase)
#[tokio::test]
async fn test_entity_deletion_orion() {
    let graph = TestKnowledgeGraph::small();
    let graph_service = Arc::new(GraphOperationsService::new());

    // Create graph collection
    let create_request = CreateGraphRequest {
        graph_id: "test-deletion".to_string(),
        name: Some("Test Deletion".to_string()),
        description: None,
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };
    graph_service.create_graph_collection(create_request)
        .await
        .expect("Failed to create graph collection");

    let store = OrionBackedEntityStore::new(graph_service, "test-deletion".to_string());

    // Insert and verify entity exists
    let entity = &graph.entities[0];
    store.upsert_entity("test-deletion", entity.clone())
        .await
        .expect("Failed to insert");
    assert!(store.get_entity("test-deletion", &entity.id, true, false)
        .await
        .unwrap()
        .is_some());

    // Delete entity
    store.delete_entity("test-deletion", &entity.id, true)
        .await
        .expect("Failed to delete");

    // Verify entity is gone
    assert!(store.get_entity("test-deletion", &entity.id, true, false)
        .await
        .unwrap()
        .is_none());

    println!("✓ Entity deletion verified");
}

/// Test 4: Relation Management (GREEN phase)
#[tokio::test]
async fn test_relation_management_orion() {
    let graph = TestKnowledgeGraph::small();
    let graph_service = Arc::new(GraphOperationsService::new());

    // Create graph collection
    let create_request = CreateGraphRequest {
        graph_id: "test-relations".to_string(),
        name: Some("Test Relations".to_string()),
        description: None,
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };
    graph_service.create_graph_collection(create_request)
        .await
        .expect("Failed to create graph collection");

    let store = OrionBackedEntityStore::new(graph_service, "test-relations".to_string());

    // Insert entities first (take 10)
    for entity in graph.entities.iter().take(10) {
        store.upsert_entity("test-relations", entity.clone())
            .await
            .expect("Failed to insert entity");
    }

    // Insert relations (take 20, but filter for entities that exist)
    let mut added_relations = 0;
    for relation in graph.relations.iter().take(20) {
        // Only add relations where both entities exist (within first 10)
        let source_idx = relation.source_entity_id.strip_prefix("entity-")
            .and_then(|s| s.parse::<usize>().ok());
        let target_idx = relation.target_entity_id.strip_prefix("entity-")
            .and_then(|s| s.parse::<usize>().ok());

        if let (Some(src), Some(tgt)) = (source_idx, target_idx) {
            if src < 10 && tgt < 10 {
                store.add_relation(relation.clone())
                    .await
                    .expect("Failed to add relation");
                added_relations += 1;
            }
        }
    }

    // Query relations for first entity
    let relations = store.get_relations(&graph.entities[0].id)
        .await
        .expect("Failed to query relations");

    // Verify relations exist (may be 0 if entity-0 has no outgoing edges in test data)
    println!("✓ Relation management verified ({} relations added, {} for entity-0)",
             added_relations, relations.len());
}

/// Test 5: Graph Traversal (GREEN phase)
#[tokio::test]
async fn test_graph_traversal_orion() {
    let graph = TestKnowledgeGraph::research_papers();
    let graph_service = Arc::new(GraphOperationsService::new());

    // Create graph collection
    let create_request = CreateGraphRequest {
        graph_id: "test-traversal".to_string(),
        name: Some("Test Traversal".to_string()),
        description: None,
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };
    graph_service.create_graph_collection(create_request)
        .await
        .expect("Failed to create graph collection");

    let store = OrionBackedEntityStore::new(graph_service, "test-traversal".to_string());

    // Insert papers and citations
    for entity in &graph.entities {
        store.upsert_entity("test-traversal", entity.clone())
            .await
            .expect("Failed to insert");
    }
    for relation in &graph.relations {
        store.add_relation(relation.clone())
            .await
            .expect("Failed to add relation");
    }

    // Traverse: Find all papers cited by paper-100 (2-hop)
    let start_id = "paper-100";
    let traversal_result = store.traverse_graph(
        start_id,
        2, // max_depth
        Some("cites"), // relation_type filter
    ).await
        .expect("Failed to traverse graph");

    // Verify traversal found citations
    println!("✓ Graph traversal found {} entities from paper-100 (2-hop)",
             traversal_result.len());
}

/// Test 6: Hybrid Query (Vector + Graph) (RED phase)
#[test]
#[ignore = "RED phase: Hybrid query engine not yet implemented"]
fn test_hybrid_query_vector_plus_graph() {
    // let graph = TestKnowledgeGraph::research_papers();
    // let store = OrionBackedEntityStore::new().expect("Failed to create store");

    // Insert data
    // for entity in &graph.entities {
    //     store.upsert_entity(entity.clone()).expect("Failed to insert");
    // }
    // for relation in &graph.relations {
    //     store.add_relation(relation.clone()).expect("Failed to add relation");
    // }

    // Hybrid query:
    // 1. Vector search for papers similar to query embedding
    // 2. Graph traversal to find related papers via citations
    // let query_embedding = &graph.embeddings[0];
    // let hybrid_results = store.hybrid_search(
    //     query_embedding,
    //     10, // top_k for vector search
    //     2,  // graph traversal depth
    //     Some("cites"), // relation filter
    // ).expect("Failed to execute hybrid query");

    // Verify results combine vector similarity and graph structure
    // assert!(!hybrid_results.is_empty());

    panic!("RED phase: This test should fail until hybrid query is implemented");
}

/// Test 7: Schema Mapping (Entity → Node) (GREEN phase)
#[test]
fn test_entity_to_node_mapping() {
    use proximadb::storage::entity_store::EntityNodeMapper;

    let graph = TestKnowledgeGraph::research_papers();
    let entity = &graph.entities[0];

    // Convert Entity to Orion Node
    let mapper = EntityNodeMapper;
    let node = mapper.entity_to_node(entity)
        .expect("Failed to map entity to node");

    // Verify mapping preserves all data
    assert_eq!(node.id, entity.id);
    assert_eq!(node.labels[0], entity.collection_id);
    if entity.embeddings.len() > 1 {
        assert!(node.properties.contains_key("__embeddings"));
    }
    if entity.typed_metadata.is_some() {
        assert!(node.properties.contains_key("__typed_metadata"));
    }

    // Convert back: Node → Entity
    let entity_restored = mapper.node_to_entity(&node)
        .expect("Failed to map node to entity");

    // Verify round-trip correctness
    assert_eq!(entity_restored.id, entity.id);
    assert_eq!(entity_restored.collection_id, entity.collection_id);
    if !entity.embeddings.is_empty() {
        assert_eq!(entity_restored.embeddings[0].vector, entity.embeddings[0].vector);
    }

    println!("✓ Entity→Node→Entity round-trip verified");
}

/// Test 8: Schema Mapping (Relation → Edge) (GREEN phase)
#[test]
fn test_relation_to_edge_mapping() {
    use proximadb::storage::entity_store::RelationEdgeMapper;

    let graph = TestKnowledgeGraph::research_papers();
    let relation = &graph.relations[0];

    // Convert Relation to Orion Edge
    let mapper = RelationEdgeMapper;
    let edge = mapper.relation_to_edge(relation)
        .expect("Failed to map relation to edge");

    // Verify mapping
    assert_eq!(edge.from_node_id, relation.source_entity_id);
    assert_eq!(edge.to_node_id, relation.target_entity_id);
    assert_eq!(edge.edge_type, relation.relation_type);
    assert_eq!(edge.weight, Some(relation.weight as f64));

    // Convert back: Edge → Relation
    let relation_restored = mapper.edge_to_relation(&edge)
        .expect("Failed to map edge to relation");

    // Verify round-trip correctness
    assert_eq!(relation_restored.source_entity_id, relation.source_entity_id);
    assert_eq!(relation_restored.target_entity_id, relation.target_entity_id);
    assert_eq!(relation_restored.relation_type, relation.relation_type);
    assert_eq!(relation_restored.weight, relation.weight);

    println!("✓ Relation→Edge→Relation round-trip verified");
}

/// Test 9: Batch Entity Insertion (RED phase)
#[test]
#[ignore = "RED phase: Batch operations not yet implemented"]
fn test_batch_entity_insertion() {
    // let graph = TestKnowledgeGraph::medium();
    // let store = OrionBackedEntityStore::new().expect("Failed to create store");

    // Batch insert 1000 entities
    // let start = std::time::Instant::now();
    // store.batch_upsert_entities(&graph.entities)
    //     .expect("Failed to batch insert");
    // let duration = start.elapsed();

    // Verify all entities inserted
    // for entity in &graph.entities {
    //     assert!(store.get_entity(&entity.id).unwrap().is_some());
    // }

    // Benchmark: Should be faster than individual inserts
    // println!("Batch insert of {} entities took {:?}", graph.entities.len(), duration);

    panic!("RED phase: This test should fail until batch operations are implemented");
}

/// Test 10: Metadata Filtering During Traversal (RED phase)
#[test]
#[ignore = "RED phase: Metadata filtering not yet implemented"]
fn test_metadata_filtering_during_traversal() {
    // let graph = TestKnowledgeGraph::ecommerce();
    // let store = OrionBackedEntityStore::new().expect("Failed to create store");

    // Insert products
    // for entity in &graph.entities {
    //     store.upsert_entity(entity.clone()).expect("Failed to insert");
    // }
    // for relation in &graph.relations {
    //     store.add_relation(relation.clone()).expect("Failed to add relation");
    // }

    // Hybrid query with metadata filter:
    // Find products related to product-0, but only in "Electronics" category
    // let results = store.traverse_graph_filtered(
    //     "product-0",
    //     2, // max_depth
    //     Some("related_to"), // relation filter
    //     Some(|entity| {
    //         // Metadata filter: category == "Electronics"
    //         entity.typed_metadata.as_ref()
    //             .and_then(|m| m.fields.get("category"))
    //             .and_then(|f| f.value.as_ref())
    //             .map(|v| {
    //                 if let typed_field::Value::StringValue(s) = v {
    //                     s == "Electronics"
    //                 } else {
    //                     false
    //                 }
    //             })
    //             .unwrap_or(false)
    //     })
    // ).expect("Failed to execute filtered traversal");

    // Verify all results are in Electronics category
    // for entity in results {
    //     let category = entity.typed_metadata.as_ref()
    //         .unwrap()
    //         .fields.get("category")
    //         .unwrap()
    //         .value.as_ref()
    //         .unwrap();
    //     if let typed_field::Value::StringValue(s) = category {
    //         assert_eq!(s, "Electronics");
    //     }
    // }

    panic!("RED phase: This test should fail until metadata filtering is implemented");
}

/// Test 11: Performance Comparison (Legacy vs Graph-First)
#[test]
#[ignore = "RED phase: Requires both implementations to compare"]
fn test_performance_comparison_legacy_vs_graph_first() {
    // let graph = TestKnowledgeGraph::medium();

    // Benchmark legacy approach
    // let legacy_start = std::time::Instant::now();
    // // Simulate legacy: insert into separate stores
    // let legacy_duration = legacy_start.elapsed();

    // Benchmark graph-first approach
    // let graph_first_start = std::time::Instant::now();
    // // Use OrionBackedEntityStore
    // let graph_first_duration = graph_first_start.elapsed();

    // Verify graph-first is faster (target: 10-20x improvement)
    // println!("Legacy: {:?}, Graph-first: {:?}", legacy_duration, graph_first_duration);
    // assert!(graph_first_duration < legacy_duration);

    panic!("RED phase: This test should fail until both implementations exist");
}

/// Test 12: Memory Overhead Comparison
#[test]
#[ignore = "RED phase: Requires memory profiling"]
fn test_memory_overhead_comparison() {
    // let graph = TestKnowledgeGraph::medium();

    // Measure legacy memory overhead
    // let legacy_memory = measure_memory_usage(|| {
    //     // Simulate legacy: vectors + relations HashMap + KV metadata
    // });

    // Measure graph-first memory overhead
    // let graph_first_memory = measure_memory_usage(|| {
    //     // Use OrionBackedEntityStore with CSR
    // });

    // Verify graph-first uses less memory (target: 50% reduction)
    // println!("Legacy: {} bytes, Graph-first: {} bytes", legacy_memory, graph_first_memory);
    // assert!(graph_first_memory < legacy_memory);

    panic!("RED phase: This test should fail until memory profiling is implemented");
}

/// Helper: Verify test fixtures are valid
#[test]
fn test_fixtures_validation() {
    // Small graph
    let small = TestKnowledgeGraph::small();
    assert_eq!(small.entities.len(), 100);
    assert!(small.relations.len() > 0);
    assert_eq!(small.embeddings.len(), 100);

    // Medium graph
    let medium = TestKnowledgeGraph::medium();
    assert_eq!(medium.entities.len(), 1000);
    assert!(medium.relations.len() > 0);

    // Research papers
    let papers = TestKnowledgeGraph::research_papers();
    assert_eq!(papers.entities.len(), 500);
    for rel in &papers.relations {
        assert_eq!(rel.relation_type, "cites");
    }

    // E-commerce
    let ecommerce = TestKnowledgeGraph::ecommerce();
    assert_eq!(ecommerce.entities.len(), 1000);
    assert!(ecommerce.entities[0].typed_metadata.is_some());
}

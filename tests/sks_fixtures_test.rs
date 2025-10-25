// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Simple test to verify SKS fixtures compile and work

#[path = "common/mod.rs"]
mod common;

use common::sks_fixtures::TestKnowledgeGraph;

#[test]
fn test_small_graph() {
    let graph = TestKnowledgeGraph::small();
    assert_eq!(graph.entities.len(), 100);
    assert!(graph.relations.len() > 0);
}

#[test]
fn test_medium_graph() {
    let graph = TestKnowledgeGraph::medium();
    assert_eq!(graph.entities.len(), 1000);
    assert!(graph.relations.len() > 0);
}

#[test]
fn test_research_papers() {
    let graph = TestKnowledgeGraph::research_papers();
    assert_eq!(graph.entities.len(), 500);

    // Verify all relations are citations
    for rel in &graph.relations {
        assert_eq!(rel.relation_type, "cites");
    }
}

#[test]
fn test_ecommerce() {
    let graph = TestKnowledgeGraph::ecommerce();
    assert_eq!(graph.entities.len(), 1000);

    // Verify products have metadata
    assert!(graph.entities[0].typed_metadata.is_some());
}

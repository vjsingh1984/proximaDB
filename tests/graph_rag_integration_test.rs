use proximadb::core::error::ProximaDBError;
use proximadb::graph::engines::GraphEngine;
use proximadb::graph::engines::orion::OrionGraphEngine;
use proximadb::graph::rag::RagPipeline;
use proximadb::graph::rag::engine_impls::KHopSubgraphBuilder;
use proximadb::graph::rag::{RagBudget, RagQuery};
use proximadb::proto::proximadb_v1::{Edge, Node};
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::test]
async fn test_rag_pipeline_integration() {
    // 1. Setup in-memory graph engine
    let engine = Arc::new(OrionGraphEngine::new());

    // Create nodes: "rust" -> "vector" -> "proximadb"
    let nodes = vec![
        Node {
            id: "rust".to_string(),
            labels: vec!["Lang".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        },
        Node {
            id: "vector".to_string(),
            labels: vec!["Topic".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        },
        Node {
            id: "proximadb".to_string(),
            labels: vec!["Tool".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        },
    ];

    for n in nodes {
        engine.insert_node(n).await.unwrap();
    }

    let edges = vec![
        Edge {
            id: "e1".to_string(),
            from_node_id: "rust".to_string(),
            to_node_id: "vector".to_string(),
            edge_type: "related".to_string(),
            properties: HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        },
        Edge {
            id: "e2".to_string(),
            from_node_id: "vector".to_string(),
            to_node_id: "proximadb".to_string(),
            edge_type: "related".to_string(),
            properties: HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        },
    ];

    for e in edges {
        engine.insert_edge(e).await.unwrap();
    }

    // 2. Mock retriever for seeds
    struct MockRetriever;
    #[async_trait::async_trait]
    impl proximadb::graph::rag::NodeRetriever for MockRetriever {
        async fn retrieve(
            &self,
            _query: &RagQuery,
        ) -> std::result::Result<Vec<String>, ProximaDBError> {
            Ok(vec!["rust".to_string()])
        }
    }

    // 3. Setup pipeline
    let retriever = MockRetriever;
    let builder = KHopSubgraphBuilder::new(engine.clone() as Arc<dyn GraphEngine>, 2, None);
    let budget = RagBudget {
        max_seeds: 5,
        max_subgraph_nodes: 10,
    };

    let pipeline = RagPipeline::without_filter(retriever, builder, budget);

    // 4. Run query
    let query = RagQuery::text("find rust stuff");
    let subgraph = pipeline.run(&query).await.unwrap();

    // 5. Verify: should contain rust, vector, and proximadb (2 hops)
    assert_eq!(subgraph.nodes.len(), 3);
    assert!(subgraph.nodes.contains(&"rust".to_string()));
    assert!(subgraph.nodes.contains(&"vector".to_string()));
    assert!(subgraph.nodes.contains(&"proximadb".to_string()));
    assert_eq!(subgraph.edges.len(), 2);
}

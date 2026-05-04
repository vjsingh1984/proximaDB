#[cfg(test)]
mod tests {
    use crate::graph::engines::GraphEngine;
    use crate::graph::engines::orion::OrionGraphEngine;
    use crate::graph::rag::SubgraphBuilder;
    use crate::graph::rag::engine_impls::*;
    use crate::proto::proximadb_v1::{Edge, Node};
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_khop_subgraph_builder() {
        let engine = Arc::new(OrionGraphEngine::new());

        // Setup a small graph: a -> b -> c
        let node_a = Node {
            id: "a".to_string(),
            labels: vec!["Label".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let node_b = Node {
            id: "b".to_string(),
            labels: vec!["Label".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let node_c = Node {
            id: "c".to_string(),
            labels: vec!["Label".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        engine.insert_node(node_a).await.unwrap();
        engine.insert_node(node_b).await.unwrap();
        engine.insert_node(node_c).await.unwrap();

        let edge_ab = Edge {
            id: "ab".to_string(),
            from_node_id: "a".to_string(),
            to_node_id: "b".to_string(),
            edge_type: "link".to_string(),
            properties: HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let edge_bc = Edge {
            id: "bc".to_string(),
            from_node_id: "b".to_string(),
            to_node_id: "c".to_string(),
            edge_type: "link".to_string(),
            properties: HashMap::new(),
            weight: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };

        engine.insert_edge(edge_ab).await.unwrap();
        engine.insert_edge(edge_bc).await.unwrap();

        let builder = KHopSubgraphBuilder::new(engine, 1, None);
        let subgraph = builder.build(&["a".to_string()]).await.unwrap();

        // 1-hop from 'a' should give 'a' and 'b'
        assert_eq!(subgraph.nodes.len(), 2);
        assert!(subgraph.nodes.contains(&"a".to_string()));
        assert!(subgraph.nodes.contains(&"b".to_string()));
        assert_eq!(subgraph.edges.len(), 1);
        assert_eq!(subgraph.edges[0].from, "a");
        assert_eq!(subgraph.edges[0].to, "b");

        // 2-hop from 'a'
        let builder_2 = KHopSubgraphBuilder::new(builder.engine().clone(), 2, None);
        let subgraph_2 = builder_2.build(&["a".to_string()]).await.unwrap();

        assert_eq!(subgraph_2.nodes.len(), 3);
        assert!(subgraph_2.nodes.contains(&"a".to_string()));
        assert!(subgraph_2.nodes.contains(&"b".to_string()));
        assert!(subgraph_2.nodes.contains(&"c".to_string()));
        assert_eq!(subgraph_2.edges.len(), 2);
    }
}

/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Integration test for `GraphOperationsService::impact_analysis` (TD-131) — forward blast radius
//! (outgoing edges: "what does X impact") and backward (incoming edges: "what impacts X"). This is
//! the server-side baseline the embedded-parity gate compares against.

use proximadb::{
    graph::{Edge, ImpactDirection, Node, service::GraphOperationsService},
    proto::proximadb_v1::{CompressionAlgorithm, CreateGraphRequest, GraphStorageConfig},
};
use std::collections::HashMap;
use std::sync::Arc;

const GRAPH_ID: &str = "impact_test_graph";

async fn build_chain_graph(service: &GraphOperationsService, dir: &str) {
    // A --CALLS--> B --CALLS--> C
    // A --CALLS--> D
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("create test dir");

    let _ = service
        .create_graph_collection(CreateGraphRequest {
            graph_id: GRAPH_ID.to_string(),
            name: Some("impact analysis test".to_string()),
            description: None,
            schema: None,
            storage_config: Some(GraphStorageConfig {
                engine_type: "ORION".to_string(),
                base_url: dir.to_string(),
                compression: CompressionAlgorithm::CompressionSnappy as i32,
                enable_wal: true,
                snapshot_interval_hours: 24,
                engine_specific_config: HashMap::new(),
            }),
            engine_config: None,
            access_control: None,
        })
        .await;

    let nodes: Vec<Node> = ["A", "B", "C", "D"]
        .into_iter()
        .map(|id| Node {
            id: id.to_string(),
            labels: vec!["Symbol".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        })
        .collect();
    let created = service
        .batch_create_nodes(GRAPH_ID, nodes)
        .await
        .expect("create nodes");
    assert_eq!(created.len(), 4, "four nodes seeded");

    let edge = |id: &str, from: &str, to: &str| Edge {
        id: id.to_string(),
        from_node_id: from.to_string(),
        to_node_id: to.to_string(),
        edge_type: "CALLS".to_string(),
        properties: HashMap::new(),
        weight: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    };
    let edges = vec![
        edge("e_ab", "A", "B"),
        edge("e_bc", "B", "C"),
        edge("e_ad", "A", "D"),
    ];
    let created_edges = service
        .batch_create_edges(GRAPH_ID, edges)
        .await
        .expect("create edges");
    assert_eq!(created_edges.len(), 3, "three edges seeded");
}

fn node_ids(service_nodes: &[Node]) -> std::collections::HashSet<&str> {
    service_nodes.iter().map(|n| n.id.as_str()).collect()
}

/// Forward from A reaches B (depth 1), C + D (depth 2): "what A impacts".
#[tokio::test]
async fn impact_analysis_forward_follows_outgoing_edges() {
    let service = Arc::new(GraphOperationsService::new());
    build_chain_graph(&service, "/tmp/proximadb-test-impact-fwd").await;

    let response = service
        .impact_analysis(
            GRAPH_ID,
            "A",
            ImpactDirection::Forward,
            vec!["CALLS".to_string()],
            2,
            100,
        )
        .await
        .expect("forward impact analysis");

    let ids = node_ids(&response.nodes);
    for expected in ["B", "C", "D"] {
        assert!(
            ids.contains(expected),
            "forward from A should reach {expected} (got {ids:?})"
        );
    }
}

/// Backward from C reaches B (depth 1) and A (depth 2): "what impacts C". D must NOT appear.
#[tokio::test]
async fn impact_analysis_backward_follows_incoming_edges() {
    let service = Arc::new(GraphOperationsService::new());
    build_chain_graph(&service, "/tmp/proximadb-test-impact-bwd").await;

    let response = service
        .impact_analysis(
            GRAPH_ID,
            "C",
            ImpactDirection::Backward,
            vec!["CALLS".to_string()],
            2,
            100,
        )
        .await
        .expect("backward impact analysis");

    let ids = node_ids(&response.nodes);
    assert!(
        ids.contains("B") && ids.contains("A"),
        "backward from C should reach B and A (got {ids:?})"
    );
    assert!(
        !ids.contains("D"),
        "D is not a predecessor of C and must not appear (got {ids:?})"
    );
}

/// Default direction is Forward (matches the REST handler default).
#[tokio::test]
async fn impact_analysis_default_direction_is_forward() {
    let service = Arc::new(GraphOperationsService::new());
    build_chain_graph(&service, "/tmp/proximadb-test-impact-default").await;

    let response = service
        .impact_analysis(GRAPH_ID, "A", ImpactDirection::default(), vec![], 1, 100)
        .await
        .expect("default-direction impact analysis");

    let ids = node_ids(&response.nodes);
    assert!(ids.contains("B"), "depth-1 forward from A reaches B");
    assert!(
        !ids.contains("C"),
        "depth-1 forward from A must not reach C (got {ids:?})"
    );
}

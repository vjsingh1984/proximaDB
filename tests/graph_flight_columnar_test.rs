//! Batched columnar graph path over Arrow Flight — service-level round-trip.
//!
//! Exercises exactly what the Flight `graph_nodes`/`graph_edges` DoExchange
//! ingest and DoGet export handlers delegate to: encode neutral nodes/edges to a
//! columnar Arrow `RecordBatch` (`graph_codec`), decode them, bulk-create them
//! through the live `GraphOperationsService`, query them back, and re-encode —
//! all through the public API. The handler glue is thin streaming over this
//! core (the Flight streaming/descriptor plumbing it reuses is already e2e-tested
//! by the vector/record path); a socket-level client e2e is a follow-up.

use std::collections::HashMap;
use std::sync::Arc;

use proximadb::graph::GraphOperationsService;
use proximadb::graph::model::{EmbeddingVersion, Node, PropertyValue, property_value::Value};
use proximadb::network::arrow_ipc::graph_codec;

/// Provision a graph collection (DDL is proto-typed, like the v2 gRPC surface;
/// the Flight ingest/export routes assume the graph already exists, created via
/// REST in production).
async fn create_graph(graph: &GraphOperationsService, graph_id: &str) {
    graph
        .create_graph_collection(proximadb::proto::proximadb_v1::CreateGraphRequest {
            graph_id: graph_id.to_string(),
            name: Some(graph_id.to_string()),
            description: None,
            schema: None,
            storage_config: None,
            engine_config: None,
            access_control: None,
        })
        .await
        .expect("create graph collection");
}

fn node(id: &str, dim: usize) -> Node {
    let mut properties = HashMap::new();
    properties.insert(
        "name".to_string(),
        PropertyValue {
            value: Some(Value::StringValue(format!("name-{id}"))),
        },
    );
    properties.insert(
        "rank".to_string(),
        PropertyValue {
            value: Some(Value::IntValue(7)),
        },
    );
    Node {
        id: id.to_string(),
        labels: vec!["Person".to_string()],
        properties,
        embedding: (dim > 0).then(|| EmbeddingVersion {
            model_id: "bge".to_string(),
            model_version: "v1".to_string(),
            vector: (0..dim).map(|i| i as f32 * 0.25).collect(),
            dimension: dim as u32,
            created_at_ms: 0,
            model_params: Default::default(),
            modality: 0,
        }),
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Ingest nodes via the columnar codec + graph batch API, read them back via a
/// label query, and confirm the columnar round-trip preserves identity,
/// labels, properties, and the embedding vector.
#[tokio::test]
async fn columnar_node_ingest_and_export_round_trip() {
    let graph = Arc::new(GraphOperationsService::new());
    let graph_id = "g_columnar_nodes";
    create_graph(&graph, graph_id).await;

    let originals = vec![node("n1", 4), node("n2", 4), node("n3", 0)];

    // --- DoExchange ingest half: encode -> wire batch -> decode -> bulk create.
    let batch = graph_codec::nodes_to_batch(&originals).expect("encode node batch");
    assert_eq!(batch.num_rows(), 3);
    let decoded = graph_codec::batch_to_nodes(&batch).expect("decode node batch");
    let created = graph
        .batch_create_nodes_with_strategy(graph_id, decoded, "update")
        .await
        .expect("bulk create nodes");
    assert_eq!(created.len(), 3);

    // --- DoGet export half: query the live engine -> encode -> decode.
    let query = proximadb::graph::NodeQuery {
        graph_id: graph_id.to_string(),
        labels: vec!["Person".to_string()],
        filters: Vec::new(),
        limit: None,
        offset: None,
        continuation_token: None,
    };
    let fetched = graph
        .query_nodes(graph_id, query)
        .await
        .expect("query nodes");
    assert_eq!(fetched.len(), 3, "all three nodes are queryable");

    let fetched_nodes: Vec<Node> = fetched.iter().map(|n| (**n).clone()).collect();
    let export = graph_codec::nodes_to_batch(&fetched_nodes).expect("encode fetched");
    let mut round_tripped = graph_codec::batch_to_nodes(&export).expect("decode fetched");
    round_tripped.sort_by(|a, b| a.id.cmp(&b.id));

    // Identity / labels / properties / embedding survive the columnar trip
    // (timestamps are engine-assigned, so compare the stable fields).
    for (orig, got) in originals.iter().zip(round_tripped.iter()) {
        assert_eq!(orig.id, got.id);
        assert_eq!(orig.labels, got.labels);
        assert_eq!(orig.properties, got.properties);
        assert_eq!(
            orig.embedding.as_ref().map(|e| &e.vector),
            got.embedding.as_ref().map(|e| &e.vector),
            "embedding vector preserved for {}",
            orig.id
        );
    }
}

/// Ingest edges via the columnar codec + graph batch API and read them back.
#[tokio::test]
async fn columnar_edge_ingest_and_export_round_trip() {
    use proximadb::graph::model::Edge;
    let graph = Arc::new(GraphOperationsService::new());
    let graph_id = "g_columnar_edges";
    create_graph(&graph, graph_id).await;

    // Edges need endpoints to exist for adjacency; create the nodes first.
    let nodes = vec![node("a", 0), node("b", 0), node("c", 0)];
    let node_batch = graph_codec::nodes_to_batch(&nodes).expect("encode nodes");
    let decoded_nodes = graph_codec::batch_to_nodes(&node_batch).expect("decode nodes");
    graph
        .batch_create_nodes_with_strategy(graph_id, decoded_nodes, "update")
        .await
        .expect("create endpoint nodes");

    let edge = |id: &str, from: &str, to: &str, w: Option<f64>| Edge {
        id: id.to_string(),
        from_node_id: from.to_string(),
        to_node_id: to.to_string(),
        edge_type: "KNOWS".to_string(),
        properties: HashMap::new(),
        weight: w,
        created_at_ms: 0,
        updated_at_ms: 0,
    };
    let originals = vec![edge("e1", "a", "b", Some(0.5)), edge("e2", "b", "c", None)];

    let batch = graph_codec::edges_to_batch(&originals).expect("encode edge batch");
    let decoded = graph_codec::batch_to_edges(&batch).expect("decode edge batch");
    let created = graph
        .batch_create_edges(graph_id, decoded)
        .await
        .expect("bulk create edges");
    assert_eq!(created.len(), 2);

    // Edge export is endpoint-scoped (the engine has no full edge scan), so
    // read each source node's outgoing edges — the columnar `graph_edges` DoGet
    // contract. Round-trip every fetched edge through the codec.
    let query_from = |from: &str| proximadb::graph::EdgeQuery {
        graph_id: graph_id.to_string(),
        from_node_id: Some(from.to_string()),
        to_node_id: None,
        edge_types: vec!["KNOWS".to_string()],
        filters: Vec::new(),
        limit: None,
        offset: None,
        continuation_token: None,
    };
    let mut round_tripped: Vec<Edge> = Vec::new();
    for from in ["a", "b"] {
        let fetched = graph
            .query_edges(graph_id, query_from(from))
            .await
            .expect("query edges by source");
        assert_eq!(fetched.len(), 1, "node {from} has one outgoing KNOWS edge");
        let fetched_edges: Vec<Edge> = fetched.iter().map(|e| (**e).clone()).collect();
        let export = graph_codec::edges_to_batch(&fetched_edges).expect("encode fetched edges");
        round_tripped.extend(graph_codec::batch_to_edges(&export).expect("decode fetched edges"));
    }
    round_tripped.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(round_tripped.len(), 2);

    for (orig, got) in originals.iter().zip(round_tripped.iter()) {
        assert_eq!(orig.id, got.id);
        assert_eq!(orig.from_node_id, got.from_node_id);
        assert_eq!(orig.to_node_id, got.to_node_id);
        assert_eq!(orig.edge_type, got.edge_type);
        assert_eq!(orig.weight, got.weight);
    }
}

/// Read one page of nodes (full scan; empty label set) — mirrors the streaming
/// export's `query_node_page`.
async fn query_page(
    graph: &GraphOperationsService,
    graph_id: &str,
    limit: u32,
    offset: u32,
) -> Vec<Node> {
    let query = proximadb::graph::NodeQuery {
        graph_id: graph_id.to_string(),
        labels: Vec::new(),
        filters: Vec::new(),
        limit: Some(limit),
        offset: Some(offset),
        continuation_token: None,
    };
    let nodes = graph
        .query_nodes(graph_id, query)
        .await
        .expect("query page");
    nodes.iter().map(|n| (**n).clone()).collect()
}

/// The streaming export pages `query_nodes` and re-encodes each page with the
/// dimension fixed from the first page, so every page shares ONE Arrow schema.
/// This verifies that paginated re-encode reconstructs every node and the
/// per-page schema is stable (what the `FlightDataEncoder` requires).
#[tokio::test]
async fn columnar_node_export_paginates_with_fixed_schema() {
    let graph = Arc::new(GraphOperationsService::new());
    let graph_id = "g_columnar_paged";
    create_graph(&graph, graph_id).await;

    // Ingest 5 nodes, all embedding dim 4.
    let originals: Vec<Node> = (0..5).map(|i| node(&format!("n{i}"), 4)).collect();
    let batch = graph_codec::nodes_to_batch(&originals).expect("encode");
    let decoded = graph_codec::batch_to_nodes(&batch).expect("decode");
    graph
        .batch_create_nodes_with_strategy(graph_id, decoded, "update")
        .await
        .expect("bulk create");

    // Fix the schema dimension from the first page (the streaming contract).
    let page = 2u32;
    let first = query_page(&graph, graph_id, page, 0).await;
    let dim = graph_codec::embedding_dim_of(&first).expect("dim");
    assert_eq!(dim, 4);
    let fixed_schema = graph_codec::graph_node_schema(dim);

    // Page through, re-encoding each page with the FIXED dim; every page must
    // carry the identical schema, and the union reconstructs all nodes.
    let mut all: Vec<Node> = Vec::new();
    let mut offset = 0u32;
    loop {
        let nodes = if offset == 0 {
            first.clone()
        } else {
            query_page(&graph, graph_id, page, offset).await
        };
        if nodes.is_empty() {
            break;
        }
        let b = graph_codec::nodes_to_batch_with_dim(&nodes, dim as usize).expect("encode page");
        assert_eq!(b.schema(), fixed_schema, "every page shares one schema");
        all.extend(graph_codec::batch_to_nodes(&b).expect("decode page"));
        let n = nodes.len();
        offset += page;
        if (n as u32) < page {
            break;
        }
    }

    let mut ids: Vec<String> = all.iter().map(|n| n.id.clone()).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec!["n0", "n1", "n2", "n3", "n4"],
        "pagination reconstructs every node exactly once"
    );
}

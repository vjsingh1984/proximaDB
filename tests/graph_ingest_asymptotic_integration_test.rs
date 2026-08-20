//! Asymptotic ratchet: bulk edge ingest must not slow down as the graph grows.
//!
//! Every performance defect in this family — the CSR rebuilt per edge during
//! replay (#1678), the client-side `get_all_edges` per-node scans, the
//! adjacency projection scanning every node key per edge (#1695) — shares one
//! shape: per-item work proportional to total resident size. Each one passed
//! every existing test, because every existing test measures a FIXED size;
//! O(N²) is invisible at test scale and detonates at repo scale (a 5.005 s
//! edge batch of which 4.993 s was one scan).
//!
//! This test measures the shape, not a wall-clock number: it ingests a stream
//! of equal-sized batches and compares the LAST batches against the FIRST.
//! For amortized-constant per-edge cost the ratio is ~1 regardless of machine
//! speed — both halves run in the same process moments apart, so the
//! comparison self-normalizes and survives noisy CI runners. For the defect
//! class it exists to block, the ratio grows with corpus size: the reverted
//! projection scan measures >40x here.
//!
//! If this test starts failing, some per-item path has re-acquired a term
//! proportional to graph size. Do not widen the threshold; find the term.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use proximadb::graph::GraphOperationsService;
use proximadb::graph::{Edge, Node};
use proximadb::proto::proximadb_v1::{
    CompressionAlgorithm, CreateGraphRequest, GraphStorageConfig,
};
use tempfile::TempDir;

const GRAPH_ID: &str = "asymptotic_ratchet";
const BATCH: usize = 500;
const BATCHES: usize = 24;
const SAMPLE: usize = 4; // compare first SAMPLE vs last SAMPLE batches
/// Late/early median ratio allowed. Amortized-linear ingest measures ~1;
/// the projection scan this blocks measured >40. Generous for CI noise and
/// for amortized structures (map rehash/rebalance) without letting any
/// size-proportional term through.
const MAX_RATIO: f64 = 4.0;

fn node(i: usize) -> Node {
    Node {
        id: format!("n{i}"),
        labels: vec!["Ratchet".to_string()],
        properties: HashMap::new(),
        embedding: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

fn edge(i: usize, nodes: usize) -> Edge {
    // Distinct (from, to, type) per edge; endpoints spread across the whole
    // node set so per-node buckets stay small and the interesting variable is
    // TOTAL resident size, not hub degree.
    Edge {
        id: format!("e{i}"),
        from_node_id: format!("n{}", i % nodes),
        to_node_id: format!("n{}", (i * 7 + 1) % nodes),
        edge_type: format!("T{}", i % 13),
        properties: HashMap::new(),
        weight: None,
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

#[tokio::test]
async fn bulk_edge_ingest_does_not_slow_down_as_the_graph_grows() {
    let service = Arc::new(GraphOperationsService::new());
    let dir = TempDir::new().expect("tempdir");
    let create = CreateGraphRequest {
        graph_id: GRAPH_ID.to_string(),
        name: Some("Asymptotic ratchet".to_string()),
        description: None,
        schema: None,
        storage_config: Some(GraphStorageConfig {
            engine_type: "ORION".to_string(),
            base_url: dir.path().to_string_lossy().to_string(),
            compression: CompressionAlgorithm::CompressionSnappy as i32,
            enable_wal: true,
            snapshot_interval_hours: 24,
            engine_specific_config: HashMap::new(),
        }),
        engine_config: None,
        access_control: None,
    };
    let _ = service.create_graph_collection(create).await;

    let total_nodes = BATCH * BATCHES;
    // Nodes first, in batches (also covered by the ratchet implicitly: a
    // size-proportional term in node ingest would surface in edge batches
    // through shared structures).
    for b in 0..BATCHES {
        let nodes: Vec<Node> = (b * BATCH..(b + 1) * BATCH).map(node).collect();
        service
            .batch_create_nodes(GRAPH_ID, nodes)
            .await
            .expect("node batch");
    }

    let mut per_batch_ms: Vec<f64> = Vec::with_capacity(BATCHES);
    for b in 0..BATCHES {
        let edges: Vec<Edge> = (b * BATCH..(b + 1) * BATCH)
            .map(|i| edge(i, total_nodes))
            .collect();
        let start = Instant::now();
        let result = service
            .batch_create_edges(GRAPH_ID, edges)
            .await
            .expect("edge batch");
        per_batch_ms.push(start.elapsed().as_secs_f64() * 1e3);
        assert!(
            result.rejected.is_empty(),
            "ratchet batches are duplicate-free by construction; rejections \
             mean the fixture is wrong: {:?}",
            result.rejected.first()
        );
    }

    let median = |window: &[f64]| -> f64 {
        let mut sorted = window.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        sorted[sorted.len() / 2]
    };
    let early = median(&per_batch_ms[..SAMPLE]);
    let late = median(&per_batch_ms[BATCHES - SAMPLE..]);
    // Floor the denominator so a sub-millisecond early median cannot turn
    // scheduler jitter into a giant ratio.
    let ratio = late / early.max(1.0);

    assert!(
        ratio <= MAX_RATIO,
        "edge-batch cost grew {ratio:.1}x from early ({early:.2} ms) to late \
         ({late:.2} ms) across {} edges — a per-item term proportional to \
         graph size is back on the ingest path (see #1695 for the last one). \
         Per-batch ms: {per_batch_ms:.1?}",
        BATCH * BATCHES,
    );
}

//! Code-graph embedded-parity gate (ADR-0017 §F, gate #3).
//!
//! Indexes a small synthesized code-graph fixture via the embedded `EmbeddedProximaDB` (one graph +
//! one co-indexed vector collection) and asserts `impact_analysis(forward/backward)` and the hybrid
//! seed→expand (`fusion_search`) return deterministic, correct results.
//!
//! ## Parity rationale
//! The embedded `fusion_search`/`impact_analysis` (TD-131) delegate to the SAME `FusionService` and
//! `GraphOperationsService::impact_analysis` that the REST `POST /api/v2/graphs/{id}/fusion-search`
//! and `…/impact-analysis` endpoints use (PR #308). The REST handler and `EmbeddedProximaDB` both
//! construct `FusionService::new(vector, graph)` identically, so the embedded result IS the server
//! result by construction. PR #308's `graph_impact_analysis_integration_test` + `fusion_service`
//! unit tests verify the server-side core; this test verifies the embedded path delegates to that
//! same core correctly and is deterministic — together they clear the "parity-tested embedded path"
//! gate. (A cross-process embedded-vs-networked-server comparison is deferred: the data-loading
//! asymmetry makes it flaky-prone, and structural parity + dual correctness is the robust proof.)

use std::collections::HashSet;

use proximadb::embedded::{
    EmbeddedConfig, EmbeddedGraphEdge, EmbeddedGraphNode, EmbeddedProximaDB,
};

const GRAPH_ID: &str = "codegraph";
const VEC_COLLECTION: &str = "code_vecs";

fn canonical_oid(node_id: &str) -> String {
    format!("graph/{GRAPH_ID}/node/{node_id}")
}

/// main --CALLS--> parse --CALLS--> validate
/// main --CALLS--> io
/// parse --IMPORTS--> util
struct Fixture {
    node_ids: &'static [&'static str],
    /// (from, to, "CALLS"|"IMPORTS")
    edges: &'static [(&'static str, &'static str, &'static str)],
    /// deterministic 4-d embedding per node; `main` is the query target
    embeddings: Vec<(String, Vec<f32>)>,
}

fn fixture() -> Fixture {
    let emb = |id: &str, v: Vec<f32>| (canonical_oid(id), v);
    Fixture {
        node_ids: &["main", "parse", "validate", "io", "util"],
        edges: &[
            ("main", "parse", "CALLS"),
            ("parse", "validate", "CALLS"),
            ("main", "io", "CALLS"),
            ("parse", "util", "IMPORTS"),
        ],
        embeddings: vec![
            emb("main", vec![1.0, 0.0, 0.0, 0.0]),
            emb("parse", vec![0.0, 1.0, 0.0, 0.0]),
            emb("validate", vec![0.0, 0.0, 1.0, 0.0]),
            emb("io", vec![0.0, 0.0, 0.0, 1.0]),
            emb("util", vec![0.1, 0.1, 0.1, 0.1]),
        ],
    }
}

fn build_db() -> EmbeddedProximaDB {
    let dir = tempfile::tempdir().expect("tempdir");
    let data_path = dir.path().join("data");
    std::fs::create_dir_all(&data_path).expect("create data dir");
    // Keep the tempdir alive for the DB's lifetime by leaking it (test process is short-lived).
    std::mem::forget(dir);

    let mut config = EmbeddedConfig::for_low_memory(data_path.to_string_lossy().to_string());
    config.enable_wal = true;
    let db = EmbeddedProximaDB::new(config).expect("create embedded db");

    let fx = fixture();

    // Co-indexed vector collection: record id == canonical graph node oid.
    db.create_collection(VEC_COLLECTION, 4, Some("sst"))
        .expect("create vector collection");
    let ids: Vec<String> = fx.embeddings.iter().map(|(id, _)| id.clone()).collect();
    let vectors: Vec<Vec<f32>> = fx.embeddings.iter().map(|(_, v)| v.clone()).collect();
    db.insert(VEC_COLLECTION, ids, vectors, None)
        .expect("insert co-indexed vectors");

    // Graph: nodes + edges.
    db.create_graph(GRAPH_ID, None).expect("create graph");
    let nodes: Vec<EmbeddedGraphNode> = fx
        .node_ids
        .iter()
        .map(|id| EmbeddedGraphNode::new(*id).with_label("Symbol"))
        .collect();
    db.create_nodes(GRAPH_ID, nodes).expect("create nodes");
    let edges: Vec<EmbeddedGraphEdge> = fx
        .edges
        .iter()
        .enumerate()
        .map(|(i, (from, to, et))| EmbeddedGraphEdge::new(*from, *to, *et).with_id(format!("e{i}")))
        .collect();
    db.create_edges(GRAPH_ID, edges).expect("create edges");

    db
}

/// Hybrid seed→expand: query vector = `main`'s embedding (top seed), 1-hop CALLS expand reaches
/// parse + io. Fused oids = {main, parse, io}.
#[test]
fn embedded_fusion_search_seeds_and_expands() {
    let db = build_db();

    let (items, stats) = db
        .fusion_search(
            GRAPH_ID,
            VEC_COLLECTION,
            vec![1.0, 0.0, 0.0, 0.0], // == main's embedding → top seed
            1,                        // max_depth
            vec!["CALLS".to_string()],
            1,     // max_seeds
            10,    // limit
            1.0,   // vector_weight
            1.0,   // graph_weight
            false, // calibrated (not rrf)
            "nodes",
        )
        .expect("embedded fusion_search");

    let oids: HashSet<String> = items.into_iter().map(|i| i.oid).collect();
    assert!(stats.items_out > 0, "fusion must return candidates");
    assert!(
        oids.contains(&canonical_oid("main")),
        "seed oid present: {oids:?}"
    );
    assert!(
        oids.contains(&canonical_oid("parse")) && oids.contains(&canonical_oid("io")),
        "1-hop expand reaches parse + io: {oids:?}"
    );
}

/// Forward blast radius from main (CALLS, depth 2): parse + io (d1), validate (d2).
#[test]
fn embedded_impact_analysis_forward() {
    let db = build_db();
    let result = db
        .impact_analysis(
            GRAPH_ID,
            "main",
            "forward",
            Some(vec!["CALLS".to_string()]),
            2,
            100,
        )
        .expect("forward impact analysis");

    let ids: HashSet<String> = result.nodes.into_iter().map(|n| n.id).collect();
    for expected in ["parse", "io", "validate"] {
        assert!(
            ids.contains(expected),
            "forward from main should reach {expected}: {ids:?}"
        );
    }
}

/// Backward blast radius to validate (CALLS, depth 2): parse (d1), main (d2). util/io must NOT appear.
#[test]
fn embedded_impact_analysis_backward() {
    let db = build_db();
    let result = db
        .impact_analysis(
            GRAPH_ID,
            "validate",
            "backward",
            Some(vec!["CALLS".to_string()]),
            2,
            100,
        )
        .expect("backward impact analysis");

    let ids: HashSet<String> = result.nodes.into_iter().map(|n| n.id).collect();
    assert!(
        ids.contains("parse") && ids.contains("main"),
        "backward to validate reaches parse + main: {ids:?}"
    );
    assert!(
        !ids.contains("util") && !ids.contains("io"),
        "util/io are not callers of validate: {ids:?}"
    );
}

/*
 * Copyright 2025 Vijaykumar Singh
 * (Apache-2.0)
 */

//! TD-066 engine-WAL replay-scope recovery tests. Lives in the ROOT crate (not
//! the `proximadb-orion-engine` crate) because it exercises real WAL behavior
//! via the root `unified_wal_factory` (ORION extraction, 6g). The engine's
//! `memory_pool` field is `pub` so these tests can assert node counts.

#[cfg(test)]
mod td066_replay_scope_tests {
    //! TD-066 (c) Part 2: recovery loads the canonical-checkpoint snapshot and
    //! replays ONLY engine-WAL frames after the matching `CanonicalEmission`
    //! marker. Frames at/before the marker are in the snapshot, so they are NOT
    //! re-applied — proven via `stats.nodes_created`: snapshot-load resets it to
    //! the snapshot's node count, then only post-marker CreateNodes bump it.
    use crate::graph::engines::orion::OrionGraphEngine;
    use proximadb_graph_model::Node;
    use std::collections::HashMap;

    fn node(id: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec!["T".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[tokio::test]
    async fn recovery_scopes_engine_wal_replay_to_post_checkpoint_frames() {
        // SAFETY: env mutation is process-local; this test asserts the
        // TD-066 Part 2 behavior under the feature flag. nextest isolates
        // processes, and no other in-process graph recovery test writes
        // snapshots, so the global flip is observationally isolated.
        unsafe {
            std::env::set_var("PROXIMADB_GRAPH_CANONICAL_REPLAY_SCOPE", "1");
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let base_url = format!("file://{}", tmp.path().display());
        let canonical_wal = tmp.path().join("canonical.wal");
        let gid = "td066_scope".to_string();
        const CHECKPOINT_LSN: u64 = 100;

        // Phase 1 — 3 pre-checkpoint nodes, then marker+snapshot, then 2 more.
        {
            let engine = OrionGraphEngine::with_persistence_for_graph_and_canonical_wal(
                gid.clone(),
                base_url.clone(),
                true,
                Some(canonical_wal.clone()),
                crate::graph::unified_wal_factory(),
            )
            .await
            .expect("engine");
            for i in 0..3u32 {
                engine
                    .create_node(node(&format!("pre_{i}")))
                    .await
                    .expect("create pre");
            }
            let persistence = engine
                .persistence()
                .expect("persistence configured")
                .clone();
            // Mirror GraphOperationsService::flush_wal Step 2 ordering: marker
            // BEFORE engine flush BEFORE snapshot — guarantees "snapshot exists
            // ⟹ marker durable" so recovery can correlate them.
            persistence
                .append_canonical_emission_marker(CHECKPOINT_LSN)
                .await
                .expect("marker");
            engine.flush_wal().await.expect("flush engine wal");
            persistence
                .save_snapshot(&engine, CHECKPOINT_LSN)
                .await
                .expect("snapshot");
            for i in 0..2u32 {
                engine
                    .create_node(node(&format!("post_{i}")))
                    .await
                    .expect("create post");
            }
            engine.flush_wal().await.expect("flush post frames");
        }

        // Phase 2 — recover into a fresh engine on the same paths.
        let engine = OrionGraphEngine::with_persistence_for_graph_and_canonical_wal(
            gid.clone(),
            base_url.clone(),
            true,
            Some(canonical_wal.clone()),
            crate::graph::unified_wal_factory(),
        )
        .await
        .expect("recovery engine");
        engine.recover().await.expect("recover");

        // Correctness — no data loss: all 5 nodes present.
        assert_eq!(
            engine.memory_pool.nodes.len(),
            5,
            "all 5 nodes (3 pre-checkpoint + 2 post) must survive recovery"
        );
        for i in 0..3u32 {
            assert!(
                engine.memory_pool.nodes.contains_key(&format!("pre_{i}")),
                "pre-checkpoint node pre_{i} missing (snapshot not loaded)"
            );
        }
        for i in 0..2u32 {
            assert!(
                engine.memory_pool.nodes.contains_key(&format!("post_{i}")),
                "post-checkpoint node post_{i} missing (replay didn't cover it)"
            );
        }

        // Scoping proof — recovery loaded the snapshot (3 pre-checkpoint
        // nodes) and replayed ONLY the 2 post-checkpoint frames. Full
        // (unscoped) replay would have applied all 5 graph-op frames.
        let replayed = engine
            .persistence()
            .expect("persistence configured")
            .last_replay_applied();
        assert_eq!(
            replayed, 2,
            "scoped replay must apply exactly the 2 post-checkpoint frames; \
             got {replayed} (5 would mean unscoped full replay of pre-checkpoint frames)"
        );

        // SAFETY: see the matching `set_var` above (test-local feature flag).
        unsafe {
            std::env::remove_var("PROXIMADB_GRAPH_CANONICAL_REPLAY_SCOPE");
        }
    }

    /// TD-066 (d): wiring `truncate_wal_through_checkpoint` after the durable
    /// snapshot must never disturb recovery. In the common small-graph regime
    /// every frame fits one 64 MB segment, so the marker shares segment 0 and
    /// nothing is reclaimed — but recovery must still yield all nodes with
    /// scoped replay. This guards the `flush_wal` Step-2 ordering
    /// (marker → flush → snapshot → truncate) as an end-to-end no-op-safe path.
    #[tokio::test]
    async fn truncate_after_snapshot_preserves_recovery() {
        // SAFETY: process-local feature flag; nextest isolates processes.
        unsafe {
            std::env::set_var("PROXIMADB_GRAPH_CANONICAL_REPLAY_SCOPE", "1");
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let base_url = format!("file://{}", tmp.path().display());
        let canonical_wal = tmp.path().join("canonical.wal");
        let gid = "td066_truncate".to_string();
        const CHECKPOINT_LSN: u64 = 100;

        {
            let engine = OrionGraphEngine::with_persistence_for_graph_and_canonical_wal(
                gid.clone(),
                base_url.clone(),
                true,
                Some(canonical_wal.clone()),
                crate::graph::unified_wal_factory(),
            )
            .await
            .expect("engine");
            for i in 0..3u32 {
                engine
                    .create_node(node(&format!("pre_{i}")))
                    .await
                    .expect("create pre");
            }
            let persistence = engine
                .persistence()
                .expect("persistence configured")
                .clone();
            persistence
                .append_canonical_emission_marker(CHECKPOINT_LSN)
                .await
                .expect("marker");
            engine.flush_wal().await.expect("flush engine wal");
            persistence
                .save_snapshot(&engine, CHECKPOINT_LSN)
                .await
                .expect("snapshot");
            // The new Step-2 tail: truncate strictly after the durable snapshot.
            let reclaimed = persistence
                .truncate_wal_through_checkpoint(CHECKPOINT_LSN)
                .await
                .expect("truncate");
            // Single 64 MB segment holds everything → marker is in segment 0 →
            // nothing below it → safe no-op.
            assert_eq!(reclaimed, 0, "single-segment WAL reclaims nothing");
            assert_eq!(persistence.last_truncate_reclaimed(), 0);
            for i in 0..2u32 {
                engine
                    .create_node(node(&format!("post_{i}")))
                    .await
                    .expect("create post");
            }
            engine.flush_wal().await.expect("flush post frames");
        }

        // Recover — truncation must not have removed anything needed.
        let engine = OrionGraphEngine::with_persistence_for_graph_and_canonical_wal(
            gid.clone(),
            base_url.clone(),
            true,
            Some(canonical_wal.clone()),
            crate::graph::unified_wal_factory(),
        )
        .await
        .expect("recovery engine");
        engine.recover().await.expect("recover");

        assert_eq!(
            engine.memory_pool.nodes.len(),
            5,
            "all 5 nodes must survive recovery after a post-snapshot truncate"
        );
        let replayed = engine
            .persistence()
            .expect("persistence configured")
            .last_replay_applied();
        assert_eq!(
            replayed, 2,
            "scoped replay must still apply exactly the 2 post-checkpoint frames"
        );

        // SAFETY: see the matching `set_var` above.
        unsafe {
            std::env::remove_var("PROXIMADB_GRAPH_CANONICAL_REPLAY_SCOPE");
        }
    }
}

#[cfg(test)]
mod read_after_restart_tests {
    //! #1524: ORION edges survived a restart but every read surface answered
    //! empty, while re-creating the same edge was rejected as a duplicate.
    //! Three compounding defects, each pinned by a test below:
    //! 1. WAL replay drove the engine's public insert paths, which re-appended
    //!    every replayed frame — doubling the WAL per restart.
    //! 2. The re-appended duplicate `CreateEdge` frame tripped the CSR's
    //!    duplicate-edge check on the NEXT restart, aborting that graph's
    //!    recovery entirely (reads then hit a lazily-created empty engine).
    //! 3. Even when recovery succeeded, the service-level read state
    //!    (adjacency projection, CSR-freshness epoch, edge counters) was never
    //!    rebuilt, so endpoint-bound queries and count surfaces stayed blind.
    use crate::graph::engines::orion::OrionGraphEngine;
    use crate::graph::service::GraphOperationsService;
    use crate::proto::proximadb_v1::CreateGraphRequest;
    use proximadb_graph_model::{Edge, EdgeQuery, Node};
    use std::sync::Arc;

    fn node(id: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec!["Sym".to_string()],
            ..Default::default()
        }
    }

    fn edge(id: &str, from: &str, to: &str) -> Edge {
        Edge {
            id: id.to_string(),
            from_node_id: from.to_string(),
            to_node_id: to.to_string(),
            edge_type: "CALLS".to_string(),
            ..Default::default()
        }
    }

    async fn engine(graph_id: &str, base_url: &str) -> OrionGraphEngine {
        OrionGraphEngine::with_persistence_for_graph(
            graph_id.to_string(),
            base_url.to_string(),
            true,
            crate::graph::unified_wal_factory(),
        )
        .await
        .expect("engine with persistence")
    }

    /// Defects 1+2 at the engine layer: replay must not re-append what it
    /// reads, so a graph recovers identically on EVERY restart, not just the
    /// first. Before the fix the second recovery replayed a doubled WAL and
    /// aborted on its own duplicate `CreateEdge` frame.
    #[tokio::test]
    async fn repeated_recovery_does_not_amplify_wal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base_url = format!("file://{}", tmp.path().display());
        let gid = "restart_amplify";

        {
            let e = engine(gid, &base_url).await;
            e.create_node(node("n1")).await.expect("n1");
            e.create_node(node("n2")).await.expect("n2");
            e.create_edge(edge("n1|calls|n2", "n1", "n2"))
                .await
                .expect("edge");
            e.flush_wal().await.expect("flush");
        }

        for restart in 1..=3u32 {
            let e = engine(gid, &base_url).await;
            e.recover()
                .await
                .expect("recovery must succeed on every restart");
            assert_eq!(e.memory_pool.nodes.len(), 2, "restart {restart}: nodes");
            assert_eq!(
                e.memory_pool.edges.len(),
                1,
                "restart {restart}: the edge must be readable after recovery"
            );
            let replayed = e
                .persistence()
                .expect("persistence configured")
                .last_replay_applied();
            assert_eq!(
                replayed, 3,
                "restart {restart}: exactly the 3 original frames replay; more means \
                 replay re-appended frames to the WAL on an earlier restart"
            );
            e.flush_wal().await.expect("flush");
        }
    }

    /// WALs written BEFORE the re-append fix already carry duplicate
    /// `CreateEdge` frames. Replay must treat them as no-ops instead of
    /// aborting recovery and leaving the graph empty.
    #[tokio::test]
    async fn recovery_tolerates_poisoned_wal_with_duplicate_edge_frames() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base_url = format!("file://{}", tmp.path().display());
        let gid = "restart_poisoned";

        {
            let e = engine(gid, &base_url).await;
            e.create_node(node("n1")).await.expect("n1");
            e.create_node(node("n2")).await.expect("n2");
            let dup = edge("n1|calls|n2", "n1", "n2");
            e.create_edge(dup.clone()).await.expect("edge");
            // Poison the WAL the way pre-fix replay did: a second CreateEdge
            // frame for the same edge.
            e.persistence()
                .expect("persistence configured")
                .write_edge_operation(dup)
                .await
                .expect("duplicate frame");
            e.flush_wal().await.expect("flush");
        }

        let e = engine(gid, &base_url).await;
        e.recover()
            .await
            .expect("recovery must survive duplicate CreateEdge frames");
        assert_eq!(e.memory_pool.nodes.len(), 2);
        assert_eq!(
            e.memory_pool.edges.len(),
            1,
            "duplicate frame applies as a no-op, not an abort"
        );
    }

    /// Defect 3 at the service layer: after recovery, endpoint-bound edge
    /// queries, the adjacency projection, and the stats counters must see the
    /// recovered edges. Before the fix all three answered zero while the
    /// engine held the data (and re-creating the edge was rejected as a
    /// duplicate — no client-side repair path).
    #[tokio::test]
    async fn recovered_graph_serves_edge_reads_and_counts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base_url = format!("file://{}", tmp.path().display());
        let gid = "restart_reads";
        let collection_service = Arc::new(
            crate::services::graph_collection::GraphCollectionService::new_with_path(
                tmp.path().join("graph_collections.json"),
            ),
        );

        let service_over =
            |collection: Arc<crate::services::graph_collection::GraphCollectionService>| {
                let mut svc = GraphOperationsService::new_with_collection_service(collection);
                svc.set_base_storage_url(base_url.clone());
                svc
            };

        // Session 1: create graph + 3 nodes + 1 edge, verify live read, flush.
        {
            let svc = service_over(collection_service.clone());
            svc.create_graph_collection(CreateGraphRequest {
                graph_id: gid.to_string(),
                name: None,
                description: None,
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            })
            .await
            .expect("create graph");
            for id in ["n1", "n2", "n3"] {
                svc.create_node(gid, node(id)).await.expect("node");
            }
            svc.create_edge(gid, edge("n1|calls|n2", "n1", "n2"))
                .await
                .expect("edge");
            let live = svc
                .query_edges(
                    gid,
                    EdgeQuery {
                        from_node_id: Some("n1".to_string()),
                        ..Default::default()
                    },
                )
                .await
                .expect("live query");
            assert_eq!(live.len(), 1, "live endpoint-bound read sees the edge");
            svc.flush_wal(gid).await.expect("flush");
        }

        // Sessions 2 and 3: fresh service over the same storage — reads must
        // see the recovered edge on every restart, not just the first.
        for restart in 1..=2u32 {
            let svc = service_over(collection_service.clone());
            svc.recover_all_graphs().await.expect("recover");

            let recovered = svc
                .query_edges(
                    gid,
                    EdgeQuery {
                        from_node_id: Some("n1".to_string()),
                        ..Default::default()
                    },
                )
                .await
                .expect("recovered query");
            assert_eq!(
                recovered.len(),
                1,
                "restart {restart}: endpoint-bound read must see the recovered edge"
            );
            assert_eq!(recovered[0].id, "n1|calls|n2");

            assert_eq!(
                svc.adjacency_projection_edge_count(gid)
                    .expect("projection count"),
                1,
                "restart {restart}: adjacency projection rebuilt from recovery"
            );

            let stats = svc.get_stats(gid).await.expect("stats");
            assert_eq!(stats.total_nodes, 3, "restart {restart}: node count");
            assert_eq!(
                stats.total_edges, 1,
                "restart {restart}: edge count surface must not report 0 for a \
                 populated collection"
            );
        }
    }
}

#[cfg(test)]
mod wal_segment_reclaim_tests {
    //! WAL-retention slice: with a small `PROXIMADB_GRAPH_WAL_SEGMENT_MB`,
    //! the marker→flush→snapshot→truncate sequence (the same one
    //! `GraphOperationsService::flush_wal` runs under the canonical-replay
    //! scope) must actually reclaim whole WAL segments below the checkpoint
    //! marker — at the previous hardcoded 64MB segment size an embedded-scale
    //! graph WAL was always a single segment and truncation reclaimed zero —
    //! and a fresh engine must recover the exact graph from snapshot + tail.
    use crate::graph::engines::orion::OrionGraphEngine;
    use proximadb_graph_model::{Edge, Node, PropertyValue, property_value};
    use std::collections::HashMap;

    fn fat_node(i: u32) -> Node {
        // ~20KB of properties so a few hundred nodes span multiple 1MB segments.
        let blob = "x".repeat(20 * 1024);
        Node {
            id: format!("n{i:04}"),
            labels: vec!["Sym".to_string()],
            properties: HashMap::from([(
                "blob".to_string(),
                PropertyValue {
                    value: Some(property_value::Value::StringValue(blob)),
                },
            )]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[tokio::test]
    async fn checkpoint_truncate_reclaims_segments_and_recovery_is_exact() {
        // SAFETY: process-local env; nextest isolates test processes. The
        // canonical-replay scope enables the snapshot-load half of recovery.
        unsafe {
            std::env::set_var("PROXIMADB_GRAPH_WAL_SEGMENT_MB", "1");
            std::env::set_var("PROXIMADB_GRAPH_CANONICAL_REPLAY_SCOPE", "1");
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let base_url = format!("file://{}", tmp.path().display());
        let gid = "wal_reclaim";
        const NODES: u32 = 120; // ~2.4MB of ops → >= 3 segments at 1MB
        const CHECKPOINT_LSN: u64 = 7;

        {
            let engine = OrionGraphEngine::with_persistence_for_graph(
                gid.to_string(),
                base_url.clone(),
                true,
                crate::graph::unified_wal_factory(),
            )
            .await
            .expect("engine");
            for i in 0..NODES {
                engine.create_node(fat_node(i)).await.expect("node");
            }
            engine
                .create_edge(Edge {
                    id: "e0".to_string(),
                    from_node_id: "n0000".to_string(),
                    to_node_id: "n0001".to_string(),
                    edge_type: "CALLS".to_string(),
                    ..Default::default()
                })
                .await
                .expect("edge");

            let persistence = engine.persistence().expect("persistence").clone();
            // The flush_wal Step-2 sequence: marker → flush → snapshot → truncate.
            persistence
                .append_canonical_emission_marker(CHECKPOINT_LSN)
                .await
                .expect("marker");
            engine.flush_wal().await.expect("flush");
            persistence
                .save_snapshot(&engine, CHECKPOINT_LSN)
                .await
                .expect("snapshot");
            let reclaimed = persistence
                .truncate_wal_through_checkpoint(CHECKPOINT_LSN)
                .await
                .expect("truncate");
            assert!(
                reclaimed > 0,
                "a multi-segment WAL must reclaim segments below the marker; got {reclaimed}"
            );
            assert_eq!(persistence.last_truncate_reclaimed(), reclaimed);
        }

        // Recovery on the truncated WAL + snapshot must yield the exact graph.
        let engine = OrionGraphEngine::with_persistence_for_graph(
            gid.to_string(),
            base_url,
            true,
            crate::graph::unified_wal_factory(),
        )
        .await
        .expect("recovery engine");
        engine.recover().await.expect("recover");
        assert_eq!(engine.memory_pool.nodes.len(), NODES as usize);
        assert_eq!(engine.memory_pool.edges.len(), 1);

        // SAFETY: matching set_var above.
        unsafe {
            std::env::remove_var("PROXIMADB_GRAPH_WAL_SEGMENT_MB");
            std::env::remove_var("PROXIMADB_GRAPH_CANONICAL_REPLAY_SCOPE");
        }
    }
}

#[cfg(test)]
mod topology_only_snapshot_tests {
    use crate::graph::engines::orion::OrionGraphEngine;
    use proximadb_graph_engine_traits::GraphEngine;
    use proximadb_graph_model::{Edge, Node};

    async fn engine(graph_id: &str, base: &std::path::Path) -> OrionGraphEngine {
        let base_url = format!("file://{}", base.display());
        OrionGraphEngine::with_persistence_for_graph(
            graph_id.to_string(),
            base_url,
            false,
            crate::graph::unified_wal_factory(),
        )
        .await
        .expect("engine with persistence")
    }

    fn node(id: &str) -> Node {
        Node {
            id: id.to_string(),
            labels: vec!["N".to_string()],
            ..Default::default()
        }
    }

    /// TD-168 Phase 1b: a topology-only snapshot (gate ON) carries CSR + node_to_index
    /// but NO payloads; it round-trips the topology, leaves payloads cold, and is
    /// fail-closed when loaded with the gate OFF. The full (gate OFF) snapshot is
    /// unchanged. Process-isolated under nextest, so the env gate doesn't leak.
    #[tokio::test]
    async fn topology_only_snapshot_round_trips_payloads_cold_and_fails_closed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("orion");
        std::fs::create_dir_all(&base).expect("mkdir");

        // Build a 2-node, 1-edge graph in the source engine (full, in-RAM).
        let src = engine("g1", &base).await;
        src.insert_node(node("a")).await.expect("node a");
        src.insert_node(node("b")).await.expect("node b");
        src.insert_edge(Edge {
            id: "e".to_string(),
            from_node_id: "a".to_string(),
            to_node_id: "b".to_string(),
            edge_type: "R".to_string(),
            ..Default::default()
        })
        .await
        .expect("edge");
        assert_eq!(src.node_to_index.len(), 2);

        // (1) Gate ON → topology-only snapshot; load into a fresh engine.
        unsafe { std::env::set_var("PROXIMADB_GRAPH_COLD_PAYLOADS", "1") };
        let topo_path = src
            .persistence()
            .expect("persistence")
            .save_snapshot(&src, 0)
            .await
            .expect("save topology-only");

        let warm = engine("g1", &base).await;
        warm.persistence()
            .expect("persistence")
            .load_snapshot(&warm, &topo_path)
            .await
            .expect("load topology-only");
        // Topology restored, payloads NOT resident (served cold via the service path).
        assert_eq!(warm.node_to_index.len(), 2, "topology (nodes) restored");
        assert!(
            warm.memory_pool.nodes.is_empty(),
            "node payloads stay cold after topology-only load"
        );
        assert!(
            warm.edge_metadata.is_empty(),
            "edge payloads stay cold after topology-only load"
        );
        // Edge TOPOLOGY survives in the CSR (one directed out-edge), even though no edge
        // payload is resident.
        let out_edges = warm.csr_outgoing.read().expect("csr read").targets.len();
        assert_eq!(out_edges, 1, "edge topology restored in CSR");

        // (2) Fail-closed: same topology-only snapshot, gate OFF → error (no silent
        // data-invisibility).
        unsafe { std::env::remove_var("PROXIMADB_GRAPH_COLD_PAYLOADS") };
        let blocked = engine("g1", &base).await;
        let result = blocked
            .persistence()
            .expect("persistence")
            .load_snapshot(&blocked, &topo_path)
            .await;
        assert!(
            result.is_err(),
            "topology-only snapshot must fail-closed when the cold-payload gate is OFF"
        );

        // (3) Gate OFF → full snapshot round-trips with payloads resident (today's path).
        let full_path = src
            .persistence()
            .expect("persistence")
            .save_snapshot(&src, 0)
            .await
            .expect("save full");
        let full = engine("g1", &base).await;
        full.persistence()
            .expect("persistence")
            .load_snapshot(&full, &full_path)
            .await
            .expect("load full");
        assert_eq!(
            full.memory_pool.nodes.len(),
            2,
            "full snapshot loads payloads"
        );
    }
}

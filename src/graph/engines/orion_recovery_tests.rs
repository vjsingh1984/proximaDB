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

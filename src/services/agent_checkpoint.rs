// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Server-composed agent checkpoint store (ADR-022, TD-MCP-1 phase MCP-1d).
//!
//! Backs the MCP `checkpoint` tool. Checkpoints are **event-sourced** over the
//! already-composed [`EventLogEngine`] rather than a bespoke store: a checkpoint
//! save is naturally an append, `parent_checkpoint_id` lineage is exactly an
//! event chain, and ADR-022's auditable trail is the same substrate. That reuse
//! is the reason this needs no new engine, filesystem handle, or LLM.
//!
//! Isolation is **fail-closed**: every operation requires a non-empty
//! `tenant_id`, and the tenant is a structural component of the entity id, so
//! one tenant's stream is not addressable from another's scope (mirrors
//! `MemoryAqlSource` / the MCP `memory` tool).
//!
//! The request/response shape follows the LangGraph checkpointer contract
//! already declared in `proximadb-api` (`CheckpointServiceRequest`):
//! `thread_id` / `checkpoint_ns` / `checkpoint_id` / `parent_checkpoint_id`.
//!
//! # Durability (TD-EVENTLOG-1)
//!
//! Checkpoints survive a restart **provided the engine was built with
//! [`EventLogEngine::open`]**, which recovers the sequence counter and rebuilds
//! the index. `EventLogEngine::new` does neither, so a store constructed over a
//! `new` engine silently loses prior checkpoints and can overwrite persisted
//! events. `SharedServices` uses `open`; anything else composing this store must
//! do the same.
//!
//! Concurrency is *not* yet safe: the engine still derives immutable object keys
//! from a process-local counter, so two writers over one base can collide. Until
//! the TD-EVENTLOG-1 re-key lands, treat a checkpoint stream as single-writer.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::storage::engines::eventlog::{Event, EventLogEngine};

/// Event type recorded for every checkpoint save.
pub const CHECKPOINT_EVENT_TYPE: &str = "CheckpointSaved";

/// Entity-id prefix for checkpoint streams.
const ENTITY_PREFIX: &str = "agent-checkpoint";

/// Default namespace when the caller does not scope one.
const DEFAULT_NS: &str = "default";

/// Upper bound on events scanned for a single read. Checkpoint threads are
/// short by construction; an unbounded scan would be a DoS amplifier.
const MAX_SCAN: usize = 1_000;

/// Tenant + thread scope for a checkpoint operation.
///
/// `tenant_id` is mandatory and validated — see [`CheckpointScope::entity_id`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointScope {
    pub tenant_id: String,
    pub checkpoint_ns: String,
    pub thread_id: String,
}

impl CheckpointScope {
    /// Build a scope, defaulting the namespace. Does not validate — validation
    /// happens in [`Self::entity_id`] so every read/write path shares one check.
    pub fn new(
        tenant_id: impl Into<String>,
        checkpoint_ns: Option<String>,
        thread_id: impl Into<String>,
    ) -> Self {
        let ns = checkpoint_ns
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| DEFAULT_NS.to_string());
        Self {
            tenant_id: tenant_id.into(),
            checkpoint_ns: ns,
            thread_id: thread_id.into(),
        }
    }

    /// The event-log entity id for this scope: `agent-checkpoint:{tenant}:{ns}:{thread}`.
    ///
    /// Fail-closed: empty tenant or thread is an error, never a wildcard. Any
    /// component containing the `:` separator is rejected — otherwise a crafted
    /// `thread_id` could forge another tenant's entity id.
    pub fn entity_id(&self) -> Result<String> {
        let tenant = self.tenant_id.trim();
        let thread = self.thread_id.trim();
        if tenant.is_empty() {
            return Err(anyhow!(
                "checkpoint requires a non-empty `tenant_id` (fail-closed isolation)"
            ));
        }
        if thread.is_empty() {
            return Err(anyhow!("checkpoint requires a non-empty `thread_id`"));
        }
        for (label, part) in [
            ("tenant_id", tenant),
            ("checkpoint_ns", self.checkpoint_ns.as_str()),
            ("thread_id", thread),
        ] {
            if part.contains(':') {
                return Err(anyhow!(
                    "`{label}` must not contain ':' (entity-id separator)"
                ));
            }
        }
        Ok(format!(
            "{ENTITY_PREFIX}:{tenant}:{}:{thread}",
            self.checkpoint_ns
        ))
    }
}

/// A checkpoint save request (LangGraph `BaseCheckpointSaver.put` shape).
#[derive(Debug, Clone, PartialEq)]
pub struct CheckpointPut {
    /// Caller-supplied id. `None` mints one (uuid v4).
    pub checkpoint_id: Option<String>,
    pub parent_checkpoint_id: Option<String>,
    pub checkpoint: Value,
    pub metadata: Value,
    pub writes: Vec<Value>,
}

/// A stored checkpoint as returned by get/list.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredCheckpoint {
    pub checkpoint_id: String,
    pub parent_checkpoint_id: Option<String>,
    pub checkpoint: Value,
    pub metadata: Value,
    pub writes: Vec<Value>,
    /// Event-log sequence — the total order within the thread.
    pub sequence: u64,
}

impl StoredCheckpoint {
    /// The LangGraph `config` echo for this checkpoint.
    pub fn config(&self, scope: &CheckpointScope) -> Value {
        json!({
            "configurable": {
                "thread_id": scope.thread_id,
                "checkpoint_ns": scope.checkpoint_ns,
                "checkpoint_id": self.checkpoint_id,
            }
        })
    }
}

/// Event-sourced checkpoint store over [`EventLogEngine`].
pub struct AgentCheckpointStore {
    event_log: Arc<EventLogEngine>,
}

impl AgentCheckpointStore {
    pub fn new(event_log: Arc<EventLogEngine>) -> Self {
        Self { event_log }
    }

    /// Append a checkpoint to the thread's stream. Returns the stored form
    /// (with the minted or echoed `checkpoint_id`).
    pub async fn save(
        &self,
        scope: &CheckpointScope,
        put: CheckpointPut,
    ) -> Result<StoredCheckpoint> {
        // Validate first — an invalid scope must never reach the log.
        let entity_id = scope.entity_id()?;
        let checkpoint_id = put
            .checkpoint_id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Bound before the payload takes ownership — `causation_id` mirrors the
        // parent link so the audit trail carries the same lineage.
        let parent = put.parent_checkpoint_id.clone();
        let event = Event {
            sequence: 0, // assigned by append_event
            entity_id,
            event_type: CHECKPOINT_EVENT_TYPE.to_string(),
            data: json!({
                "checkpoint_id": checkpoint_id,
                "parent_checkpoint_id": put.parent_checkpoint_id,
                "checkpoint": put.checkpoint,
                "metadata": put.metadata,
                "writes": put.writes,
            }),
            timestamp: chrono::Utc::now(),
            causation_id: parent,
            metadata: std::collections::HashMap::new(),
        };

        let appended = self
            .event_log
            .append_event(event)
            .await
            .map_err(|e| anyhow!("checkpoint save failed: {e}"))?;

        decode(&appended).ok_or_else(|| anyhow!("checkpoint save produced an undecodable event"))
    }

    /// Fetch a checkpoint. `checkpoint_id = None` returns the **latest** for the
    /// thread; `Some(id)` returns that specific one. `Ok(None)` = absent.
    pub async fn get(
        &self,
        scope: &CheckpointScope,
        checkpoint_id: Option<&str>,
    ) -> Result<Option<StoredCheckpoint>> {
        let found = self.read(scope, MAX_SCAN).await?;
        Ok(match checkpoint_id {
            // Ascending order ⇒ the last decoded entry is the newest.
            None => found.into_iter().next_back(),
            Some(id) => found.into_iter().find(|c| c.checkpoint_id == id),
        })
    }

    /// List checkpoints for the thread in ascending sequence order.
    pub async fn list(
        &self,
        scope: &CheckpointScope,
        limit: usize,
    ) -> Result<Vec<StoredCheckpoint>> {
        let mut found = self.read(scope, MAX_SCAN).await?;
        found.truncate(limit);
        Ok(found)
    }

    /// Read + decode the thread's checkpoint stream in ascending sequence order.
    async fn read(&self, scope: &CheckpointScope, scan: usize) -> Result<Vec<StoredCheckpoint>> {
        let entity_id = scope.entity_id()?;
        let events = self
            .event_log
            .read_events(&entity_id, 0, scan)
            .await
            .map_err(|e| anyhow!("checkpoint read failed: {e}"))?;
        let mut found: Vec<StoredCheckpoint> = events.iter().filter_map(decode).collect();
        found.sort_by_key(|c| c.sequence);
        Ok(found)
    }
}

/// Decode a `CheckpointSaved` event back into a [`StoredCheckpoint`].
/// Non-checkpoint events and malformed payloads yield `None` (skipped, never
/// surfaced as a checkpoint).
fn decode(event: &Event) -> Option<StoredCheckpoint> {
    if event.event_type != CHECKPOINT_EVENT_TYPE {
        return None;
    }
    let checkpoint_id = event.data.get("checkpoint_id")?.as_str()?.to_string();
    Some(StoredCheckpoint {
        checkpoint_id,
        parent_checkpoint_id: event
            .data
            .get("parent_checkpoint_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        checkpoint: event.data.get("checkpoint").cloned().unwrap_or(Value::Null),
        metadata: event.data.get("metadata").cloned().unwrap_or(Value::Null),
        writes: event
            .data
            .get("writes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        sequence: event.sequence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::eventlog::EventLogConfig;
    use crate::storage::persistence::filesystem::UnifiedCachingFilesystem;
    use crate::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};

    /// Unique tempdir per test (mandate #17c — no shared `/tmp` paths).
    /// Leaked so the dir outlives the engine's background handles.
    async fn store() -> AgentCheckpointStore {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().to_string_lossy().to_string();
        std::mem::forget(dir);
        let local = LocalFileSystem::new(LocalConfig::default())
            .await
            .expect("local fs");
        let fs = Arc::new(UnifiedCachingFilesystem::new(
            Arc::new(local),
            "checkpoint_test".to_string(),
            "eventlog".to_string(),
        ));
        let engine = EventLogEngine::new(
            EventLogConfig {
                base_dir: base,
                ..Default::default()
            },
            fs,
        )
        .expect("eventlog engine");
        AgentCheckpointStore::new(Arc::new(engine))
    }

    fn scope(tenant: &str, thread: &str) -> CheckpointScope {
        CheckpointScope::new(tenant, None, thread)
    }

    fn put(state: &str) -> CheckpointPut {
        CheckpointPut {
            checkpoint_id: None,
            parent_checkpoint_id: None,
            checkpoint: json!({ "state": state }),
            metadata: json!({ "step": 1 }),
            writes: vec![],
        }
    }

    // ---- scope / isolation --------------------------------------------------

    #[test]
    fn entity_id_is_tenant_scoped() {
        let id = scope("acme", "t1").entity_id().expect("entity id");
        assert_eq!(id, "agent-checkpoint:acme:default:t1");
    }

    #[test]
    fn empty_tenant_is_rejected_fail_closed() {
        let err = scope("", "t1").entity_id().expect_err("must reject");
        assert!(err.to_string().contains("tenant_id"), "got: {err}");
        // whitespace-only is equally empty
        assert!(scope("   ", "t1").entity_id().is_err());
    }

    #[test]
    fn empty_thread_is_rejected() {
        assert!(scope("acme", "").entity_id().is_err());
    }

    /// A `thread_id` carrying the separator could otherwise forge another
    /// tenant's entity id — the core isolation property of the id scheme.
    #[test]
    fn separator_injection_is_rejected() {
        let sneaky = CheckpointScope::new("acme", None, "x:victim:default:t1");
        assert!(sneaky.entity_id().is_err(), "':' must be rejected");
        assert!(CheckpointScope::new("a:b", None, "t1").entity_id().is_err());
        assert!(
            CheckpointScope::new("acme", Some("n:s".into()), "t1")
                .entity_id()
                .is_err()
        );
    }

    #[test]
    fn namespace_defaults_when_absent_or_blank() {
        assert_eq!(
            CheckpointScope::new("a", None, "t").checkpoint_ns,
            "default"
        );
        assert_eq!(
            CheckpointScope::new("a", Some("  ".into()), "t").checkpoint_ns,
            "default"
        );
        assert_eq!(
            CheckpointScope::new("a", Some("ns1".into()), "t").checkpoint_ns,
            "ns1"
        );
    }

    // ---- save / get round-trip ---------------------------------------------

    #[tokio::test]
    async fn save_then_get_latest_round_trips() {
        let s = store().await;
        let sc = scope("acme", "t1");
        let saved = s.save(&sc, put("a")).await.expect("save");
        assert!(!saved.checkpoint_id.is_empty(), "id must be minted");

        let got = s.get(&sc, None).await.expect("get").expect("present");
        assert_eq!(got.checkpoint_id, saved.checkpoint_id);
        assert_eq!(got.checkpoint, json!({ "state": "a" }));
        assert_eq!(got.metadata, json!({ "step": 1 }));
    }

    #[tokio::test]
    async fn get_latest_returns_most_recent_not_first() {
        let s = store().await;
        let sc = scope("acme", "t1");
        let first = s.save(&sc, put("a")).await.expect("save a");
        let second = s.save(&sc, put("b")).await.expect("save b");
        assert_ne!(first.checkpoint_id, second.checkpoint_id, "ids unique");

        let got = s.get(&sc, None).await.expect("get").expect("present");
        assert_eq!(got.checkpoint_id, second.checkpoint_id);
        assert_eq!(got.checkpoint, json!({ "state": "b" }));
    }

    #[tokio::test]
    async fn get_by_explicit_id_selects_that_checkpoint() {
        let s = store().await;
        let sc = scope("acme", "t1");
        let first = s.save(&sc, put("a")).await.expect("save a");
        let _ = s.save(&sc, put("b")).await.expect("save b");

        let got = s
            .get(&sc, Some(&first.checkpoint_id))
            .await
            .expect("get")
            .expect("present");
        assert_eq!(got.checkpoint, json!({ "state": "a" }));
    }

    #[tokio::test]
    async fn caller_supplied_id_is_honoured() {
        let s = store().await;
        let sc = scope("acme", "t1");
        let saved = s
            .save(
                &sc,
                CheckpointPut {
                    checkpoint_id: Some("cp-explicit".into()),
                    ..put("a")
                },
            )
            .await
            .expect("save");
        assert_eq!(saved.checkpoint_id, "cp-explicit");
        assert!(
            s.get(&sc, Some("cp-explicit"))
                .await
                .expect("get")
                .is_some()
        );
    }

    #[tokio::test]
    async fn missing_checkpoint_is_none_not_error() {
        let s = store().await;
        let sc = scope("acme", "t1");
        assert!(s.get(&sc, None).await.expect("empty thread").is_none());
        s.save(&sc, put("a")).await.expect("save");
        assert!(s.get(&sc, Some("nope")).await.expect("absent id").is_none());
    }

    #[tokio::test]
    async fn parent_lineage_is_preserved() {
        let s = store().await;
        let sc = scope("acme", "t1");
        let root = s.save(&sc, put("a")).await.expect("root");
        let child = s
            .save(
                &sc,
                CheckpointPut {
                    parent_checkpoint_id: Some(root.checkpoint_id.clone()),
                    ..put("b")
                },
            )
            .await
            .expect("child");
        assert_eq!(
            child.parent_checkpoint_id.as_deref(),
            Some(root.checkpoint_id.as_str())
        );

        let got = s.get(&sc, None).await.expect("get").expect("present");
        assert_eq!(got.parent_checkpoint_id, Some(root.checkpoint_id));
    }

    #[tokio::test]
    async fn writes_survive_the_round_trip() {
        let s = store().await;
        let sc = scope("acme", "t1");
        s.save(
            &sc,
            CheckpointPut {
                writes: vec![json!({ "ch": "x", "v": 1 }), json!({ "ch": "y", "v": 2 })],
                ..put("a")
            },
        )
        .await
        .expect("save");
        let got = s.get(&sc, None).await.expect("get").expect("present");
        assert_eq!(got.writes.len(), 2);
        assert_eq!(got.writes[1], json!({ "ch": "y", "v": 2 }));
    }

    // ---- cross-tenant / cross-thread isolation ------------------------------

    /// The load-bearing isolation test: same thread id, different tenants must
    /// never see each other's checkpoints.
    #[tokio::test]
    async fn tenants_are_isolated_on_the_same_thread_id() {
        let s = store().await;
        let a = scope("tenant-a", "shared-thread");
        let b = scope("tenant-b", "shared-thread");
        s.save(&a, put("secret-a")).await.expect("save a");

        assert!(
            s.get(&b, None).await.expect("get b").is_none(),
            "tenant-b must not see tenant-a's checkpoint"
        );
        s.save(&b, put("secret-b")).await.expect("save b");
        let ga = s.get(&a, None).await.expect("get a").expect("present");
        let gb = s.get(&b, None).await.expect("get b").expect("present");
        assert_eq!(ga.checkpoint, json!({ "state": "secret-a" }));
        assert_eq!(gb.checkpoint, json!({ "state": "secret-b" }));
    }

    #[tokio::test]
    async fn namespaces_are_isolated() {
        let s = store().await;
        let ns1 = CheckpointScope::new("acme", Some("ns1".into()), "t1");
        let ns2 = CheckpointScope::new("acme", Some("ns2".into()), "t1");
        s.save(&ns1, put("a")).await.expect("save ns1");
        assert!(s.get(&ns2, None).await.expect("get ns2").is_none());
    }

    #[tokio::test]
    async fn threads_are_isolated() {
        let s = store().await;
        s.save(&scope("acme", "t1"), put("a")).await.expect("save");
        assert!(
            s.get(&scope("acme", "t2"), None)
                .await
                .expect("get t2")
                .is_none()
        );
    }

    #[tokio::test]
    async fn save_rejects_empty_tenant_before_touching_the_log() {
        let s = store().await;
        assert!(s.save(&scope("", "t1"), put("a")).await.is_err());
        assert!(s.get(&scope("", "t1"), None).await.is_err());
        assert!(s.list(&scope("", "t1"), 10).await.is_err());
    }

    // ---- list ---------------------------------------------------------------

    #[tokio::test]
    async fn list_returns_ascending_sequence_and_respects_limit() {
        let s = store().await;
        let sc = scope("acme", "t1");
        for state in ["a", "b", "c"] {
            s.save(&sc, put(state)).await.expect("save");
        }
        let all = s.list(&sc, 10).await.expect("list");
        assert_eq!(all.len(), 3);
        assert!(
            all[0].sequence < all[1].sequence && all[1].sequence < all[2].sequence,
            "ascending sequence"
        );
        assert_eq!(all[0].checkpoint, json!({ "state": "a" }));

        let capped = s.list(&sc, 2).await.expect("list capped");
        assert_eq!(capped.len(), 2, "limit must be honoured");
    }

    #[tokio::test]
    async fn list_on_empty_thread_is_empty_not_error() {
        let s = store().await;
        assert!(
            s.list(&scope("acme", "nothing"), 10)
                .await
                .expect("list")
                .is_empty()
        );
    }

    // ---- config echo --------------------------------------------------------

    #[tokio::test]
    async fn config_echoes_the_langgraph_shape() {
        let s = store().await;
        let sc = CheckpointScope::new("acme", Some("ns1".into()), "t1");
        let saved = s.save(&sc, put("a")).await.expect("save");
        let cfg = saved.config(&sc);
        assert_eq!(cfg["configurable"]["thread_id"], json!("t1"));
        assert_eq!(cfg["configurable"]["checkpoint_ns"], json!("ns1"));
        assert_eq!(
            cfg["configurable"]["checkpoint_id"],
            json!(saved.checkpoint_id)
        );
    }
}

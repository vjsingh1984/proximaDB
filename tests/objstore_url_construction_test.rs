// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-OBJSTORE-1 (#960): object-store durability — URL construction guards.
//!
//! The bug class: `metadata_url.replace("file://", "")` followed by
//! re-prepending a scheme (`format!("file://{}…")` / `PathBuf::from`)
//! produced invalid `file://adls://…` URLs, and several subsystems drove
//! `std::fs`/`tokio::fs` at object-store URLs. These tests pin the fixed
//! behavior:
//!
//! 1. The unified WAL writer/reader normalize their base to ONE
//!    scheme-qualified URL and never double-prefix — proven by a full
//!    write→flush→recover round-trip over a scheme-qualified base (the
//!    exact code path an `adls://`/`s3://` base takes, minus the network).
//! 2. The eventlog engine accepts a scheme-qualified base without touching
//!    the local filesystem for directory creation.
//! 3. Audit storage dispatches by scheme and fail-fasts on mismatches.
//! 4. A source-level guard: the `.replace("file://"` strip-and-re-prepend
//!    pattern must never reappear in `src/`.

use proximadb::storage::persistence::write_ahead_log::wal_operations::{
    ObservabilityOperation, UnifiedWALOperation, UnifiedWALReader, UnifiedWALWriter,
};

fn obs_op(name: &str) -> UnifiedWALOperation {
    UnifiedWALOperation::ObservabilityOp(ObservabilityOperation::CreateNamespace {
        namespace: name.to_string(),
        config_json: "{}".to_string(),
    })
}

/// A scheme-qualified WAL base must round-trip write→flush→recover without
/// constructing any `file://file://…` (double-scheme) segment URL. This is
/// the same normalized-URL code path an object-store base exercises.
#[tokio::test]
async fn wal_round_trips_over_scheme_qualified_base() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = format!("file://{}", dir.path().display());

    let mut writer = UnifiedWALWriter::new(base.clone()).await.expect("writer");
    writer.append(obs_op("ns-a")).await.expect("append a");
    writer.append(obs_op("ns-b")).await.expect("append b");
    writer.flush().await.expect("flush");

    // Segments must land under the LOCAL directory (no literal `file:` dir
    // beside it — the signature of a double-prefixed URL).
    let seg = dir.path().join("wal_00000000.log");
    assert!(
        seg.exists(),
        "expected WAL segment at {} — segment URL was mis-joined",
        seg.display()
    );
    assert!(
        !dir.path().join("file:").exists(),
        "double-scheme URL constructed: found literal `file:` directory"
    );

    let reader = UnifiedWALReader::new(base).await.expect("reader");
    let entries = reader.read_all().await.expect("read_all");
    assert_eq!(entries.len(), 2, "both WAL entries must recover");
}

/// A BARE local path base must keep working exactly as before (the writer
/// normalizes it to `file://…` internally).
#[tokio::test]
async fn wal_round_trips_over_bare_local_base() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().display().to_string();

    let mut writer = UnifiedWALWriter::new(base.clone()).await.expect("writer");
    writer.append(obs_op("ns-bare")).await.expect("append");
    writer.flush().await.expect("flush");

    let reader = UnifiedWALReader::new(base).await.expect("reader");
    let entries = reader.read_all().await.expect("read_all");
    assert_eq!(entries.len(), 1);
}

/// The eventlog engine (audit trail) must accept a scheme-qualified base and
/// route persistence through its injected filesystem — full append→read over
/// a `file://` URL base, with no stray literal scheme directories.
#[tokio::test]
async fn eventlog_engine_over_scheme_qualified_base() {
    use proximadb::storage::engines::eventlog::{Event, EventLogConfig, EventLogEngine};
    use std::sync::Arc;

    let dir = tempfile::tempdir().expect("tempdir");
    let base = format!("file://{}", dir.path().display());

    let local = proximadb::storage::persistence::filesystem::local::LocalFileSystem::new(
        proximadb::storage::persistence::filesystem::local::LocalConfig::default(),
    )
    .await
    .expect("local fs");
    let fs = Arc::new(
        proximadb::storage::persistence::filesystem::UnifiedCachingFilesystem::new(
            Arc::new(local),
            "objstore_url_test".to_string(),
            "eventlog".to_string(),
        ),
    );

    let config = EventLogConfig {
        base_dir: base,
        ..Default::default()
    };
    let engine = EventLogEngine::new(config, fs).expect("engine over URL base");

    let event = Event {
        sequence: 0,
        entity_id: "tenant:acme".to_string(),
        event_type: "CollectionCreated".to_string(),
        data: serde_json::json!({"name": "docs"}),
        timestamp: chrono::Utc::now(),
        causation_id: None,
        metadata: std::collections::HashMap::new(),
    };
    let appended = engine.append_event(event).await.expect("append");

    let events = engine
        .read_events(&"tenant:acme".to_string(), 0, 10)
        .await
        .expect("read");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].sequence, appended.sequence);
    assert!(
        !dir.path().join("file:").exists(),
        "eventlog constructed a double-scheme path"
    );
}

/// Audit storage dispatch: the local backend rejects object-store URLs
/// fail-fast; the object-store backend rejects bare paths fail-fast; and the
/// object-store backend round-trips over a factory-routed `file://` URL.
#[tokio::test]
async fn audit_storage_scheme_dispatch() {
    use proximadb::audit::storage::{FileAuditStorage, ObjectStoreAuditStorage};

    let err = FileAuditStorage::new("adls://container/data/audit".to_string())
        .await
        .err()
        .expect("FileAuditStorage must reject object-store URLs");
    assert!(err.to_string().contains("ObjectStoreAuditStorage"));

    let err = ObjectStoreAuditStorage::new("/tmp/not-a-url".to_string())
        .await
        .err()
        .expect("ObjectStoreAuditStorage must reject bare paths");
    assert!(err.to_string().contains("scheme-qualified"));

    // Factory-routed round-trip (file:// exercises the same
    // get_filesystem → write/list/read path as adls:///s3://).
    use proximadb_security::AuditStorage as _;
    use proximadb_security::{AuditEvent, AuditEventType, AuditResource, AuditResult};

    let dir = tempfile::tempdir().expect("tempdir");
    let store = ObjectStoreAuditStorage::new(format!("file://{}", dir.path().display()))
        .await
        .expect("object-store audit storage over file:// URL");

    let event = AuditEvent::new(
        AuditEventType::Authentication,
        AuditResource::new("collection".to_string(), "docs".to_string()),
        "login".to_string(),
        AuditResult::Success,
    );
    store.store_audit_event(&event).await.expect("store");

    let events = store
        .query_events(None, None, None, None, None)
        .await
        .expect("query");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, event.event_id);
}

/// Graph collection metadata follows `metadata_url` through FileSystem rather
/// than falling back to the hard-coded `/tmp/proximadb/metadata` sidecar.
#[tokio::test]
async fn graph_catalog_round_trips_through_scheme_qualified_metadata_url() {
    use proximadb::proto::proximadb_v1::CreateGraphRequest;
    use proximadb::services::GraphCollectionService;
    use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use std::sync::Arc;

    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("file://{}/graph_collections.json", dir.path().display());
    let factory = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("filesystem factory"),
    );
    {
        let service =
            GraphCollectionService::new_with_recovery_at_url(url.clone(), factory.clone())
                .await
                .expect("graph service");
        service
            .create_graph(CreateGraphRequest {
                graph_id: "durable-graph".to_string(),
                name: Some("durable-graph".to_string()),
                ..Default::default()
            })
            .await
            .expect("create graph");
    }
    let reopened = GraphCollectionService::new_with_recovery_at_url(url, factory)
        .await
        .expect("reopen graph service");
    assert!(
        reopened
            .get_graph("durable-graph")
            .await
            .expect("get graph")
            .is_some()
    );
}

/// Discovery registry object-store mode uses one ordered filesystem writer and
/// can restore its job snapshot through a scheme-qualified URL.
#[tokio::test]
async fn discovery_registry_round_trips_through_scheme_qualified_metadata_url() {
    use proximadb::services::discovery::{DiscoveryJob, DiscoveryJobKind, DiscoveryRegistry};
    use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use std::sync::Arc;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("discovery")).expect("discovery dir");
    let url = format!("file://{}/discovery/registry.json", dir.path().display());
    let factory = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .expect("filesystem factory"),
    );
    let job_id = {
        let registry = DiscoveryRegistry::load_or_create_at_url(url.clone(), factory.clone())
            .await
            .expect("discovery registry");
        let job = registry.schedule(DiscoveryJob::new(
            "durable-collection",
            DiscoveryJobKind::Recluster,
        ));
        let fs = factory.get_filesystem(&url).expect("filesystem");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !fs.exists(&url).await.expect("exists") {
            assert!(
                std::time::Instant::now() < deadline,
                "ordered discovery writer did not persist in time"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        job.job_id
    };
    let reopened = DiscoveryRegistry::load_or_create_at_url(url, factory)
        .await
        .expect("reopen discovery registry");
    assert!(reopened.get(&job_id).is_some());
}

/// Source guard: the strip-and-re-prepend pattern (`.replace("file://"`)
/// that produced `file://adls://…` URLs must not reappear anywhere in `src/`.
#[test]
fn no_strip_and_reprepend_pattern_in_src() {
    fn scan(dir: &std::path::Path, offenders: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan(&path, offenders);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && let Ok(content) = std::fs::read_to_string(&path)
                && content.contains(".replace(\"file://\"")
            {
                offenders.push(path.display().to_string());
            }
        }
    }

    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    scan(&src, &mut offenders);
    assert!(
        offenders.is_empty(),
        "`.replace(\"file://\"` (strip-and-re-prepend, TD-OBJSTORE-1 #960) found in: {:?} — \
         use a scheme-preserving join (see shared_services::join_storage_url) instead",
        offenders
    );
}

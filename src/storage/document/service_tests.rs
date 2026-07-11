use super::*;
// The convergence tests call `RecordRoutePort` methods (collection_exists / ensure_collection)
// directly on the mock, so the trait must be in scope (the impls use the fully-qualified path).
use crate::proto::proximadb_v1::{
    DocFilterCondition, DocFilterOperator, DocIndexType, DocumentCollectionConfig, DocumentFilter,
    DocumentUpdate, IndexDefinition, SqlObject, SqlValue, UpdateOperation, sql_value,
};
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageFormatStrategy,
    UnifiedStorageFormat,
};
use async_trait::async_trait;
use proximadb_runtime::RecordRoutePort;
use std::collections::HashMap;
use std::sync::Arc;

// =========================================================================
// Mock storage engine for document service tests
// =========================================================================

struct MockStorageEngine;

#[async_trait]
impl UnifiedStorageFormat for MockStorageEngine {
    fn engine_name(&self) -> &'static str {
        "MockEngine"
    }

    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> StorageFormatStrategy {
        StorageFormatStrategy::Sst
    }

    async fn do_flush(&self, _params: &FlushParameters) -> Result<FlushResult> {
        Ok(FlushResult {
            success: true,
            collections_affected: Vec::new(),
            entries_flushed: Some(0),
            bytes_written: Some(0),
            files_created: Some(0),
            file_paths: Vec::new(),
            duration_ms: Some(0),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
            compaction_error: None,
            flushed_batch_ids: Vec::new(),
        })
    }

    async fn do_compact(&self, _params: &CompactionParameters) -> Result<CompactionResult> {
        Ok(CompactionResult {
            success: true,
            collections_affected: Vec::new(),
            entries_processed: Some(0),
            entries_removed: Some(0),
            bytes_read: Some(0),
            bytes_written: Some(0),
            input_files: Some(0),
            output_files: Some(0),
            duration_ms: Some(0),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        Ok(HashMap::new())
    }

    async fn vector_by_id(
        &self,
        _collection_id: &str,
        _base_path: &str,
        _vector_id: &str,
    ) -> Result<Option<proximadb_records::ProximaRecord>> {
        Ok(None)
    }

    async fn search_vectors_unified(
        &self,
        _ctx: &crate::storage::traits::StorageQueryContext,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        Ok(Vec::new())
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        unimplemented!("MockEngine does not provide a filesystem factory")
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Create a DocumentService backed by the mock storage engine (no WAL)
fn create_test_service() -> DocumentService {
    let engine: Arc<dyn UnifiedStorageFormat> = Arc::new(MockStorageEngine);
    DocumentService::new(engine)
}

/// Build an SqlObject from key-value pairs (string values)
fn make_document(fields: Vec<(&str, SqlValue)>) -> SqlObject {
    SqlObject {
        fields: fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    }
}

/// Convenience: create a string SqlValue
fn sql_string(s: &str) -> SqlValue {
    SqlValue {
        value: Some(sql_value::Value::StringValue(s.to_string())),
    }
}

/// Convenience: create an i64 SqlValue
fn sql_int(n: i64) -> SqlValue {
    SqlValue {
        value: Some(sql_value::Value::Int64Value(n)),
    }
}

/// Convenience: create a numeric (f64) SqlValue
#[allow(dead_code)]
fn sql_number(n: f64) -> SqlValue {
    SqlValue {
        value: Some(sql_value::Value::NumberValue(n)),
    }
}

/// Create a default collection config for testing
fn test_collection_config() -> DocumentCollectionConfig {
    DocumentCollectionConfig {
        name: "test_collection".to_string(),
        ..Default::default()
    }
}

/// Set up a service with a pre-created collection, ready for document operations
async fn service_with_collection(collection_name: &str) -> DocumentService {
    let svc = create_test_service();
    svc.create_collection(
        collection_name,
        DocumentCollectionConfig {
            name: collection_name.to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("collection creation should succeed");
    svc
}

// =========================================================================
// ADR-009 canonical-vector route (document store convergence)
// =========================================================================

/// In-memory stand-in for the shared record/vector store: proves a document routed onto
/// the canonical-vector path lands in — and is served from — the SAME store the REST v2
/// record surface uses (cross-surface visibility), without a full network server. Keyed
/// by (clean collection, raw-id oid).
#[derive(Default)]
struct MockRecordRoute {
    store: std::sync::Mutex<HashMap<String, HashMap<String, proximadb_records::ProximaRecord>>>,
    /// Collections the mock should report as NON-canonical (so `canonical_route` sends them
    /// to the legacy path). Empty ⇒ every collection is treated as an existing canonical
    /// collection — the common case for the convergence tests.
    non_canonical: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Per-collection promote keys captured from `ensure_collection` (P-Shred follow-up):
    /// lets a test assert the document facade forwarded the declared index fields.
    promoted: std::sync::Mutex<HashMap<String, Vec<String>>>,
}

#[async_trait::async_trait]
impl proximadb_runtime::RecordRoutePort for MockRecordRoute {
    async fn insert_records(
        &self,
        collection_id: &str,
        records: Vec<proximadb_records::ProximaRecord>,
        _tenant: Option<&str>,
    ) -> anyhow::Result<usize> {
        let mut store = self.store.lock().expect("mock route lock");
        let coll = store.entry(collection_id.to_string()).or_default();
        let n = records.len();
        for r in records {
            coll.insert(r.oid.clone(), r);
        }
        Ok(n)
    }

    async fn get_record(
        &self,
        collection_id: &str,
        record_id: &str,
        _tenant: Option<&str>,
    ) -> anyhow::Result<Option<proximadb_records::ProximaRecord>> {
        let store = self.store.lock().expect("mock route lock");
        Ok(store
            .get(collection_id)
            .and_then(|c| c.get(record_id).cloned()))
    }

    async fn scan_records(
        &self,
        collection_id: &str,
        limit: usize,
        _tenant: Option<&str>,
    ) -> anyhow::Result<Vec<proximadb_records::ProximaRecord>> {
        let store = self.store.lock().expect("mock route lock");
        Ok(store
            .get(collection_id)
            .map(|c| c.values().take(limit).cloned().collect())
            .unwrap_or_default())
    }

    async fn delete_records(
        &self,
        collection_id: &str,
        record_ids: Vec<String>,
        _tenant: Option<&str>,
    ) -> anyhow::Result<usize> {
        let mut store = self.store.lock().expect("mock route lock");
        let mut n = 0;
        if let Some(coll) = store.get_mut(collection_id) {
            for id in record_ids {
                if coll.remove(&id).is_some() {
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    async fn collection_exists(&self, collection_id: &str, _tenant: Option<&str>) -> bool {
        // Every collection is canonical unless a test explicitly marks it non-canonical.
        !self
            .non_canonical
            .lock()
            .expect("mock route lock")
            .contains(collection_id)
    }

    async fn ensure_collection(
        &self,
        collection_id: &str,
        _dimension: u32,
        _tenant: Option<&str>,
        promote_keys: &[String],
    ) -> anyhow::Result<()> {
        // Provisioning makes a (possibly previously non-canonical) collection canonical —
        // idempotent (removing an absent entry is a no-op). Record the seeded promote keys so
        // tests can assert the document facade forwarded the declared index fields.
        self.non_canonical
            .lock()
            .expect("mock route lock")
            .remove(collection_id);
        self.promoted
            .lock()
            .expect("mock route lock")
            .insert(collection_id.to_string(), promote_keys.to_vec());
        Ok(())
    }
}

/// Scope the process-global `PROXIMADB_DOC_CANONICAL_VECTOR` gate to one collection for
/// the duration of a test; removes it on drop. A unique collection name keeps the gate
/// scoped even if the env leaks across a shared-process `cargo test` (nextest — the
/// mandated runner — isolates per process, so the set/remove is single-threaded there).
struct GateGuard;
impl GateGuard {
    fn on(collection: &str) -> Self {
        // SAFETY (edition 2024): under nextest each test owns its process, so this env
        // mutation is single-threaded; the value is a fixed collection name (allowlist mode).
        unsafe { std::env::set_var("PROXIMADB_DOC_CANONICAL_VECTOR", collection) };
        Self
    }
    /// Force the global kill-switch OFF (the gate is DEFAULT-ON, so a test that wants the
    /// legacy path must explicitly force OFF).
    fn off() -> Self {
        unsafe { std::env::set_var("PROXIMADB_DOC_CANONICAL_VECTOR", "off") };
        Self
    }
}
impl Drop for GateGuard {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("PROXIMADB_DOC_CANONICAL_VECTOR") };
    }
}

fn doc_record(id: &str, collection: &str, title: &str) -> DocumentRecord {
    DocumentRecord::from_tree(
        id.to_string(),
        crate::storage::document::canonical_adapter::sql_object_to_proxima_tree(&make_document(
            vec![("title", sql_string(title))],
        )),
        collection.to_string(),
        None,
        None,
    )
}

#[tokio::test]
async fn canonical_route_insert_is_visible_via_shared_store_and_not_legacy_map() {
    let _gate = GateGuard::on("conv_docs");
    let svc = service_with_collection("conv_docs").await;
    let route = Arc::new(MockRecordRoute::default());
    svc.set_record_route(route.clone());

    // Insert via the DocumentService (the gRPC surface's entry point).
    svc.insert_document_record("conv_docs", doc_record("d1", "conv_docs", "Alpha"))
        .await
        .expect("canonical insert");

    // It landed in the SHARED store with a raw-id OID + document label — NOT the legacy map.
    {
        let store = route.store.lock().expect("lock");
        let coll = store
            .get("conv_docs")
            .expect("collection present in shared store");
        let stored = coll.get("d1").expect("raw-id OID key");
        assert_eq!(
            stored.oid, "d1",
            "OID is the raw doc id, no document/ prefix"
        );
        assert!(
            stored
                .labels
                .contains(proximadb_document::DOCUMENT_RECORD_LABEL),
            "record carries the document facade label"
        );
    }
    assert!(
        !svc.documents
            .get("conv_docs")
            .is_some_and(|d| d.contains_key("d1")),
        "canonical write must NOT populate the legacy in-memory map"
    );

    // Read-back via the DocumentService reads THROUGH the shared store (cross-surface).
    let got = svc
        .get_document("conv_docs", "d1", None)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got.id, "d1");
    assert_eq!(
        got.props.get("title"),
        Some(&ProximaTreeNode::Value(ProximaValue::String(
            "Alpha".to_string()
        )))
    );

    // Query sees it too, sourced from the shared store.
    let queried = svc
        .query_documents("conv_docs", DocumentQueryParams::default())
        .await
        .expect("query");
    assert_eq!(queried.documents.len(), 1);

    // Delete tombstones in the shared store.
    assert!(
        svc.delete_document("conv_docs", "d1")
            .await
            .expect("delete")
    );
    assert!(
        svc.get_document("conv_docs", "d1", None)
            .await
            .expect("get2")
            .is_none()
    );
}

#[tokio::test]
async fn kill_switch_forces_legacy_path_and_ignores_route() {
    // The gate is DEFAULT-ON, so force the global kill-switch OFF to exercise the legacy path.
    let _gate = GateGuard::off();
    let svc = service_with_collection("legacy_docs").await;
    let route = Arc::new(MockRecordRoute::default());
    svc.set_record_route(route.clone());

    svc.insert_document_record("legacy_docs", doc_record("d1", "legacy_docs", "Beta"))
        .await
        .expect("legacy insert");

    // The shared store was NOT touched; the legacy map serves the doc.
    assert!(
        route.store.lock().expect("lock").is_empty(),
        "kill-switch OFF must not route to the shared store"
    );
    let got = svc
        .get_document("legacy_docs", "d1", None)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got.id, "d1");
}

#[tokio::test]
async fn default_on_but_non_canonical_collection_stays_legacy() {
    // Gate DEFAULT-ON (no guard), route wired — but the collection is NOT a canonical vector
    // collection, so the mixed-write-safe capability check must keep it on the legacy path
    // (a pure-document collection must not hard-fail the canonical write under default-ON).
    let svc = service_with_collection("plain_docs").await;
    let route = Arc::new(MockRecordRoute::default());
    route
        .non_canonical
        .lock()
        .expect("lock")
        .insert("plain_docs".to_string());
    svc.set_record_route(route.clone());

    svc.insert_document_record("plain_docs", doc_record("d1", "plain_docs", "Delta"))
        .await
        .expect("legacy insert for non-canonical collection");

    assert!(
        route.store.lock().expect("lock").is_empty(),
        "a non-canonical collection must stay on the legacy path even with the gate ON"
    );
    let got = svc
        .get_document("plain_docs", "d1", None)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got.id, "d1");
}

// =========================================================================
// P-Provision (ADR-055): document-collection create provisions the canonical collection
// =========================================================================

#[tokio::test]
async fn create_collection_provisions_canonical_collection() {
    // With a route wired, creating a document collection provisions the canonical (record/
    // vector) collection, so a pure-document collection converges on the shared store.
    let svc = create_test_service();
    let route = Arc::new(MockRecordRoute::default());
    // Start NON-canonical (legacy) so provisioning is observable as a flip.
    route
        .non_canonical
        .lock()
        .expect("lock")
        .insert("provdocs".to_string());
    svc.set_record_route(route.clone());

    assert!(
        !route.collection_exists("provdocs", None).await,
        "precondition: collection starts non-canonical (legacy)"
    );

    svc.create_collection(
        "provdocs",
        DocumentCollectionConfig {
            name: "provdocs".to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("create document collection");

    assert!(
        route.collection_exists("provdocs", None).await,
        "create_collection must provision the canonical collection (P-Provision)"
    );

    // A subsequent document write now routes CANONICAL (shared store), not the legacy map.
    svc.insert_document_record("provdocs", doc_record("d1", "provdocs", "Zeta"))
        .await
        .expect("canonical insert after provisioning");
    assert!(
        route
            .store
            .lock()
            .expect("lock")
            .get("provdocs")
            .is_some_and(|c| c.contains_key("d1")),
        "insert routes canonical after provisioning (lands in the shared store)"
    );
    assert!(
        !svc.documents
            .get("provdocs")
            .is_some_and(|d| d.contains_key("d1")),
        "canonical write must NOT populate the legacy in-memory map"
    );
}

#[tokio::test]
async fn create_collection_forwards_top_level_index_keys_as_promote_keys() {
    // P-Shred follow-up (ADR-055): the document facade extracts the TOP-LEVEL scalar key from
    // each declared index path and forwards it to `ensure_collection` as a promote key (which
    // the catalog then seeds as a props-auto-promotion column). Nested/array paths are skipped.
    let svc = create_test_service();
    let route = Arc::new(MockRecordRoute::default());
    svc.set_record_route(route.clone());

    svc.create_collection(
        "idxdocs",
        DocumentCollectionConfig {
            name: "idxdocs".to_string(),
            indexes: vec![
                IndexDefinition {
                    path: "$.status".to_string(),
                    index_type: DocIndexType::Btree as i32,
                    ..Default::default()
                },
                IndexDefinition {
                    path: "priority".to_string(),
                    index_type: DocIndexType::Btree as i32,
                    ..Default::default()
                },
                IndexDefinition {
                    path: "$.user.email".to_string(), // nested ⇒ skipped for promotion
                    index_type: DocIndexType::Btree as i32,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    )
    .await
    .expect("create document collection");

    let promoted = route.promoted.lock().expect("lock");
    let keys = promoted.get("idxdocs").expect("promote keys captured");
    assert!(
        keys.contains(&"status".to_string()),
        "top-level $.status promoted"
    );
    assert!(keys.contains(&"priority".to_string()), "bare key promoted");
    assert!(
        !keys
            .iter()
            .any(|k| k.contains("email") || k.contains("user")),
        "nested $.user.email is skipped (would shred nothing useful)"
    );
}

#[tokio::test]
async fn ensure_collection_is_idempotent() {
    let route = MockRecordRoute::default();
    route
        .non_canonical
        .lock()
        .expect("lock")
        .insert("c".to_string());
    route
        .ensure_collection("c", 0, None, &[])
        .await
        .expect("first ensure");
    route
        .ensure_collection("c", 0, None, &[])
        .await
        .expect("second ensure is idempotent");
    assert!(route.collection_exists("c", None).await);
}

#[tokio::test]
async fn create_collection_without_route_stays_legacy_no_panic() {
    // No route wired ⇒ create provisions nothing and does not panic (pure legacy path).
    let svc = create_test_service();
    svc.create_collection(
        "legacyonly",
        DocumentCollectionConfig {
            name: "legacyonly".to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("create without route");
    assert!(
        svc.get_collection("legacyonly")
            .await
            .expect("get")
            .is_some(),
        "collection created on the legacy path when no route is wired"
    );
}

/// Set up a canonical-record-backed service with a pre-created collection.
#[cfg(feature = "canonical-document-store")]
async fn canonical_service_with_collection(collection_name: &str) -> DocumentService {
    use crate::storage::engines::cedar::CedarEngine;
    use proximadb_records::RecordStorage;

    let cedar = Arc::new(CedarEngine::new().expect("cedar engine"));
    let storage_engine: Arc<dyn UnifiedStorageFormat> = cedar.clone();
    let record_store: Arc<dyn RecordStorage> = cedar;
    let svc = DocumentService::with_canonical_record_store(storage_engine, record_store);

    svc.create_collection(
        collection_name,
        DocumentCollectionConfig {
            name: collection_name.to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("collection creation should succeed");
    svc
}

#[allow(dead_code)]
fn assert_same_document_shape(left: &DocumentRecord, right: &DocumentRecord) {
    assert_eq!(left.id, right.id);
    assert_eq!(left.collection_id, right.collection_id);
    assert_eq!(left.version, right.version);
    assert_eq!(left.schema_id, right.schema_id);
    assert_eq!(left.document_type, right.document_type);
    assert_eq!(left.props, right.props);
}

/// Read a top-level field from a record's canonical props as a legacy
/// `SqlValue` (test convenience after TD-106 Slice 7e removed `document`).
fn field(rec: &DocumentRecord, key: &str) -> SqlValue {
    proxima_tree_to_sql_object(&rec.props)
        .fields
        .get(key)
        .cloned()
        .unwrap_or_else(|| panic!("{key} field"))
}

// =========================================================================
// Document CRUD lifecycle tests
// =========================================================================

#[tokio::test]
async fn test_insert_and_get_document() {
    let svc = service_with_collection("books").await;

    let doc = make_document(vec![
        ("title", sql_string("Rust Programming")),
        ("year", sql_int(2024)),
    ]);

    let inserted = svc
        .insert_document("books", Some("book-1"), doc)
        .await
        .expect("insert should succeed");

    assert_eq!(inserted.id, "book-1");
    assert_eq!(inserted.version, 1);
    assert_eq!(inserted.collection_id, "books");

    // Retrieve and verify
    let fetched = svc
        .get_document("books", "book-1", None)
        .await
        .expect("get should succeed")
        .expect("document should exist");

    assert_eq!(fetched.id, "book-1");
    assert_eq!(fetched.version, 1);

    // Verify field contents
    let title_val = field(&fetched, "title");
    assert_eq!(
        title_val.value,
        Some(sql_value::Value::StringValue(
            "Rust Programming".to_string()
        ))
    );

    let year_val = field(&fetched, "year");
    assert_eq!(year_val.value, Some(sql_value::Value::Int64Value(2024)));
}

#[tokio::test]
async fn test_update_document() {
    let svc = service_with_collection("users").await;

    let doc = make_document(vec![
        ("name", sql_string("Alice")),
        ("email", sql_string("alice@example.com")),
    ]);
    svc.insert_document("users", Some("user-1"), doc)
        .await
        .expect("insert should succeed");

    // Update the email field
    let updates = vec![DocumentUpdate {
        operation: UpdateOperation::Set as i32,
        path: "email".to_string(),
        value: Some(sql_string("alice@newdomain.com")),
    }];

    let updated = svc
        .update_document("users", "user-1", updates, None)
        .await
        .expect("update should succeed");

    assert_eq!(updated.version, 2, "version should be incremented");

    // Verify the update persisted
    let fetched = svc
        .get_document("users", "user-1", None)
        .await
        .expect("get should succeed")
        .expect("document should exist");

    let email = field(&fetched, "email");
    assert_eq!(
        email.value,
        Some(sql_value::Value::StringValue(
            "alice@newdomain.com".to_string()
        ))
    );

    // Original field should still be present
    let name = field(&fetched, "name");
    assert_eq!(
        name.value,
        Some(sql_value::Value::StringValue("Alice".to_string()))
    );

    // TD-106 Slice 7: the update mutates the canonical props tree directly.
    match fetched.props.get("email") {
        Some(proximadb_records::ProximaTreeNode::Value(
            proximadb_data_model::ProximaValue::String(s),
        )) => assert_eq!(
            s, "alice@newdomain.com",
            "props must carry the updated value"
        ),
        other => panic!("expected updated email in props, got {other:?}"),
    }
}

#[tokio::test]
async fn test_delete_document() {
    let svc = service_with_collection("items").await;

    let doc = make_document(vec![("product", sql_string("Widget"))]);
    svc.insert_document("items", Some("item-1"), doc)
        .await
        .expect("insert should succeed");

    // Confirm it exists
    let before = svc
        .get_document("items", "item-1", None)
        .await
        .expect("get should succeed");
    assert!(before.is_some(), "document should exist before delete");

    // Delete
    let deleted = svc
        .delete_document("items", "item-1")
        .await
        .expect("delete should succeed");
    assert!(deleted, "delete should return true for existing doc");

    // Confirm it is gone
    let after = svc
        .get_document("items", "item-1", None)
        .await
        .expect("get should succeed");
    assert!(after.is_none(), "document should be gone after delete");
}

#[tokio::test]
async fn test_insert_duplicate_id() {
    let svc = service_with_collection("dup").await;

    let doc1 = make_document(vec![("val", sql_string("first"))]);
    svc.insert_document("dup", Some("same-id"), doc1)
        .await
        .expect("first insert should succeed");

    // Inserting with the same ID acts as an upsert in the in-memory store
    // because insert_document unconditionally inserts into the HashMap.
    let doc2 = make_document(vec![("val", sql_string("second"))]);
    svc.insert_document("dup", Some("same-id"), doc2)
        .await
        .expect("second insert (upsert) should succeed");

    let fetched = svc
        .get_document("dup", "same-id", None)
        .await
        .expect("get should succeed")
        .expect("document should exist");

    // The second insert should have overwritten the first
    let val = field(&fetched, "val");
    assert_eq!(
        val.value,
        Some(sql_value::Value::StringValue("second".to_string())),
        "second insert should overwrite the first"
    );
}

#[tokio::test]
async fn test_get_nonexistent_document() {
    let svc = service_with_collection("empty_coll").await;

    let result = svc
        .get_document("empty_coll", "does-not-exist", None)
        .await
        .expect("get should not error");

    assert!(result.is_none(), "nonexistent ID should return None");
}

#[tokio::test]
async fn test_insert_batch_documents() {
    let svc = service_with_collection("batch").await;

    let batch: Vec<(Option<String>, SqlObject)> = (0..5)
        .map(|i| {
            (
                Some(format!("doc-{}", i)),
                make_document(vec![("index", sql_int(i))]),
            )
        })
        .collect();

    let result = svc
        .insert_documents("batch", batch)
        .await
        .expect("batch insert should succeed");

    assert_eq!(result.ingested, 5);
    assert_eq!(result.failed, 0);
    assert!(result.errors.is_empty());

    // Verify each document is retrievable
    for i in 0..5 {
        let doc = svc
            .get_document("batch", &format!("doc-{}", i), None)
            .await
            .expect("get should succeed")
            .expect("document should exist");
        let idx_val = field(&doc, "index");
        assert_eq!(idx_val.value, Some(sql_value::Value::Int64Value(i)));
    }
}

// =========================================================================
// Query tests
// =========================================================================

#[tokio::test]
async fn test_query_with_filter() {
    let svc = service_with_collection("products").await;

    // Insert 3 documents with different categories
    svc.insert_document(
        "products",
        Some("p1"),
        make_document(vec![
            ("name", sql_string("Laptop")),
            ("category", sql_string("electronics")),
        ]),
    )
    .await
    .expect("insert p1");

    svc.insert_document(
        "products",
        Some("p2"),
        make_document(vec![
            ("name", sql_string("Shirt")),
            ("category", sql_string("clothing")),
        ]),
    )
    .await
    .expect("insert p2");

    svc.insert_document(
        "products",
        Some("p3"),
        make_document(vec![
            ("name", sql_string("Phone")),
            ("category", sql_string("electronics")),
        ]),
    )
    .await
    .expect("insert p3");

    // Query with filter: category == "electronics"
    let filter = DocumentFilter {
        conditions: vec![DocFilterCondition {
            path: "category".to_string(),
            operator: DocFilterOperator::Eq as i32,
            value: Some(sql_string("electronics")),
            values: Vec::new(),
        }],
        ..Default::default()
    };

    let result = svc
        .query_documents(
            "products",
            DocumentQueryParams {
                filter: Some(filter),
                limit: 100,
                include_count: true,
                ..Default::default()
            },
        )
        .await
        .expect("query should succeed");

    assert_eq!(
        result.documents.len(),
        2,
        "should return only electronics items"
    );
    assert_eq!(result.total_count, Some(2));

    // Verify all returned docs are in the electronics category
    for doc in &result.documents {
        let cat = field(doc, "category");
        assert_eq!(
            cat.value,
            Some(sql_value::Value::StringValue("electronics".to_string()))
        );
    }
}

#[tokio::test]
async fn test_query_with_pagination() {
    let svc = service_with_collection("paginated").await;

    // Insert 10 documents
    for i in 0..10 {
        svc.insert_document(
            "paginated",
            Some(&format!("item-{:02}", i)),
            make_document(vec![("seq", sql_int(i))]),
        )
        .await
        .expect("insert should succeed");
    }

    // Query with limit=3, offset=2
    let result = svc
        .query_documents(
            "paginated",
            DocumentQueryParams {
                limit: 3,
                offset: 2,
                include_count: true,
                ..Default::default()
            },
        )
        .await
        .expect("query should succeed");

    assert_eq!(
        result.documents.len(),
        3,
        "should return exactly 3 documents"
    );
    assert_eq!(
        result.total_count,
        Some(10),
        "total count should be 10 (before pagination)"
    );
}

#[tokio::test]
async fn test_query_all_documents() {
    let svc = service_with_collection("all_docs").await;

    // Insert 4 documents
    for i in 0..4 {
        svc.insert_document(
            "all_docs",
            Some(&format!("d{}", i)),
            make_document(vec![("n", sql_int(i))]),
        )
        .await
        .expect("insert should succeed");
    }

    // Query with no filter (limit=0 means "all")
    let result = svc
        .query_documents(
            "all_docs",
            DocumentQueryParams {
                include_count: true,
                ..Default::default()
            },
        )
        .await
        .expect("query should succeed");

    assert_eq!(result.documents.len(), 4, "should return all 4 documents");
    assert_eq!(result.total_count, Some(4));
}

#[cfg(feature = "canonical-document-store")]
#[tokio::test]
async fn test_canonical_document_service_parity_with_legacy_path() {
    let legacy = service_with_collection("parity").await;
    let canonical = canonical_service_with_collection("parity").await;

    let doc = make_document(vec![
        ("title", sql_string("Record Spine")),
        ("category", sql_string("architecture")),
        ("revision", sql_int(1)),
    ]);

    let legacy_inserted = legacy
        .insert_document("parity", Some("doc-1"), doc.clone())
        .await
        .expect("legacy insert");
    let canonical_inserted = canonical
        .insert_document("parity", Some("doc-1"), doc)
        .await
        .expect("canonical insert");
    assert_same_document_shape(&legacy_inserted, &canonical_inserted);

    let legacy_fetched = legacy
        .get_document("parity", "doc-1", None)
        .await
        .expect("legacy get")
        .expect("legacy document");
    let canonical_fetched = canonical
        .get_document("parity", "doc-1", None)
        .await
        .expect("canonical get")
        .expect("canonical document");
    assert_same_document_shape(&legacy_fetched, &canonical_fetched);

    let updates = vec![DocumentUpdate {
        operation: UpdateOperation::Set as i32,
        path: "revision".to_string(),
        value: Some(sql_int(2)),
    }];
    let legacy_updated = legacy
        .update_document("parity", "doc-1", updates.clone(), None)
        .await
        .expect("legacy update");
    let canonical_updated = canonical
        .update_document("parity", "doc-1", updates, None)
        .await
        .expect("canonical update");
    assert_same_document_shape(&legacy_updated, &canonical_updated);

    legacy
        .insert_document(
            "parity",
            Some("doc-2"),
            make_document(vec![
                ("title", sql_string("Projection")),
                ("category", sql_string("architecture")),
                ("revision", sql_int(1)),
            ]),
        )
        .await
        .expect("legacy insert second");
    canonical
        .insert_document(
            "parity",
            Some("doc-2"),
            make_document(vec![
                ("title", sql_string("Projection")),
                ("category", sql_string("architecture")),
                ("revision", sql_int(1)),
            ]),
        )
        .await
        .expect("canonical insert second");

    let filter = DocumentFilter {
        conditions: vec![DocFilterCondition {
            path: "category".to_string(),
            operator: DocFilterOperator::Eq as i32,
            value: Some(sql_string("architecture")),
            values: Vec::new(),
        }],
        ..Default::default()
    };
    let query_params = DocumentQueryParams {
        filter: Some(filter),
        include_count: true,
        limit: 100,
        ..Default::default()
    };
    let legacy_query = legacy
        .query_documents("parity", query_params.clone())
        .await
        .expect("legacy query");
    let canonical_query = canonical
        .query_documents("parity", query_params)
        .await
        .expect("canonical query");
    assert_eq!(legacy_query.total_count, canonical_query.total_count);

    let mut legacy_ids: Vec<_> = legacy_query
        .documents
        .iter()
        .map(|document| document.id.as_str())
        .collect();
    let mut canonical_ids: Vec<_> = canonical_query
        .documents
        .iter()
        .map(|document| document.id.as_str())
        .collect();
    legacy_ids.sort_unstable();
    canonical_ids.sort_unstable();
    assert_eq!(legacy_ids, canonical_ids);

    assert!(legacy.delete_document("parity", "doc-1").await.unwrap());
    assert!(canonical.delete_document("parity", "doc-1").await.unwrap());
    assert!(
        legacy
            .get_document("parity", "doc-1", None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        canonical
            .get_document("parity", "doc-1", None)
            .await
            .unwrap()
            .is_none()
    );
}

#[cfg(feature = "canonical-document-store")]
#[tokio::test]
async fn test_canonical_document_query_uses_record_oid_projection_keys() {
    use crate::storage::engines::cedar::CedarEngine;
    use proximadb_records::RecordStorage;

    let cedar = Arc::new(CedarEngine::new().expect("cedar engine"));
    let storage_engine: Arc<dyn UnifiedStorageFormat> = cedar.clone();
    let record_store: Arc<dyn RecordStorage> = cedar;
    let svc = DocumentService::with_canonical_record_store(storage_engine, record_store);

    svc.create_collection(
        "indexed",
        DocumentCollectionConfig {
            name: "indexed".to_string(),
            indexes: vec![IndexDefinition {
                path: "category".to_string(),
                index_type: DocIndexType::Btree as i32,
                unique: false,
                sparse: false,
                name: Some("category_idx".to_string()),
            }],
            ..Default::default()
        },
    )
    .await
    .expect("collection creation should succeed");

    svc.insert_document(
        "indexed",
        Some("doc-1"),
        make_document(vec![
            ("title", sql_string("Canonical Index")),
            ("category", sql_string("architecture")),
        ]),
    )
    .await
    .expect("insert should succeed");

    let query_params = DocumentQueryParams {
        filter: Some(DocumentFilter {
            conditions: vec![DocFilterCondition {
                path: "category".to_string(),
                operator: DocFilterOperator::Eq as i32,
                value: Some(sql_string("architecture")),
                values: Vec::new(),
            }],
            ..Default::default()
        }),
        include_count: true,
        ..Default::default()
    };

    let query_result = svc
        .query_documents("indexed", query_params.clone())
        .await
        .expect("indexed canonical query should succeed");
    assert_eq!(query_result.total_count, Some(1));
    assert_eq!(query_result.documents[0].id, "doc-1");

    assert!(
        svc.delete_document("indexed", "doc-1")
            .await
            .expect("delete should succeed")
    );

    let query_after_delete = svc
        .query_documents("indexed", query_params)
        .await
        .expect("indexed canonical query after delete should succeed");
    assert_eq!(query_after_delete.total_count, Some(0));
    assert!(query_after_delete.documents.is_empty());
}

#[cfg(feature = "canonical-document-store")]
#[tokio::test]
async fn test_canonical_document_wal_recovery_replays_into_record_store() {
    use crate::storage::engines::cedar::CedarEngine;
    use proximadb_records::RecordStorage;

    let temp_dir = tempfile::tempdir().expect("temp wal dir");
    let wal_base_path = temp_dir.path().to_str().expect("utf-8 temp path");
    let collection_id = "wal_docs_upsert";
    let document_id = "doc-upsert";

    let first_cedar = Arc::new(CedarEngine::new().expect("cedar engine"));
    let first_storage_engine: Arc<dyn UnifiedStorageFormat> = first_cedar.clone();
    let first_record_store: Arc<dyn RecordStorage> = first_cedar;
    let first = DocumentService::with_canonical_record_store_and_wal(
        first_storage_engine,
        first_record_store,
        wal_base_path,
    )
    .await
    .expect("canonical wal service");

    first
        .create_collection(
            collection_id,
            DocumentCollectionConfig {
                name: collection_id.to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("create collection");
    first
        .insert_document(
            collection_id,
            Some(document_id),
            make_document(vec![("title", sql_string("Recovered"))]),
        )
        .await
        .expect("insert");
    first.flush_wal().await.expect("flush wal");
    let wal_dir = format!("{}/document_wal", wal_base_path);
    let wal_files = std::fs::read_dir(&wal_dir)
        .expect("list document wal dir")
        .map(|entry| {
            let entry = entry.expect("wal dir entry");
            let len = entry.metadata().expect("wal entry metadata").len();
            format!("{}:{}", entry.file_name().to_string_lossy(), len)
        })
        .collect::<Vec<_>>();
    let durable_entries =
        crate::storage::persistence::write_ahead_log::wal_operations::UnifiedWALReader::new(
            wal_dir,
        )
        .await
        .expect("open wal reader")
        .read_all()
        .await
        .expect("read flushed wal");
    assert!(
        durable_entries.iter().any(|entry| matches!(
            &entry.operation,
            UnifiedWALOperation::DocumentOp(
                DocumentOperation::UpsertCanonicalDocumentRecord {
                    collection_id: wal_collection,
                    ..
                }
            ) if wal_collection == collection_id
        )),
        "flushed WAL should contain canonical document upsert; files {:?}; got {:?}",
        wal_files,
        durable_entries
            .iter()
            .map(|entry| &entry.operation)
            .collect::<Vec<_>>()
    );
    drop(first);

    let restarted_cedar = Arc::new(CedarEngine::new().expect("restarted cedar engine"));
    let restarted_storage_engine: Arc<dyn UnifiedStorageFormat> = restarted_cedar.clone();
    let restarted_record_store: Arc<dyn RecordStorage> = restarted_cedar;
    let restarted_record_probe = restarted_record_store.clone();
    let restarted = DocumentService::with_canonical_record_store_and_wal(
        restarted_storage_engine,
        restarted_record_store,
        wal_base_path,
    )
    .await
    .expect("restart from wal");

    let recovered_scan = restarted_record_probe
        .scan_records(10)
        .await
        .expect("scan recovered records");
    assert_eq!(
        recovered_scan.len(),
        1,
        "canonical WAL recovery should replay one record; got {:?}",
        recovered_scan
            .iter()
            .map(|record| record.oid.as_str())
            .collect::<Vec<_>>()
    );

    let recovered_key = DocumentRecordKey::new(collection_id, document_id);
    let recovered_record = restarted_record_probe
        .get_record(&RecordKey::new(recovered_key.canonical_oid()))
        .await
        .expect("get recovered canonical record")
        .unwrap_or_else(|| {
            panic!(
                "canonical record recovered at {}; scanned {:?}",
                recovered_key.canonical_oid(),
                recovered_scan
                    .iter()
                    .map(|record| record.oid.as_str())
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(recovered_record.oid, recovered_key.canonical_oid());

    let recovered = restarted
        .get_document(collection_id, document_id, None)
        .await
        .expect("get recovered")
        .expect("document recovered through canonical store");
    assert_eq!(
        field(&recovered, "title").value,
        Some(sql_value::Value::StringValue("Recovered".to_string()))
    );
}

#[cfg(feature = "canonical-document-store")]
#[tokio::test]
async fn test_canonical_document_wal_recovery_replays_deletes_into_record_store() {
    use crate::storage::engines::cedar::CedarEngine;
    use proximadb_records::RecordStorage;

    let temp_dir = tempfile::tempdir().expect("temp wal dir");
    let wal_base_path = temp_dir.path().to_str().expect("utf-8 temp path");
    let collection_id = "wal_docs_delete";
    let document_id = "doc-delete";

    let first_cedar = Arc::new(CedarEngine::new().expect("cedar engine"));
    let first_storage_engine: Arc<dyn UnifiedStorageFormat> = first_cedar.clone();
    let first_record_store: Arc<dyn RecordStorage> = first_cedar;
    let first = DocumentService::with_canonical_record_store_and_wal(
        first_storage_engine,
        first_record_store,
        wal_base_path,
    )
    .await
    .expect("canonical wal service");

    first
        .create_collection(
            collection_id,
            DocumentCollectionConfig {
                name: collection_id.to_string(),
                ..Default::default()
            },
        )
        .await
        .expect("create collection");
    first
        .insert_document(
            collection_id,
            Some(document_id),
            make_document(vec![("title", sql_string("Deleted"))]),
        )
        .await
        .expect("insert");
    assert!(
        first
            .delete_document(collection_id, document_id)
            .await
            .expect("delete")
    );
    first.flush_wal().await.expect("flush wal");
    drop(first);

    let restarted_cedar = Arc::new(CedarEngine::new().expect("restarted cedar engine"));
    let restarted_storage_engine: Arc<dyn UnifiedStorageFormat> = restarted_cedar.clone();
    let restarted_record_store: Arc<dyn RecordStorage> = restarted_cedar;
    let restarted_record_probe = restarted_record_store.clone();
    let restarted = DocumentService::with_canonical_record_store_and_wal(
        restarted_storage_engine,
        restarted_record_store,
        wal_base_path,
    )
    .await
    .expect("restart from wal");

    let recovered_records = restarted_record_probe
        .scan_records(10)
        .await
        .expect("scan recovered records");
    assert!(
        recovered_records.is_empty(),
        "delete replay should remove canonical records"
    );

    assert!(
        restarted
            .get_document(collection_id, document_id, None)
            .await
            .expect("get after delete replay")
            .is_none()
    );
}

#[tokio::test]
async fn test_query_empty_collection() {
    let svc = service_with_collection("empty").await;

    let result = svc
        .query_documents(
            "empty",
            DocumentQueryParams {
                include_count: true,
                ..Default::default()
            },
        )
        .await
        .expect("query on empty collection should succeed");

    assert!(result.documents.is_empty(), "should return no documents");
    assert_eq!(result.total_count, Some(0));
}

#[tokio::test]
async fn document_query_service_searches_via_contract() {
    use proximadb_document_query::{
        DocumentQueryService, DocumentSearchRequest, DocumentSortOrder, SortDirection,
    };

    let svc = service_with_collection("contract_docs").await;

    for i in 0..3 {
        svc.insert_document(
            "contract_docs",
            Some(&format!("doc-{i}")),
            make_document(vec![("seq", sql_int(i))]),
        )
        .await
        .expect("insert should succeed");
    }

    let result = DocumentQueryService::document_search(
        &svc,
        DocumentSearchRequest {
            collection_id: "contract_docs".to_string(),
            filter: None,
            limit: 2,
            offset: 1,
            projection: None,
            sort: Some(DocumentSortOrder {
                field: "seq".to_string(),
                direction: SortDirection::Ascending,
            }),
        },
    )
    .await
    .expect("contract search should succeed");

    assert_eq!(result.total_count, 3);
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.results[0].id, "doc-1");
    assert_eq!(result.results[1].id, "doc-2");
}

#[tokio::test]
async fn document_query_service_gets_document_via_contract() {
    use proximadb_document_query::DocumentQueryService;

    let svc = service_with_collection("contract_get").await;
    svc.insert_document(
        "contract_get",
        Some("doc-1"),
        make_document(vec![("title", sql_string("Contract"))]),
    )
    .await
    .expect("insert should succeed");

    let result =
        DocumentQueryService::get_document(&svc, "contract_get".to_string(), "doc-1".to_string())
            .await
            .expect("contract get should succeed")
            .expect("document should exist");

    assert_eq!(result.id, "doc-1");
    assert_eq!(result.version, 1);
}

// =========================================================================
// Collection management tests
// =========================================================================

#[tokio::test]
async fn test_create_and_list_collections() {
    let svc = create_test_service();

    // No collections initially
    let before = svc.list_collections().await.expect("list should succeed");
    assert!(before.is_empty(), "should start with no collections");

    // Create two collections
    svc.create_collection("alpha", test_collection_config())
        .await
        .expect("create alpha should succeed");
    svc.create_collection(
        "beta",
        DocumentCollectionConfig {
            name: "beta".to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("create beta should succeed");

    let after = svc.list_collections().await.expect("list should succeed");
    assert_eq!(after.len(), 2, "should have 2 collections");

    let names: Vec<&str> = after.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));

    // Verify get_collection returns metadata
    let alpha = svc
        .get_collection("alpha")
        .await
        .expect("get should succeed")
        .expect("alpha should exist");
    assert_eq!(alpha.name, "alpha");
    assert_eq!(alpha.document_count, 0);
}

#[tokio::test]
async fn test_delete_collection() {
    let svc = create_test_service();

    svc.create_collection("ephemeral", test_collection_config())
        .await
        .expect("create should succeed");

    // Insert a document so we can verify data is also removed
    svc.insert_document(
        "ephemeral",
        Some("d1"),
        make_document(vec![("x", sql_int(1))]),
    )
    .await
    .expect("insert should succeed");

    // Delete the collection
    let deleted = svc
        .delete_collection("ephemeral")
        .await
        .expect("delete should succeed");
    assert!(deleted, "delete should return true for existing collection");

    // Verify it is gone
    let after = svc
        .get_collection("ephemeral")
        .await
        .expect("get should succeed");
    assert!(after.is_none(), "collection should be gone after delete");

    // Listing should not include it
    let list = svc.list_collections().await.expect("list should succeed");
    assert!(list.is_empty(), "no collections should remain");

    // Deleting again should return false
    let again = svc
        .delete_collection("ephemeral")
        .await
        .expect("delete should succeed");
    assert!(!again, "deleting non-existent collection returns false");
}

#[test]
fn scoped_document_collection_default_bare_named_scoped_invalid_rejected() {
    assert_eq!(
        scoped_document_collection("default", "docs").unwrap(),
        "docs",
        "default tenant stays bare (matches bare-created collections)"
    );
    assert_eq!(
        scoped_document_collection("acme", "docs").unwrap(),
        "acme/docs",
        "named tenant is path-scoped"
    );
    assert!(
        scoped_document_collection("../evil", "docs").is_err(),
        "path traversal rejected"
    );
    assert!(
        scoped_document_collection("_system", "docs").is_err(),
        "reserved-prefix tenant rejected"
    );
}

/// Named-tenant document isolation with NO explicit provisioning (TD-DOC-TENANT-1): the first
/// write to each scoped collection auto-provisions it, and two tenants using the SAME clean
/// logical collection + doc id are isolated by their distinct `{tenant}/{collection}` keys.
#[tokio::test]
async fn scoped_document_collections_isolate_by_tenant_with_auto_provision() {
    let svc = create_test_service();
    let acme = scoped_document_collection("acme", "shared").unwrap();
    let globex = scoped_document_collection("globex", "shared").unwrap();

    // Same clean logical collection ("shared") AND same doc id ("d1"), different tenants.
    svc.insert_document(
        &acme,
        Some("d1"),
        make_document(vec![("owner", sql_string("acme"))]),
    )
    .await
    .expect("acme insert auto-provisions acme/shared");
    svc.insert_document(
        &globex,
        Some("d1"),
        make_document(vec![("owner", sql_string("globex"))]),
    )
    .await
    .expect("globex insert auto-provisions globex/shared");

    let a = svc
        .get_document(&acme, "d1", None)
        .await
        .expect("get acme")
        .expect("acme doc exists");
    let g = svc
        .get_document(&globex, "d1", None)
        .await
        .expect("get globex")
        .expect("globex doc exists");

    // Each tenant's doc lives under its own scoped collection — no cross-tenant bleed despite
    // identical clean collection name + doc id.
    assert_eq!(a.collection_id, "acme/shared");
    assert_eq!(g.collection_id, "globex/shared");
    assert_ne!(
        tree_get(&a.props, "owner"),
        tree_get(&g.props, "owner"),
        "same clean collection + doc id isolate by tenant"
    );
}

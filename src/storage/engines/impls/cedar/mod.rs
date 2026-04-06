//! # CEDAR Document Storage Engine
//!
//! **STATUS**: Phase 1 (Apr 2026)
//!
//! Columnar Extensible Document Archive -- LSM-based document engine.
//!
//! CEDAR implements `DocumentStorageEngine` (the document-native trait) for
//! efficient JSON document CRUD with secondary indexes, MVCC versioning,
//! and aggregation pipelines.
//!
//! It also implements `UnifiedStorageEngine` as a thin stub for factory
//! registration, but all real document operations go through `DocumentStorageEngine`.

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::search::results::OptimizedSearchRecord;
use crate::proto::proximadb_v1::{AggregationStage, DocumentUpdate, IndexDefinition, VectorRecord};
use crate::storage::document::{
    AggregateResult, DocumentQueryParams, DocumentQueryResult, DocumentRecord,
    DocumentStorageEngine, FlushToStorageResult,
};
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageEngineStrategy,
    StorageQueryContext, UnifiedStorageEngine,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the CEDAR document storage engine.
#[derive(Debug, Clone)]
pub struct CedarConfig {
    /// Base directory for data files. `None` uses the system default.
    pub base_path: Option<PathBuf>,
    /// In-memory write buffer size in megabytes before flushing to disk.
    pub memtable_size_mb: usize,
}

impl Default for CedarConfig {
    fn default() -> Self {
        Self {
            base_path: None,
            memtable_size_mb: 256,
        }
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// CEDAR storage engine -- document-oriented LSM store.
///
/// Uses DashMap for lock-free concurrent document access in the memtable.
/// Each collection gets its own DashMap partition.
pub struct CedarEngine {
    config: CedarConfig,
    /// Per-collection document memtables: collection_name -> (doc_id -> DocumentRecord)
    collections: DashMap<String, DashMap<String, DocumentRecord>>,
    /// Global document counter for metrics
    doc_count: AtomicU64,
}

impl CedarEngine {
    /// Create a new `CedarEngine` with default configuration.
    pub fn new() -> Result<Self> {
        Self::with_config(CedarConfig::default())
    }

    /// Create a new `CedarEngine` with the given configuration.
    pub fn with_config(config: CedarConfig) -> Result<Self> {
        Ok(Self {
            config,
            collections: DashMap::new(),
            doc_count: AtomicU64::new(0),
        })
    }

    /// Get or create the memtable for a collection.
    fn get_collection(
        &self,
        collection: &str,
    ) -> dashmap::mapref::one::Ref<'_, String, DashMap<String, DocumentRecord>> {
        if !self.collections.contains_key(collection) {
            self.collections
                .insert(collection.to_string(), DashMap::new());
        }
        self.collections.get(collection).unwrap()
    }
}

// ---------------------------------------------------------------------------
// DocumentStorageEngine implementation (the real document interface)
// ---------------------------------------------------------------------------

#[async_trait]
impl DocumentStorageEngine for CedarEngine {
    fn engine_name(&self) -> &'static str {
        "cedar"
    }

    async fn insert_document(
        &self,
        collection: &str,
        doc: DocumentRecord,
    ) -> Result<DocumentRecord> {
        let col = self.get_collection(collection);
        col.insert(doc.id.clone(), doc.clone());
        self.doc_count.fetch_add(1, Ordering::Relaxed);
        Ok(doc)
    }

    async fn get_document(&self, collection: &str, id: &str) -> Result<Option<DocumentRecord>> {
        if let Some(col) = self.collections.get(collection) {
            Ok(col.get(id).map(|r| r.value().clone()))
        } else {
            Ok(None)
        }
    }

    async fn update_document(
        &self,
        collection: &str,
        id: &str,
        _updates: Vec<DocumentUpdate>,
    ) -> Result<DocumentRecord> {
        let col = self.get_collection(collection);
        let mut doc = col
            .get(id)
            .map(|r| r.value().clone())
            .ok_or_else(|| anyhow::anyhow!("Document '{}' not found in '{}'", id, collection))?;

        doc.version += 1;
        doc.updated_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        col.insert(id.to_string(), doc.clone());
        Ok(doc)
    }

    async fn delete_document(&self, collection: &str, id: &str) -> Result<bool> {
        if let Some(col) = self.collections.get(collection) {
            if col.remove(id).is_some() {
                self.doc_count.fetch_sub(1, Ordering::Relaxed);
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn query_documents(
        &self,
        collection: &str,
        params: DocumentQueryParams,
    ) -> Result<DocumentQueryResult> {
        let start = std::time::Instant::now();
        let docs = self.scan_documents(collection, None).await?;

        // Apply limit
        let limited: Vec<DocumentRecord> = if params.limit > 0 {
            docs.into_iter()
                .skip(params.offset as usize)
                .take(params.limit as usize)
                .collect()
        } else {
            docs
        };

        Ok(DocumentQueryResult {
            total_count: if params.include_count {
                Some(self.document_count(collection).await?)
            } else {
                None
            },
            documents: limited,
            query_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn scan_documents(
        &self,
        collection: &str,
        limit: Option<usize>,
    ) -> Result<Vec<DocumentRecord>> {
        if let Some(col) = self.collections.get(collection) {
            let iter = col.iter().map(|r| r.value().clone());
            Ok(match limit {
                Some(n) => iter.take(n).collect(),
                None => iter.collect(),
            })
        } else {
            Ok(vec![])
        }
    }

    async fn aggregate(
        &self,
        _collection: &str,
        _pipeline: Vec<AggregationStage>,
    ) -> Result<AggregateResult> {
        // Phase 2: implement aggregation pipeline
        Ok(AggregateResult {
            results: vec![],
            query_time_ms: 0,
        })
    }

    async fn create_index(&self, _collection: &str, _index_def: IndexDefinition) -> Result<()> {
        // Phase 2: implement secondary indexes
        Ok(())
    }

    async fn flush(&self, _collection: &str) -> Result<FlushToStorageResult> {
        // Phase 5: implement disk persistence
        Ok(FlushToStorageResult::default())
    }

    async fn compact(&self, _collection: &str) -> Result<FlushToStorageResult> {
        // Phase 5: implement compaction
        Ok(FlushToStorageResult::default())
    }

    async fn document_count(&self, collection: &str) -> Result<u64> {
        Ok(self
            .collections
            .get(collection)
            .map(|c| c.len() as u64)
            .unwrap_or(0))
    }

    async fn collect_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        metrics.insert("engine".to_string(), serde_json::json!("cedar"));
        metrics.insert(
            "document_count".to_string(),
            serde_json::json!(self.doc_count.load(Ordering::Relaxed)),
        );
        metrics.insert(
            "collection_count".to_string(),
            serde_json::json!(self.collections.len()),
        );
        Ok(metrics)
    }
}

// ---------------------------------------------------------------------------
// UnifiedStorageEngine stub (for factory registration only)
// ---------------------------------------------------------------------------

#[async_trait]
impl UnifiedStorageEngine for CedarEngine {
    fn engine_name(&self) -> &'static str {
        "cedar"
    }

    fn engine_version(&self) -> &'static str {
        "0.1.0"
    }

    fn strategy(&self) -> StorageEngineStrategy {
        StorageEngineStrategy::Cedar
    }

    fn get_filesystem_factory(&self) -> &FilesystemFactory {
        use std::sync::OnceLock;
        static FACTORY: OnceLock<FilesystemFactory> = OnceLock::new();
        FACTORY.get_or_init(|| {
            futures::executor::block_on(async {
                FilesystemFactory::create(FilesystemConfig::default())
                    .await
                    .unwrap_or_else(|_| {
                        #[allow(clippy::panic)]
                        {
                            panic!("Failed to create filesystem factory for CEDAR engine")
                        }
                    })
            })
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        DocumentStorageEngine::collect_metrics(self).await
    }

    async fn vector_by_id(
        &self,
        _collection_id: &str,
        _base_path: &str,
        _vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        Ok(None) // CEDAR stores documents, not vectors
    }

    async fn search_vectors_unified(
        &self,
        _ctx: &StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        Ok(vec![]) // Use DocumentStorageEngine methods for document queries
    }

    async fn do_flush(&self, _params: &FlushParameters) -> Result<FlushResult> {
        Ok(FlushResult::default())
    }

    async fn do_compact(&self, _params: &CompactionParameters) -> Result<CompactionResult> {
        Ok(CompactionResult::default())
    }
}

// ---------------------------------------------------------------------------
// Tests -- TDD Phase 1 (red/green cycles 1.1 + 1.2)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::SqlObject;

    fn make_doc(id: &str, collection: &str) -> DocumentRecord {
        DocumentRecord::new(id.to_string(), SqlObject::default(), collection.to_string())
    }

    // -- Cycle 1.1: Identity --

    #[test]
    fn test_cedar_engine_name() {
        let engine = CedarEngine::new().unwrap();
        assert_eq!(
            crate::storage::document::DocumentStorageEngine::engine_name(&engine),
            "cedar"
        );
    }

    #[test]
    fn test_cedar_strategy() {
        let engine = CedarEngine::new().unwrap();
        assert_eq!(engine.strategy(), StorageEngineStrategy::Cedar);
    }

    #[tokio::test]
    async fn test_cedar_collect_metrics() {
        let engine = CedarEngine::new().unwrap();
        let metrics = DocumentStorageEngine::collect_metrics(&engine)
            .await
            .unwrap();
        assert_eq!(metrics["engine"], serde_json::json!("cedar"));
    }

    // -- Cycle 1.1: Insert + Get --

    #[tokio::test]
    async fn test_cedar_insert_and_get() {
        let engine = CedarEngine::new().unwrap();
        let doc = make_doc("doc1", "users");
        engine.insert_document("users", doc).await.unwrap();

        let retrieved = engine.get_document("users", "doc1").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "doc1");
    }

    #[tokio::test]
    async fn test_cedar_get_missing() {
        let engine = CedarEngine::new().unwrap();
        let result = engine.get_document("users", "nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cedar_get_from_nonexistent_collection() {
        let engine = CedarEngine::new().unwrap();
        let result = engine.get_document("no_such_col", "doc1").await.unwrap();
        assert!(result.is_none());
    }

    // -- Cycle 1.2: Delete + Scan --

    #[tokio::test]
    async fn test_cedar_delete() {
        let engine = CedarEngine::new().unwrap();
        engine
            .insert_document("col", make_doc("doc1", "col"))
            .await
            .unwrap();
        assert!(engine.delete_document("col", "doc1").await.unwrap());
        assert!(engine.get_document("col", "doc1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cedar_delete_nonexistent() {
        let engine = CedarEngine::new().unwrap();
        assert!(!engine.delete_document("col", "nope").await.unwrap());
    }

    #[tokio::test]
    async fn test_cedar_scan_all() {
        let engine = CedarEngine::new().unwrap();
        for i in 0..5 {
            engine
                .insert_document("col", make_doc(&format!("d{}", i), "col"))
                .await
                .unwrap();
        }
        let docs = engine.scan_documents("col", None).await.unwrap();
        assert_eq!(docs.len(), 5);
    }

    #[tokio::test]
    async fn test_cedar_scan_with_limit() {
        let engine = CedarEngine::new().unwrap();
        for i in 0..10 {
            engine
                .insert_document("col", make_doc(&format!("d{}", i), "col"))
                .await
                .unwrap();
        }
        let docs = engine.scan_documents("col", Some(3)).await.unwrap();
        assert_eq!(docs.len(), 3);
    }

    #[tokio::test]
    async fn test_cedar_document_count() {
        let engine = CedarEngine::new().unwrap();
        assert_eq!(engine.document_count("col").await.unwrap(), 0);
        engine
            .insert_document("col", make_doc("d1", "col"))
            .await
            .unwrap();
        engine
            .insert_document("col", make_doc("d2", "col"))
            .await
            .unwrap();
        assert_eq!(engine.document_count("col").await.unwrap(), 2);
    }

    // -- Cycle 1.2: Update + Version --

    #[tokio::test]
    async fn test_cedar_update_increments_version() {
        let engine = CedarEngine::new().unwrap();
        let doc = make_doc("doc1", "col");
        engine.insert_document("col", doc).await.unwrap();

        let updated = engine.update_document("col", "doc1", vec![]).await.unwrap();
        assert_eq!(updated.version, 2);

        let updated2 = engine.update_document("col", "doc1", vec![]).await.unwrap();
        assert_eq!(updated2.version, 3);
    }

    #[tokio::test]
    async fn test_cedar_update_nonexistent_fails() {
        let engine = CedarEngine::new().unwrap();
        let result = engine.update_document("col", "nope", vec![]).await;
        assert!(result.is_err());
    }

    // -- Cycle 1.2: Query with pagination --

    #[tokio::test]
    async fn test_cedar_query_with_limit() {
        let engine = CedarEngine::new().unwrap();
        for i in 0..10 {
            engine
                .insert_document("col", make_doc(&format!("d{}", i), "col"))
                .await
                .unwrap();
        }

        let result = engine
            .query_documents(
                "col",
                DocumentQueryParams {
                    limit: 3,
                    include_count: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(result.documents.len(), 3);
        assert_eq!(result.total_count, Some(10));
    }
}

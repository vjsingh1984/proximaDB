// Document service - CRUD operations for JSON documents
//
// Provides MongoDB-like document operations:
// - Insert/upsert documents with automatic indexing
// - Get by ID with optional projection
// - Update with patch operations (set, unset, inc, push, pull, etc.)
// - Delete by ID or filter
// - Query with filters, sorting, and pagination
// - Aggregate with pipeline operations
//
// Durability: All write operations are WAL-logged before in-memory update

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::metrics::collectors::DocumentMetricsCollector;
use crate::proto::proximadb_v1::{
    DocumentCollectionConfig, DocumentFilter, DocumentUpdate, SqlArray, SqlObject, SqlValue,
    UpdateOperation,
};
use crate::storage::persistence::write_ahead_log::unified_operations::{
    DocumentOperation, UnifiedWALOperation, UnifiedWALWriter,
};
use crate::storage::traits::UnifiedStorageEngine;

use super::aggregation_extensions::{LookupConfig, LookupFetcher};
use super::indexes::IndexManager;
use super::query::QueryExecutor;
use super::query::path_parser::JsonPath;
use super::{
    DocumentCollection, DocumentIngestResult, DocumentQueryParams, DocumentQueryResult,
    DocumentRecord, FlushToStorageResult,
};

/// Document service for CRUD operations
pub struct DocumentService {
    /// Storage engine for persistence
    storage_engine: Arc<dyn UnifiedStorageEngine>,
    /// Index manager for path and full-text indexes
    index_manager: Arc<IndexManager>,
    /// Query executor for filter evaluation
    query_executor: Arc<QueryExecutor>,
    /// Collection metadata cache
    collections: Arc<RwLock<HashMap<String, DocumentCollection>>>,
    /// In-memory document store (hot cache, backed by WAL)
    documents: Arc<RwLock<HashMap<String, HashMap<String, DocumentRecord>>>>,
    /// WAL writer for durability
    wal_writer: Arc<Mutex<Option<UnifiedWALWriter>>>,
    /// Base path for WAL files
    wal_path: String,
    /// Optional metrics collector for observability
    metrics_collector: Option<Arc<DocumentMetricsCollector>>,
}

impl DocumentService {
    /// Create a new document service
    pub fn new(storage_engine: Arc<dyn UnifiedStorageEngine>) -> Self {
        Self {
            storage_engine,
            index_manager: Arc::new(IndexManager::new()),
            query_executor: Arc::new(QueryExecutor::new()),
            collections: Arc::new(RwLock::new(HashMap::new())),
            documents: Arc::new(RwLock::new(HashMap::new())),
            wal_writer: Arc::new(Mutex::new(None)),
            wal_path: String::new(),
            metrics_collector: None,
        }
    }

    /// Create a new document service with metrics collection
    pub fn new_with_metrics(
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        metrics_collector: Arc<DocumentMetricsCollector>,
    ) -> Self {
        Self {
            storage_engine,
            index_manager: Arc::new(IndexManager::new()),
            query_executor: Arc::new(QueryExecutor::new()),
            collections: Arc::new(RwLock::new(HashMap::new())),
            documents: Arc::new(RwLock::new(HashMap::new())),
            wal_writer: Arc::new(Mutex::new(None)),
            wal_path: String::new(),
            metrics_collector: Some(metrics_collector),
        }
    }

    /// Create a new document service with WAL support
    pub async fn new_with_wal(
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        wal_base_path: &str,
    ) -> Result<Self> {
        Self::new_with_wal_and_metrics(storage_engine, wal_base_path, None).await
    }

    /// Create a new document service with WAL support and optional metrics
    pub async fn new_with_wal_and_metrics(
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        wal_base_path: &str,
        metrics_collector: Option<Arc<DocumentMetricsCollector>>,
    ) -> Result<Self> {
        let wal_path = format!("{}/document_wal", wal_base_path);
        let wal_writer = UnifiedWALWriter::new(wal_path.clone())
            .await
            .context("Failed to create document WAL writer")?;

        let mut service = Self {
            storage_engine,
            index_manager: Arc::new(IndexManager::new()),
            query_executor: Arc::new(QueryExecutor::new()),
            collections: Arc::new(RwLock::new(HashMap::new())),
            documents: Arc::new(RwLock::new(HashMap::new())),
            wal_writer: Arc::new(Mutex::new(Some(wal_writer))),
            wal_path,
            metrics_collector,
        };

        // Recover from WAL on startup
        service.recover_from_wal().await?;

        Ok(service)
    }

    /// Record insert metrics if collector is configured
    async fn record_insert_metrics(&self, start: std::time::Instant, is_error: bool) {
        if let Some(ref collector) = self.metrics_collector {
            let latency_us = start.elapsed().as_micros() as f64;
            collector.record_insert(latency_us, is_error).await;
        }
    }

    /// Record update metrics if collector is configured
    async fn record_update_metrics(&self, start: std::time::Instant, is_error: bool) {
        if let Some(ref collector) = self.metrics_collector {
            let latency_us = start.elapsed().as_micros() as f64;
            collector.record_update(latency_us, is_error).await;
        }
    }

    /// Record delete metrics if collector is configured
    async fn record_delete_metrics(&self, start: std::time::Instant, is_error: bool) {
        if let Some(ref collector) = self.metrics_collector {
            let latency_us = start.elapsed().as_micros() as f64;
            collector.record_delete(latency_us, is_error).await;
        }
    }

    /// Record query metrics if collector is configured
    async fn record_query_metrics(&self, start: std::time::Instant, is_error: bool) {
        if let Some(ref collector) = self.metrics_collector {
            let latency_us = start.elapsed().as_micros() as f64;
            collector.record_query(latency_us, is_error).await;
        }
    }

    /// Recover state from WAL on startup
    async fn recover_from_wal(&mut self) -> Result<()> {
        use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALReader;

        info!("Recovering document service from WAL at: {}", self.wal_path);
        let reader = UnifiedWALReader::new(self.wal_path.clone()).await?;
        let entries = reader.read_all().await?;

        let mut recovered_docs = 0;
        let mut recovered_collections = 0;

        for entry in entries {
            if entry.is_document_operation()
                && let UnifiedWALOperation::DocumentOp(op) = entry.operation {
                    match op {
                        DocumentOperation::InsertDocument {
                            collection_id,
                            document,
                        } => {
                            // Replay insert
                            let mut documents = self.documents.write().await;
                            let collection_docs =
                                documents.entry(collection_id).or_default();
                            collection_docs.insert(document.id.clone(), document);
                            recovered_docs += 1;
                        }
                        DocumentOperation::UpdateDocument {
                            collection_id,
                            document_id,
                            new_version,
                            ..
                        } => {
                            // Update version (simplified recovery - full update replay would need stored doc)
                            let mut documents = self.documents.write().await;
                            if let Some(collection_docs) = documents.get_mut(&collection_id)
                                && let Some(doc) = collection_docs.get_mut(&document_id) {
                                    doc.version = new_version;
                                }
                        }
                        DocumentOperation::DeleteDocument {
                            collection_id,
                            document_id,
                        } => {
                            // Replay delete
                            let mut documents = self.documents.write().await;
                            if let Some(collection_docs) = documents.get_mut(&collection_id) {
                                collection_docs.remove(&document_id);
                            }
                        }
                        DocumentOperation::BatchDocuments {
                            collection_id,
                            documents: docs,
                        } => {
                            // Replay batch insert
                            let mut doc_store = self.documents.write().await;
                            let collection_docs =
                                doc_store.entry(collection_id).or_default();
                            for doc in docs {
                                collection_docs.insert(doc.id.clone(), doc);
                                recovered_docs += 1;
                            }
                        }
                        DocumentOperation::CreateCollection {
                            collection_id,
                            config_json,
                        } => {
                            // Replay collection creation
                            if let Ok(config) =
                                serde_json::from_str::<DocumentCollectionConfig>(&config_json)
                            {
                                let collection =
                                    DocumentCollection::new(collection_id.clone(), config);
                                let mut collections = self.collections.write().await;
                                collections.insert(collection_id, collection);
                                recovered_collections += 1;
                            }
                        }
                        DocumentOperation::DeleteCollection { collection_id } => {
                            // Replay collection deletion
                            let mut collections = self.collections.write().await;
                            collections.remove(&collection_id);
                            let mut documents = self.documents.write().await;
                            documents.remove(&collection_id);
                        }
                    }
                }
        }

        info!(
            "WAL recovery complete: {} documents, {} collections recovered",
            recovered_docs, recovered_collections
        );
        Ok(())
    }

    /// Write operation to WAL (if enabled)
    async fn write_to_wal(&self, operation: DocumentOperation) -> Result<()> {
        let mut wal_guard = self.wal_writer.lock().await;
        if let Some(ref mut writer) = *wal_guard {
            let wal_op = UnifiedWALOperation::DocumentOp(operation);
            writer.append(wal_op).await?;
        }
        Ok(())
    }

    /// Flush WAL to disk
    pub async fn flush_wal(&self) -> Result<()> {
        let mut wal_guard = self.wal_writer.lock().await;
        if let Some(ref mut writer) = *wal_guard {
            writer.flush().await?;
        }
        Ok(())
    }

    /// Flush documents from a collection to persistent storage (SST engine)
    ///
    /// This method converts in-memory documents to VectorRecords and flushes them
    /// to the underlying storage engine for cold tier persistence.
    ///
    /// Documents are stored with:
    /// - id: original document ID prefixed with collection (e.g., "mycollection::doc1")
    /// - vector: [0.0] placeholder (documents don't have inherent vectors)
    /// - metadata: "_document" contains serialized JSON, "_collection" for routing
    pub async fn flush_to_storage(&self, collection: &str) -> Result<FlushToStorageResult> {
        use crate::proto::proximadb_v1::VectorRecord;
        use crate::storage::traits::FlushParameters;

        info!(
            "Flushing documents from collection '{}' to storage engine",
            collection
        );
        let start = std::time::Instant::now();

        // Get documents for this collection
        let docs_to_flush: Vec<DocumentRecord> = {
            let documents = self.documents.read().await;
            match documents.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => {
                    warn!(
                        "No documents found in collection '{}' for flush",
                        collection
                    );
                    return Ok(FlushToStorageResult {
                        documents_flushed: 0,
                        bytes_written: 0,
                        duration_ms: start.elapsed().as_millis() as u64,
                        success: true,
                    });
                }
            }
        };

        if docs_to_flush.is_empty() {
            return Ok(FlushToStorageResult {
                documents_flushed: 0,
                bytes_written: 0,
                duration_ms: start.elapsed().as_millis() as u64,
                success: true,
            });
        }

        // Convert documents to VectorRecords
        let vector_records: Vec<VectorRecord> = docs_to_flush
            .iter()
            .filter_map(|doc| self.document_to_vector_record(doc, collection))
            .collect();

        let record_count = vector_records.len();
        let estimated_size: usize = vector_records
            .iter()
            .map(|r| r.id.len() + 4 + r.metadata.len() * 50) // rough estimate
            .sum();

        // Build flush parameters
        let params = FlushParameters {
            collection_id: Some(format!("_documents_{}", collection)),
            force: true,
            synchronous: true,
            vector_records,
            trigger_compaction: false,
            estimated_size,
            ..Default::default()
        };

        // Flush to storage engine
        let result = self.storage_engine.flush(params).await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        info!(
            "Flushed {} documents from '{}' to storage in {}ms",
            record_count, collection, duration_ms
        );

        Ok(FlushToStorageResult {
            documents_flushed: record_count,
            bytes_written: result.bytes_written.unwrap_or(0) as usize,
            duration_ms,
            success: result.success,
        })
    }

    /// Convert a DocumentRecord to VectorRecord for storage engine persistence
    fn document_to_vector_record(
        &self,
        doc: &DocumentRecord,
        collection: &str,
    ) -> Option<crate::proto::proximadb_v1::VectorRecord> {
        use crate::proto::proximadb_v1::sql_value::Value;
        use crate::proto::proximadb_v1::{SqlValue, VectorRecord};

        // Serialize the document to JSON string
        let doc_json = match serde_json::to_string(&doc.document) {
            Ok(json) => json,
            Err(e) => {
                warn!("Failed to serialize document {}: {}", doc.id, e);
                return None;
            }
        };

        // Build metadata map
        let mut metadata = HashMap::new();

        // Store document type marker
        metadata.insert(
            "_type".to_string(),
            SqlValue {
                value: Some(Value::StringValue("document".to_string())),
            },
        );

        // Store collection for routing
        metadata.insert(
            "_collection".to_string(),
            SqlValue {
                value: Some(Value::StringValue(collection.to_string())),
            },
        );

        // Store serialized document
        metadata.insert(
            "_document".to_string(),
            SqlValue {
                value: Some(Value::StringValue(doc_json)),
            },
        );

        // Store version
        metadata.insert(
            "_version".to_string(),
            SqlValue {
                value: Some(Value::Int64Value(doc.version as i64)),
            },
        );

        Some(VectorRecord {
            id: format!("{}::{}", collection, doc.id),
            vector: vec![0.0], // Placeholder - documents don't have vectors
            metadata,
            timestamp: Some(doc.updated_at_ns / 1_000_000), // Convert ns to ms
            updated_at: Some(doc.updated_at_ns / 1_000_000),
            expires_at: None,
            version: Some(doc.version as u32), // VectorRecord uses u32, DocumentRecord uses u64
            source: None,
        })
    }

    /// Read documents from storage engine (cold tier)
    ///
    /// This is used for documents that have been flushed to storage
    /// but evicted from the in-memory hot cache.
    ///
    /// The implementation follows SOLID principles:
    /// - Uses DocumentMetadataFilterBuilder to construct the search filter (SRP)
    /// - Uses ColdTierRetriever trait for storage access (DIP)
    /// - Can be extended for different storage backends (OCP)
    ///
    /// Documents are stored as VectorRecords with metadata:
    /// - `_type`: "document"
    /// - `_collection`: collection name
    /// - `_document`: serialized JSON content
    /// - `_version`: document version
    #[allow(dead_code)]
    pub async fn read_from_storage(
        &self,
        collection: &str,
        ids: &[&str],
    ) -> Result<Vec<DocumentRecord>> {
        use crate::storage::document::storage::cold_tier::{
            ColdTierRetriever, StorageEngineColdTierRetriever,
        };

        if ids.is_empty() {
            return Ok(Vec::new());
        }

        debug!(
            "Cold tier: Reading {} documents from collection '{}'",
            ids.len(),
            collection
        );

        // Create the cold tier retriever using the storage engine
        let retriever = StorageEngineColdTierRetriever::new(self.storage_engine.clone());

        // Retrieve documents from cold storage
        match retriever.retrieve_documents(collection, ids).await {
            Ok(documents) => {
                debug!(
                    "Cold tier: Successfully retrieved {} of {} requested documents",
                    documents.len(),
                    ids.len()
                );
                Ok(documents)
            }
            Err(e) => {
                warn!(
                    "Cold tier: Failed to retrieve documents from collection '{}': {}",
                    collection, e
                );
                // Return empty result on error to allow graceful degradation
                // The caller can fall back to other retrieval mechanisms
                Ok(Vec::new())
            }
        }
    }

    /// Convert a VectorRecord back to DocumentRecord
    #[allow(dead_code)]
    fn vector_record_to_document(
        &self,
        record: &crate::proto::proximadb_v1::VectorRecord,
    ) -> Option<DocumentRecord> {
        use crate::proto::proximadb_v1::sql_value::Value;

        // Check if this is a document record
        let type_value = record.metadata.get("_type")?;
        if let Some(Value::StringValue(t)) = &type_value.value {
            if t != "document" {
                return None;
            }
        } else {
            return None;
        }

        // Get collection
        let collection = record.metadata.get("_collection")?;
        let collection_name = if let Some(Value::StringValue(c)) = &collection.value {
            c.clone()
        } else {
            return None;
        };

        // Get document JSON
        let doc_value = record.metadata.get("_document")?;
        let doc_json = if let Some(Value::StringValue(j)) = &doc_value.value {
            j.clone()
        } else {
            return None;
        };

        // Deserialize document
        let document: SqlObject = match serde_json::from_str(&doc_json) {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to deserialize document from storage: {}", e);
                return None;
            }
        };

        // Extract original ID (remove collection prefix)
        let original_id = record
            .id
            .strip_prefix(&format!("{}::", collection_name))
            .unwrap_or(&record.id)
            .to_string();

        // Get version
        let version = record
            .metadata
            .get("_version")
            .and_then(|v| {
                if let Some(Value::Int64Value(i)) = &v.value {
                    Some(*i as u64)
                } else {
                    None
                }
            })
            .unwrap_or(0);

        Some(DocumentRecord {
            id: original_id,
            document,
            collection_id: collection_name,
            version,
            updated_at_ns: record.updated_at.unwrap_or(0) * 1_000_000, // Convert ms to ns
            schema_id: None,
            document_type: None,
        })
    }

    /// Flush all collections to storage
    pub async fn flush_all_to_storage(&self) -> Result<FlushToStorageResult> {
        info!("Flushing all document collections to storage");
        let start = std::time::Instant::now();

        let collection_names: Vec<String> = {
            let collections = self.collections.read().await;
            collections.keys().cloned().collect()
        };

        let mut total_docs = 0;
        let mut total_bytes = 0;

        for collection_name in collection_names {
            let result = self.flush_to_storage(&collection_name).await?;
            total_docs += result.documents_flushed;
            total_bytes += result.bytes_written;
        }

        Ok(FlushToStorageResult {
            documents_flushed: total_docs,
            bytes_written: total_bytes,
            duration_ms: start.elapsed().as_millis() as u64,
            success: true,
        })
    }

    /// Create a new document collection
    pub async fn create_collection(
        &self,
        name: &str,
        config: DocumentCollectionConfig,
    ) -> Result<String> {
        info!("Creating document collection: {}", name);

        // Check if collection already exists
        {
            let collections = self.collections.read().await;
            if collections.contains_key(name) {
                return Err(anyhow!("Collection '{}' already exists", name));
            }
        }

        // Write to WAL first (durability before in-memory update)
        let config_json =
            serde_json::to_string(&config).context("Failed to serialize collection config")?;
        self.write_to_wal(DocumentOperation::CreateCollection {
            collection_id: name.to_string(),
            config_json,
        })
        .await?;

        // Create collection metadata
        let collection = DocumentCollection::new(name.to_string(), config.clone());

        // Initialize indexes
        for index_def in &config.indexes {
            self.index_manager
                .create_index(name, index_def)
                .await
                .context("Failed to create index")?;
        }

        // Store collection metadata
        {
            let mut collections = self.collections.write().await;
            collections.insert(name.to_string(), collection);
        }

        info!("Created document collection: {}", name);
        Ok(name.to_string())
    }

    /// Get collection metadata
    pub async fn get_collection(&self, name: &str) -> Result<Option<DocumentCollection>> {
        let collections = self.collections.read().await;
        Ok(collections.get(name).cloned())
    }

    /// List all collections
    pub async fn list_collections(&self) -> Result<Vec<DocumentCollection>> {
        let collections = self.collections.read().await;
        Ok(collections.values().cloned().collect())
    }

    /// Delete a collection
    pub async fn delete_collection(&self, name: &str) -> Result<bool> {
        info!("Deleting document collection: {}", name);

        // Check if collection exists first
        let exists = {
            let collections = self.collections.read().await;
            collections.contains_key(name)
        };

        if !exists {
            return Ok(false);
        }

        // Write to WAL first (durability before in-memory update)
        self.write_to_wal(DocumentOperation::DeleteCollection {
            collection_id: name.to_string(),
        })
        .await?;

        // Remove from cache
        {
            let mut collections = self.collections.write().await;
            collections.remove(name);
        }

        // Drop indexes
        self.index_manager.drop_collection_indexes(name).await?;

        // Remove documents
        {
            let mut documents = self.documents.write().await;
            documents.remove(name);
        }

        Ok(true)
    }

    /// Insert a single document
    pub async fn insert_document(
        &self,
        collection: &str,
        id: Option<&str>,
        document: SqlObject,
    ) -> Result<DocumentRecord> {
        let start = std::time::Instant::now();
        debug!("Inserting document into collection: {}", collection);

        // Verify collection exists
        let _collection_meta = match self
            .get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))
        {
            Ok(meta) => meta,
            Err(e) => {
                self.record_insert_metrics(start, true).await;
                return Err(e);
            }
        };

        // Generate ID if not provided
        let doc_id = id.map_or_else(|| uuid::Uuid::new_v4().to_string(), |s| s.to_string());

        // Create document record
        let record = DocumentRecord::new(doc_id.clone(), document, collection.to_string());

        // Write to WAL first (durability before in-memory update)
        if let Err(e) = self
            .write_to_wal(DocumentOperation::InsertDocument {
                collection_id: collection.to_string(),
                document: record.clone(),
            })
            .await
        {
            self.record_insert_metrics(start, true).await;
            return Err(e);
        }

        // Update indexes
        if let Err(e) = self.index_manager.index_document(collection, &record).await {
            self.record_insert_metrics(start, true).await;
            return Err(e);
        }

        // Store document in memory (backed by WAL for durability)
        {
            let mut documents = self.documents.write().await;
            let collection_docs = documents
                .entry(collection.to_string())
                .or_default();
            collection_docs.insert(doc_id.clone(), record.clone());
        }

        // Update collection stats
        {
            let mut collections = self.collections.write().await;
            if let Some(coll) = collections.get_mut(collection) {
                coll.document_count += 1;
                coll.updated_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
            }
        }

        debug!("Inserted document {} into {}", doc_id, collection);
        self.record_insert_metrics(start, false).await;
        Ok(record)
    }

    /// Insert multiple documents in batch
    pub async fn insert_documents(
        &self,
        collection: &str,
        documents: Vec<(Option<String>, SqlObject)>,
    ) -> Result<DocumentIngestResult> {
        let start = std::time::Instant::now();
        let mut result = DocumentIngestResult::default();

        for (id, doc) in documents {
            match self.insert_document(collection, id.as_deref(), doc).await {
                Ok(_) => result.ingested += 1,
                Err(e) => {
                    result.failed += 1;
                    result.errors.push(e.to_string());
                }
            }
        }

        result.processing_time_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// Get a document by ID
    pub async fn get_document(
        &self,
        collection: &str,
        id: &str,
        projection: Option<Vec<String>>,
    ) -> Result<Option<DocumentRecord>> {
        debug!("Getting document {} from {}", id, collection);

        // Verify collection exists
        self.get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;

        // Retrieve from in-memory store
        let documents = self.documents.read().await;
        if let Some(collection_docs) = documents.get(collection)
            && let Some(record) = collection_docs.get(id) {
                let mut result = record.clone();

                // Apply projection if specified
                if let Some(fields) = projection
                    && !fields.is_empty() {
                        result.document = self.apply_projection(&result.document, &fields);
                    }

                return Ok(Some(result));
            }

        Ok(None)
    }

    /// Apply field projection to a document
    fn apply_projection(&self, document: &SqlObject, fields: &[String]) -> SqlObject {
        let mut projected = SqlObject {
            fields: HashMap::new(),
        };

        for field in fields {
            // Parse field as JSON path and extract value
            if let Ok(path) = JsonPath::parse(&format!("$.{}", field)) {
                let values = path.evaluate(document);
                if let Some(value) = values.into_iter().next() {
                    // Use the field name as key (simplified - doesn't handle nested paths)
                    let key = field.split('.').next_back().unwrap_or(field);
                    projected.fields.insert(key.to_string(), value);
                }
            } else if let Some(value) = document.fields.get(field) {
                // Direct field access if path parsing fails
                projected.fields.insert(field.clone(), value.clone());
            }
        }

        projected
    }

    /// Update a document by ID
    pub async fn update_document(
        &self,
        collection: &str,
        id: &str,
        updates: Vec<DocumentUpdate>,
        expected_version: Option<u64>,
    ) -> Result<DocumentRecord> {
        let start = std::time::Instant::now();
        debug!("Updating document {} in {}", id, collection);

        // Get existing document
        let mut record = match self
            .get_document(collection, id, None)
            .await?
            .ok_or_else(|| anyhow!("Document '{}' not found", id))
        {
            Ok(r) => r,
            Err(e) => {
                self.record_update_metrics(start, true).await;
                return Err(e);
            }
        };

        // Check version for optimistic locking
        if let Some(expected) = expected_version
            && record.version != expected {
                self.record_update_metrics(start, true).await;
                return Err(anyhow!(
                    "Version mismatch: expected {}, got {}",
                    expected,
                    record.version
                ));
            }

        // Apply updates
        for update in &updates {
            if let Err(e) = self.apply_update(&mut record.document, update) {
                self.record_update_metrics(start, true).await;
                return Err(e);
            }
        }

        // Increment version
        let new_version = record.version + 1;
        record.version = new_version;
        record.updated_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        // Write to WAL first (durability before in-memory update)
        // Store full updated document for proper recovery replay
        if let Err(e) = self
            .write_to_wal(DocumentOperation::InsertDocument {
                collection_id: collection.to_string(),
                document: record.clone(),
            })
            .await
        {
            self.record_update_metrics(start, true).await;
            return Err(e);
        }

        // Update indexes
        if let Err(e) = self
            .index_manager
            .reindex_document(collection, &record)
            .await
        {
            self.record_update_metrics(start, true).await;
            return Err(e);
        }

        // Persist updated document to in-memory store
        {
            let mut documents = self.documents.write().await;
            if let Some(collection_docs) = documents.get_mut(collection) {
                collection_docs.insert(id.to_string(), record.clone());
            }
        }

        debug!("Updated document {} in {}", id, collection);
        self.record_update_metrics(start, false).await;
        Ok(record)
    }

    /// Apply a single update operation to a document
    fn apply_update(&self, document: &mut SqlObject, update: &DocumentUpdate) -> Result<()> {
        let path = &update.path;
        let value = update.value.as_ref();

        match UpdateOperation::try_from(update.operation).unwrap_or(UpdateOperation::Unspecified) {
            UpdateOperation::Set => {
                if let Some(v) = value {
                    self.set_path_value(document, path, v.clone())?;
                }
            }
            UpdateOperation::Unset => {
                self.unset_path(document, path)?;
            }
            UpdateOperation::Inc => {
                if let Some(v) = value {
                    self.increment_path(document, path, v)?;
                }
            }
            UpdateOperation::Push => {
                if let Some(v) = value {
                    self.push_to_array(document, path, v.clone())?;
                }
            }
            UpdateOperation::Pull => {
                if let Some(v) = value {
                    self.pull_from_array(document, path, v)?;
                }
            }
            UpdateOperation::AddToSet => {
                if let Some(v) = value {
                    self.add_to_set(document, path, v.clone())?;
                }
            }
            UpdateOperation::Rename => {
                if let Some(v) = value {
                    // Value should be the new path name
                    self.rename_path(document, path, v)?;
                }
            }
            UpdateOperation::Unspecified => {
                return Err(anyhow!("Unspecified update operation"));
            }
        }

        Ok(())
    }

    // Path manipulation helpers

    /// Set a value at a JSON path
    fn set_path_value(&self, doc: &mut SqlObject, path: &str, value: SqlValue) -> Result<()> {
        // Parse path segments (simplified: handle dot notation)
        let parts: Vec<&str> = path.trim_start_matches("$.").split('.').collect();

        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            return Err(anyhow!("Empty path"));
        }

        // Navigate to parent and set final field
        let mut current = doc;
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                // Last segment - set the value
                current.fields.insert(part.to_string(), value.clone());
            } else {
                // Navigate to nested object, creating if needed
                let entry = current
                    .fields
                    .entry(part.to_string())
                    .or_insert_with(|| SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(
                            SqlObject {
                                fields: HashMap::new(),
                            },
                        )),
                    });

                // Get mutable reference to nested object
                if let Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(
                    ref mut obj,
                )) = entry.value
                {
                    current = obj;
                } else {
                    return Err(anyhow!("Path {} is not an object", part));
                }
            }
        }

        Ok(())
    }

    /// Remove a field at a JSON path
    fn unset_path(&self, doc: &mut SqlObject, path: &str) -> Result<()> {
        let parts: Vec<&str> = path.trim_start_matches("$.").split('.').collect();

        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            return Err(anyhow!("Empty path"));
        }

        // Navigate to parent
        let mut current = doc;
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                // Last segment - remove the field
                current.fields.remove(*part);
                return Ok(());
            } else {
                // Navigate to nested object
                if let Some(entry) = current.fields.get_mut(*part) {
                    if let Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(
                        ref mut obj,
                    )) = entry.value
                    {
                        current = obj;
                    } else {
                        return Err(anyhow!("Path {} is not an object", part));
                    }
                } else {
                    return Ok(()); // Path doesn't exist, nothing to unset
                }
            }
        }

        Ok(())
    }

    /// Increment a numeric value at a path
    fn increment_path(&self, doc: &mut SqlObject, path: &str, value: &SqlValue) -> Result<()> {
        let parts: Vec<&str> = path.trim_start_matches("$.").split('.').collect();

        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            return Err(anyhow!("Empty path"));
        }

        // Get the increment value
        let inc = match &value.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => *i as f64,
            Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(f)) => *f,
            _ => return Err(anyhow!("Increment value must be numeric")),
        };

        // Navigate to the field and increment
        let mut current = doc;
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                // Last segment - increment the value
                if let Some(field) = current.fields.get_mut(*part) {
                    match &mut field.value {
                        Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(v)) => {
                            *v += inc as i64;
                        }
                        Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(v)) => {
                            *v += inc;
                        }
                        _ => return Err(anyhow!("Field at path {} is not numeric", path)),
                    }
                } else {
                    // Initialize to increment value
                    current.fields.insert(
                        part.to_string(),
                        SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(
                                inc,
                            )),
                        },
                    );
                }
                return Ok(());
            } else if let Some(entry) = current.fields.get_mut(*part) {
                if let Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(
                    ref mut obj,
                )) = entry.value
                {
                    current = obj;
                } else {
                    return Err(anyhow!("Path {} is not an object", part));
                }
            } else {
                return Err(anyhow!("Path {} does not exist", part));
            }
        }

        Ok(())
    }

    /// Push a value to an array at a path
    fn push_to_array(&self, doc: &mut SqlObject, path: &str, value: SqlValue) -> Result<()> {
        let parts: Vec<&str> = path.trim_start_matches("$.").split('.').collect();

        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            return Err(anyhow!("Empty path"));
        }

        // Navigate to the field and push
        let mut current = doc;
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                // Last segment - push to array
                if let Some(field) = current.fields.get_mut(*part) {
                    if let Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(
                        ref mut arr,
                    )) = field.value
                    {
                        arr.values.push(value);
                        return Ok(());
                    } else {
                        return Err(anyhow!("Field at path {} is not an array", path));
                    }
                } else {
                    // Initialize as new array with single value
                    current.fields.insert(
                        part.to_string(),
                        SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(
                                crate::proto::proximadb_v1::SqlArray {
                                    values: vec![value],
                                },
                            )),
                        },
                    );
                    return Ok(());
                }
            } else if let Some(entry) = current.fields.get_mut(*part) {
                if let Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(
                    ref mut obj,
                )) = entry.value
                {
                    current = obj;
                } else {
                    return Err(anyhow!("Path {} is not an object", part));
                }
            } else {
                return Err(anyhow!("Path {} does not exist", part));
            }
        }

        Ok(())
    }

    /// Pull (remove) a value from an array at a path
    fn pull_from_array(&self, doc: &mut SqlObject, path: &str, value: &SqlValue) -> Result<()> {
        let parts: Vec<&str> = path.trim_start_matches("$.").split('.').collect();

        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            return Err(anyhow!("Empty path"));
        }

        // Navigate to the field and pull
        let mut current = doc;
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                if let Some(field) = current.fields.get_mut(*part) {
                    if let Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(
                        ref mut arr,
                    )) = field.value
                    {
                        arr.values.retain(|v| v != value);
                        return Ok(());
                    } else {
                        return Err(anyhow!("Field at path {} is not an array", path));
                    }
                }
                return Ok(()); // Field doesn't exist
            } else if let Some(entry) = current.fields.get_mut(*part) {
                if let Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(
                    ref mut obj,
                )) = entry.value
                {
                    current = obj;
                } else {
                    return Err(anyhow!("Path {} is not an object", part));
                }
            } else {
                return Ok(()); // Path doesn't exist
            }
        }

        Ok(())
    }

    /// Add a value to a set (array with unique values) at a path
    fn add_to_set(&self, doc: &mut SqlObject, path: &str, value: SqlValue) -> Result<()> {
        let parts: Vec<&str> = path.trim_start_matches("$.").split('.').collect();

        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            return Err(anyhow!("Empty path"));
        }

        // Navigate to the field and add if not exists
        let mut current = doc;
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                if let Some(field) = current.fields.get_mut(*part) {
                    if let Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(
                        ref mut arr,
                    )) = field.value
                    {
                        // Only add if not already present
                        if !arr.values.contains(&value) {
                            arr.values.push(value);
                        }
                        return Ok(());
                    } else {
                        return Err(anyhow!("Field at path {} is not an array", path));
                    }
                } else {
                    // Initialize as new array with single value
                    current.fields.insert(
                        part.to_string(),
                        SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(
                                crate::proto::proximadb_v1::SqlArray {
                                    values: vec![value],
                                },
                            )),
                        },
                    );
                    return Ok(());
                }
            } else if let Some(entry) = current.fields.get_mut(*part) {
                if let Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(
                    ref mut obj,
                )) = entry.value
                {
                    current = obj;
                } else {
                    return Err(anyhow!("Path {} is not an object", part));
                }
            } else {
                return Err(anyhow!("Path {} does not exist", part));
            }
        }

        Ok(())
    }

    /// Rename a field at a path
    fn rename_path(&self, doc: &mut SqlObject, old_path: &str, new_name: &SqlValue) -> Result<()> {
        // Get new name from SqlValue
        let new_field_name = match &new_name.value {
            Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => s.clone(),
            _ => return Err(anyhow!("New name must be a string")),
        };

        let parts: Vec<&str> = old_path.trim_start_matches("$.").split('.').collect();

        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            return Err(anyhow!("Empty path"));
        }

        // Navigate to parent and rename
        let mut current = doc;
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                // Last segment - rename the field
                if let Some(value) = current.fields.remove(*part) {
                    current.fields.insert(new_field_name, value);
                }
                return Ok(());
            } else if let Some(entry) = current.fields.get_mut(*part) {
                if let Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(
                    ref mut obj,
                )) = entry.value
                {
                    current = obj;
                } else {
                    return Err(anyhow!("Path {} is not an object", part));
                }
            } else {
                return Ok(()); // Path doesn't exist
            }
        }

        Ok(())
    }

    /// Delete a document by ID
    pub async fn delete_document(&self, collection: &str, id: &str) -> Result<bool> {
        let start = std::time::Instant::now();
        debug!("Deleting document {} from {}", id, collection);

        // Verify collection exists
        if let Err(e) = self
            .get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))
        {
            self.record_delete_metrics(start, true).await;
            return Err(e);
        }

        // Check if document exists before WAL write
        let exists = {
            let documents = self.documents.read().await;
            documents
                .get(collection)
                .is_some_and(|docs| docs.contains_key(id))
        };

        if !exists {
            self.record_delete_metrics(start, false).await;
            return Ok(false);
        }

        // Write to WAL first (durability before in-memory update)
        if let Err(e) = self
            .write_to_wal(DocumentOperation::DeleteDocument {
                collection_id: collection.to_string(),
                document_id: id.to_string(),
            })
            .await
        {
            self.record_delete_metrics(start, true).await;
            return Err(e);
        }

        // Remove from indexes
        if let Err(e) = self.index_manager.remove_document(collection, id).await {
            self.record_delete_metrics(start, true).await;
            return Err(e);
        }

        // Remove from in-memory store
        {
            let mut documents = self.documents.write().await;
            if let Some(collection_docs) = documents.get_mut(collection) {
                collection_docs.remove(id);
            }
        }

        // Update collection stats
        {
            let mut collections = self.collections.write().await;
            if let Some(coll) = collections.get_mut(collection) {
                if coll.document_count > 0 {
                    coll.document_count -= 1;
                }
                coll.updated_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
            }
        }

        debug!("Deleted document {} from {}", id, collection);
        self.record_delete_metrics(start, false).await;
        Ok(true)
    }

    /// Query documents with filter, sort, and pagination
    pub async fn query_documents(
        &self,
        collection: &str,
        params: DocumentQueryParams,
    ) -> Result<DocumentQueryResult> {
        let start = std::time::Instant::now();
        debug!("Querying documents in {}", collection);

        // Verify collection exists
        let _collection_meta = match self
            .get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))
        {
            Ok(meta) => meta,
            Err(e) => {
                self.record_query_metrics(start, true).await;
                return Err(e);
            }
        };

        // Execute query
        let documents: Vec<DocumentRecord> = {
            let docs = self.documents.read().await;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        let (documents, total_count) = match self
            .query_executor
            .execute(collection, &documents, &params, &self.index_manager)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                self.record_query_metrics(start, true).await;
                return Err(e);
            }
        };

        let query_time_ms = start.elapsed().as_millis() as u64;
        self.record_query_metrics(start, false).await;

        Ok(DocumentQueryResult {
            documents,
            total_count: if params.include_count {
                Some(total_count)
            } else {
                None
            },
            query_time_ms,
        })
    }

    /// Aggregate documents with a pipeline
    ///
    /// Executes a MongoDB-style aggregation pipeline on documents in a collection.
    /// Supports stages: match, group, project, sort, limit, skip, unwind.
    ///
    /// # Arguments
    /// * `collection` - The collection name
    /// * `filter` - Optional filter to apply before the pipeline
    /// * `pipeline` - The aggregation pipeline stages
    ///
    /// # Returns
    /// A vector of aggregated result documents
    pub async fn aggregate_documents(
        &self,
        collection: &str,
        filter: Option<crate::proto::proximadb_v1::DocumentFilter>,
        pipeline: Vec<crate::proto::proximadb_v1::AggregationStage>,
    ) -> Result<crate::storage::document::AggregateResult> {
        use crate::storage::document::aggregation::AggregationExecutor;

        let start = std::time::Instant::now();
        debug!("Aggregating documents in {}", collection);

        // Verify collection exists
        self.get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;

        // Get all documents from the collection
        let documents: Vec<DocumentRecord> = {
            let docs = self.documents.read().await;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        // Execute the aggregation pipeline
        let executor = AggregationExecutor::new();
        let results = executor.execute(documents, filter.as_ref(), &pipeline)?;

        let query_time_ms = start.elapsed().as_millis() as u64;
        debug!(
            "Aggregation complete: {} results in {}ms",
            results.len(),
            query_time_ms
        );

        Ok(crate::storage::document::AggregateResult {
            results,
            query_time_ms,
        })
    }

    /// Aggregate documents with $lookup support
    ///
    /// Extended aggregation that supports cross-collection joins via the $lookup stage.
    /// For each lookup stage in the pipeline, this method fetches matching documents
    /// from the foreign collection and merges them into the local documents.
    ///
    /// # Arguments
    /// * `collection` - Source collection name
    /// * `filter` - Optional filter to apply before aggregation
    /// * `pipeline` - Aggregation pipeline stages (may include $lookup)
    ///
    /// # Returns
    /// Aggregated results with lookup fields populated
    ///
    /// # Note
    /// For lookup stages, this method requires Arc<DocumentService>. Use the async variant
    /// that takes Arc<DocumentService> for full lookup support.
    pub async fn aggregate_documents_with_lookup(
        &self,
        collection: &str,
        filter: Option<DocumentFilter>,
        pipeline: Vec<crate::proto::proximadb_v1::AggregationStage>,
    ) -> Result<crate::storage::document::AggregateResult> {
        use crate::storage::document::aggregation::AggregationExecutor;

        let start = std::time::Instant::now();
        debug!(
            "Aggregating documents in {} with lookup support",
            collection
        );

        // Verify source collection exists
        self.get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;

        // Get all documents from the source collection
        let documents: Vec<DocumentRecord> = {
            let docs = self.documents.read().await;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        // Check if pipeline contains lookup stages
        let has_lookup = pipeline.iter().any(|stage| {
            matches!(&stage.stage, Some(crate::proto::proximadb_v1::aggregation_stage::Stage::Lookup(_)))
        });

        if has_lookup {
            // For lookup stages, caller should use aggregate_documents_with_lookup_arc
            return Err(anyhow!(
                "Pipeline contains $lookup stages - use aggregate_documents_with_lookup_arc which takes Arc<DocumentService>"
            ));
        }

        // Execute the aggregation pipeline (without lookup support)
        let executor = AggregationExecutor::new();
        let results = executor.execute(documents, filter.as_ref(), &pipeline)?;

        let query_time_ms = start.elapsed().as_millis() as u64;
        debug!(
            "Aggregation complete: {} results in {}ms",
            results.len(),
            query_time_ms
        );

        Ok(crate::storage::document::AggregateResult {
            results,
            query_time_ms,
        })
    }

    /// Aggregate documents with $lookup support (Arc variant)
    ///
    /// This is the preferred method for aggregation pipelines that include $lookup stages.
    /// It takes Arc<DocumentService> which allows the lookup fetcher to query foreign collections.
    ///
    /// # Arguments
    /// * `collection` - Source collection name
    /// * `filter` - Optional filter to apply before aggregation
    /// * `pipeline` - Aggregation pipeline stages (may include $lookup)
    ///
    /// # Returns
    /// Aggregated results with lookup fields populated
    pub async fn aggregate_documents_with_lookup_arc(
        this: Arc<DocumentService>,
        collection: &str,
        filter: Option<DocumentFilter>,
        pipeline: Vec<crate::proto::proximadb_v1::AggregationStage>,
    ) -> Result<crate::storage::document::AggregateResult> {
        use crate::storage::document::aggregation::AggregationExecutor;

        let start = std::time::Instant::now();
        debug!(
            "Aggregating documents in {} with lookup support (Arc variant)",
            collection
        );

        // Verify source collection exists
        this.get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;

        // Get all documents from the source collection
        let documents: Vec<DocumentRecord> = {
            let docs = this.documents.read().await;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        // Create a lookup fetcher that can query foreign collections
        let fetcher = DocumentServiceLookupFetcher {
            service: this.clone(),
        };

        // Execute aggregation pipeline with lookup support
        let executor = AggregationExecutor::new();
        let mut working_set: Vec<SqlObject> = if let Some(f) = &filter {
            documents
                .iter()
                .filter(|doc| executor.matches_filter(doc, f))
                .map(|doc| doc.document.clone())
                .collect()
        } else {
            documents.into_iter().map(|doc| doc.document).collect()
        };

        // Process each stage, handling lookups specially
        for (stage_idx, stage) in pipeline.iter().enumerate() {
            use crate::proto::proximadb_v1::aggregation_stage::Stage;

            match &stage.stage {
                Some(Stage::Lookup(lookup_stage)) => {
                    working_set = executor.process_lookup(&working_set, lookup_stage, &fetcher)?;
                    debug!("After lookup stage {}: {} documents", stage_idx, working_set.len());
                }
                Some(_) => {
                    // Use the standard process_stage for non-lookup stages
                    working_set = executor.process_stage(&working_set, stage, stage_idx)?;
                    debug!("After stage {}: {} documents", stage_idx, working_set.len());
                }
                None => {
                    return Err(anyhow!("Empty stage at index {}", stage_idx));
                }
            }
        }

        let query_time_ms = start.elapsed().as_millis() as u64;
        debug!(
            "Aggregation with lookup complete: {} results in {}ms",
            working_set.len(),
            query_time_ms
        );

        Ok(crate::storage::document::AggregateResult {
            results: working_set,
            query_time_ms,
        })
    }

    /// Calculate full-text search scores for documents matching a query
    ///
    /// This provides basic TF-IDF-like scoring for text matching.
    /// For production use, integrate with Tantivy index.
    ///
    /// # Arguments
    /// * `collection` - The collection name
    /// * `query_terms` - Terms to search for
    /// * `text_paths` - Document paths to search in (e.g., ["title", "body"])
    /// * `limit` - Maximum number of results
    ///
    /// # Returns
    /// Documents sorted by relevance score (highest first)
    pub async fn fulltext_search(
        &self,
        collection: &str,
        query_terms: Vec<String>,
        text_paths: Vec<String>,
        limit: usize,
    ) -> Result<Vec<(DocumentRecord, f32)>> {
        use crate::storage::document::aggregation::AggregationExecutor;

        debug!(
            "Full-text search in {} for terms: {:?}",
            collection, query_terms
        );

        // Verify collection exists
        self.get_collection(collection)
            .await?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection))?;

        // Get all documents from the collection
        let documents: Vec<DocumentRecord> = {
            let docs = self.documents.read().await;
            match docs.get(collection) {
                Some(collection_docs) => collection_docs.values().cloned().collect(),
                None => Vec::new(),
            }
        };

        // Calculate scores
        let executor = AggregationExecutor::new();
        let mut scored = executor.calculate_fulltext_scores(&documents, &query_terms, &text_paths);

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Apply limit
        scored.truncate(limit);

        Ok(scored)
    }
}

// =============================================================================
// TRAIT IMPLEMENTATION: DocumentStorageOperations
// =============================================================================
// This implements the SOLID-compliant trait interface for document operations,
// bridging the existing DocumentService to the new multi-model storage traits.

use crate::storage::traits::{
    DocumentCollectionInfo as TraitDocCollectionInfo, DocumentRecord as TraitDocRecord,
    DocumentStorageOperations,
};
use async_trait::async_trait;

/// Convert internal DocumentRecord to trait DocumentRecord
fn to_trait_doc_record(doc: &DocumentRecord) -> TraitDocRecord {
    TraitDocRecord {
        id: doc.id.clone(),
        document: doc.document.clone(),
        version: doc.version,
        // Use updated_at_ns for both since internal type doesn't have created_at_ns
        created_at_ns: doc.updated_at_ns,
        updated_at_ns: doc.updated_at_ns,
    }
}

#[async_trait]
impl DocumentStorageOperations for DocumentService {
    async fn insert_document(
        &self,
        collection: &str,
        id: &str,
        document: SqlObject,
        indexed_paths: Vec<String>,
    ) -> Result<TraitDocRecord> {
        // Use existing insert_document method (with impl block method name)
        let doc_record = DocumentService::insert_document(
            self,
            collection,
            Some(id), // Already &str, no conversion needed
            document,
        )
        .await?;

        // Note: index_document indexes all configured paths for the document
        // The indexed_paths parameter specifies which paths to index from config
        // The actual indexing is done during insert_document call above
        // We log if additional path-specific indexing is requested but not yet supported
        if !indexed_paths.is_empty() {
            debug!(
                "Additional indexed_paths specified: {:?} (already indexed via collection config)",
                indexed_paths
            );
        }

        Ok(to_trait_doc_record(&doc_record))
    }

    async fn get_document(&self, collection: &str, id: &str) -> Result<Option<TraitDocRecord>> {
        // Use existing get_document method with projection=None
        match DocumentService::get_document(self, collection, id, None).await? {
            Some(doc) => Ok(Some(to_trait_doc_record(&doc))),
            None => Ok(None),
        }
    }

    async fn query_documents(
        &self,
        collection: &str,
        filter: Option<DocumentFilter>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<TraitDocRecord>> {
        let params = DocumentQueryParams {
            filter,
            projection: Vec::new(),
            sort: Vec::new(),
            limit: limit as u32,
            offset: offset as u32,
            include_count: false,
        };

        let result = DocumentService::query_documents(self, collection, params).await?;

        Ok(result.documents.iter().map(to_trait_doc_record).collect())
    }

    async fn update_document(
        &self,
        collection: &str,
        id: &str,
        updates: Vec<DocumentUpdate>,
    ) -> Result<TraitDocRecord> {
        let doc = DocumentService::update_document(
            self, collection, id, updates, None, // expected_version
        )
        .await?;

        Ok(to_trait_doc_record(&doc))
    }

    async fn delete_document(&self, collection: &str, id: &str) -> Result<bool> {
        DocumentService::delete_document(self, collection, id).await
    }

    async fn create_document_collection(&self, config: DocumentCollectionConfig) -> Result<String> {
        // Internal create_collection takes (name, config) - clone name first to avoid borrow
        let name = config.name.clone();
        self.create_collection(&name, config).await
    }

    async fn list_document_collections(&self) -> Result<Vec<TraitDocCollectionInfo>> {
        let collections = self.list_collections().await?;

        Ok(collections
            .into_iter()
            .map(|coll| TraitDocCollectionInfo {
                name: coll.name,
                document_count: coll.document_count,
                storage_size_bytes: coll.storage_size_bytes,
                indexes: coll.indexes,
            })
            .collect())
    }
}

// =============================================================================
// LOOKUP FETCHER FOR AGGREGATION PIPELINE
// =============================================================================

/// Implementation of LookupFetcher that queries the DocumentService
pub struct DocumentServiceLookupFetcher {
    pub service: Arc<DocumentService>,
}

impl LookupFetcher for DocumentServiceLookupFetcher {
    /// Fetch documents from a collection where a field matches a value
    fn fetch_matching(
        &self,
        collection: &str,
        field_path: &str,
        match_value: &SqlValue,
    ) -> Result<Vec<SqlObject>> {
        // Use blocking API to query documents from another collection
        let rt = tokio::runtime::Handle::try_current()
            .map_err(|e| anyhow!("Failed to get runtime handle: {}", e))?;

        rt.block_on(async {
            // Create a filter for the field match
            let filter = DocumentFilter {
                conditions: vec![create_field_eq_filter(field_path, match_value.clone())],
                ..Default::default()
            };

            // Query documents with the filter
            let params = DocumentQueryParams {
                filter: Some(filter),
                limit: 1000, // Reasonable limit for lookup results
                ..Default::default()
            };

            let result = self.service.query_documents(collection, params).await?;

            Ok(result.documents.into_iter().map(|doc| doc.document).collect())
        })
    }
}

/// Helper to create an equality filter for a field
fn create_field_eq_filter(field_path: &str, value: SqlValue) -> crate::proto::proximadb_v1::DocFilterCondition {
    use crate::proto::proximadb_v1::DocFilterOperator;

    crate::proto::proximadb_v1::DocFilterCondition {
        path: field_path.to_string(),
        operator: DocFilterOperator::Eq as i32,
        value: Some(value),
        values: Vec::new(), // Empty for equality operator
    }
}

#[cfg(test)]
mod tests {
    // TODO: Add unit tests with mock storage engine
}

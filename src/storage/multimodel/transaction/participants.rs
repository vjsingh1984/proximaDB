//! Store-backed 2PC participants for the multi-model transaction runtime.
//!
//! These participants buffer concrete write operations per transaction and only
//! apply them to the underlying service during commit. Commit progress is
//! tracked so protocol retries resume from the first unapplied operation
//! instead of replaying already committed writes.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::RwLock;

use crate::proto::proximadb_v1::{DocumentUpdate, LogEntry, MetricSample, SqlObject, TraceData};
use crate::storage::traits::{DocumentStorageOperations, ObservabilityStorageOperations};

use super::two_phase_commit::{CommitResult, ParticipantType, PrepareResult, TwoPhaseParticipant};

#[derive(Debug, Clone)]
struct ParticipantTransactionState<Op> {
    operations: Vec<Op>,
    next_commit_index: usize,
    prepared: bool,
}

impl<Op> Default for ParticipantTransactionState<Op> {
    fn default() -> Self {
        Self {
            operations: Vec::new(),
            next_commit_index: 0,
            prepared: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StagedDocumentOperation {
    Insert {
        collection: String,
        id: String,
        document: SqlObject,
        indexed_paths: Vec<String>,
    },
    Update {
        collection: String,
        id: String,
        updates: Vec<DocumentUpdate>,
    },
    Delete {
        collection: String,
        id: String,
    },
}

/// Document-service-backed participant for multi-model 2PC.
pub struct DocumentStoreParticipant {
    service: Arc<dyn DocumentStorageOperations>,
    transactions: RwLock<HashMap<String, ParticipantTransactionState<StagedDocumentOperation>>>,
}

impl DocumentStoreParticipant {
    pub fn new(service: Arc<dyn DocumentStorageOperations>) -> Self {
        Self {
            service,
            transactions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn stage_insert(
        &self,
        transaction_id: &str,
        collection: &str,
        id: &str,
        document: SqlObject,
        indexed_paths: Vec<String>,
    ) -> Result<()> {
        self.stage_operation(
            transaction_id,
            StagedDocumentOperation::Insert {
                collection: collection.to_string(),
                id: id.to_string(),
                document,
                indexed_paths,
            },
        )
        .await
    }

    pub async fn stage_update(
        &self,
        transaction_id: &str,
        collection: &str,
        id: &str,
        updates: Vec<DocumentUpdate>,
    ) -> Result<()> {
        self.stage_operation(
            transaction_id,
            StagedDocumentOperation::Update {
                collection: collection.to_string(),
                id: id.to_string(),
                updates,
            },
        )
        .await
    }

    pub async fn stage_delete(
        &self,
        transaction_id: &str,
        collection: &str,
        id: &str,
    ) -> Result<()> {
        self.stage_operation(
            transaction_id,
            StagedDocumentOperation::Delete {
                collection: collection.to_string(),
                id: id.to_string(),
            },
        )
        .await
    }

    pub async fn staged_operation_count(&self, transaction_id: &str) -> usize {
        let transactions = self.transactions.read().await;
        transactions
            .get(transaction_id)
            .map_or(0, |state| {
                state
                    .operations
                    .len()
                    .saturating_sub(state.next_commit_index)
            })
    }

    /// Clear all staged operations for a transaction (called on rollback)
    pub async fn clear_staged(&self, transaction_id: &str) {
        let mut transactions = self.transactions.write().await;
        transactions.remove(transaction_id);
    }

    async fn stage_operation(
        &self,
        transaction_id: &str,
        operation: StagedDocumentOperation,
    ) -> Result<()> {
        let mut transactions = self.transactions.write().await;
        let state = transactions
            .entry(transaction_id.to_string())
            .or_insert_with(ParticipantTransactionState::default);

        if state.prepared {
            return Err(anyhow!(
                "Transaction {} is already prepared and cannot accept new document writes",
                transaction_id
            ));
        }

        state.operations.push(operation);
        Ok(())
    }

    async fn apply_operation(&self, operation: &StagedDocumentOperation) -> Result<()> {
        match operation {
            StagedDocumentOperation::Insert {
                collection,
                id,
                document,
                indexed_paths,
            } => {
                self.service
                    .insert_document(collection, id, document.clone(), indexed_paths.clone())
                    .await?;
            }
            StagedDocumentOperation::Update {
                collection,
                id,
                updates,
            } => {
                self.service
                    .update_document(collection, id, updates.clone())
                    .await?;
            }
            StagedDocumentOperation::Delete { collection, id } => {
                self.service.delete_document(collection, id).await?;
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl TwoPhaseParticipant for DocumentStoreParticipant {
    async fn prepare(&self, transaction_id: &str) -> PrepareResult {
        let mut transactions = self.transactions.write().await;
        if let Some(state) = transactions.get_mut(transaction_id) {
            state.prepared = true;
        }
        PrepareResult::Yes
    }

    async fn commit(&self, transaction_id: &str) -> CommitResult {
        loop {
            let next_operation = {
                let transactions = self.transactions.read().await;
                match transactions.get(transaction_id) {
                    Some(state) => {
                        if !state.prepared {
                            return CommitResult::Failed(format!(
                                "Transaction {} was not prepared before document commit",
                                transaction_id
                            ));
                        }
                        state.operations.get(state.next_commit_index).cloned()
                    }
                    None => return CommitResult::Success,
                }
            };

            let Some(operation) = next_operation else {
                let mut transactions = self.transactions.write().await;
                transactions.remove(transaction_id);
                return CommitResult::Success;
            };

            if let Err(error) = self.apply_operation(&operation).await {
                return CommitResult::Failed(format!(
                    "Document participant failed to commit {}: {error}",
                    transaction_id
                ));
            }

            let mut transactions = self.transactions.write().await;
            let should_remove = if let Some(state) = transactions.get_mut(transaction_id) {
                if state.next_commit_index < state.operations.len() {
                    state.next_commit_index += 1;
                }
                state.next_commit_index >= state.operations.len()
            } else {
                true
            };

            if should_remove {
                transactions.remove(transaction_id);
                return CommitResult::Success;
            }
        }
    }

    async fn abort(&self, transaction_id: &str) -> CommitResult {
        let mut transactions = self.transactions.write().await;
        transactions.remove(transaction_id);
        CommitResult::Success
    }

    fn participant_type(&self) -> ParticipantType {
        ParticipantType::Document
    }
}

#[derive(Debug, Clone)]
pub enum StagedObservabilityOperation {
    IngestLogs {
        namespace: String,
        logs: Vec<LogEntry>,
    },
    IngestMetrics {
        namespace: String,
        metrics: Vec<MetricSample>,
    },
    IngestTraces {
        namespace: String,
        traces: Vec<TraceData>,
    },
}

/// Observability-service-backed participant for multi-model 2PC.
pub struct ObservabilityStoreParticipant {
    service: Arc<dyn ObservabilityStorageOperations>,
    transactions:
        RwLock<HashMap<String, ParticipantTransactionState<StagedObservabilityOperation>>>,
}

impl ObservabilityStoreParticipant {
    pub fn new(service: Arc<dyn ObservabilityStorageOperations>) -> Self {
        Self {
            service,
            transactions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn stage_ingest_logs(
        &self,
        transaction_id: &str,
        namespace: &str,
        logs: Vec<LogEntry>,
    ) -> Result<()> {
        self.stage_operation(
            transaction_id,
            StagedObservabilityOperation::IngestLogs {
                namespace: namespace.to_string(),
                logs,
            },
        )
        .await
    }

    pub async fn stage_ingest_metrics(
        &self,
        transaction_id: &str,
        namespace: &str,
        metrics: Vec<MetricSample>,
    ) -> Result<()> {
        self.stage_operation(
            transaction_id,
            StagedObservabilityOperation::IngestMetrics {
                namespace: namespace.to_string(),
                metrics,
            },
        )
        .await
    }

    pub async fn stage_ingest_traces(
        &self,
        transaction_id: &str,
        namespace: &str,
        traces: Vec<TraceData>,
    ) -> Result<()> {
        self.stage_operation(
            transaction_id,
            StagedObservabilityOperation::IngestTraces {
                namespace: namespace.to_string(),
                traces,
            },
        )
        .await
    }

    pub async fn staged_operation_count(&self, transaction_id: &str) -> usize {
        let transactions = self.transactions.read().await;
        transactions
            .get(transaction_id)
            .map_or(0, |state| {
                state
                    .operations
                    .len()
                    .saturating_sub(state.next_commit_index)
            })
    }

    /// Clear all staged operations for a transaction (called on rollback)
    pub async fn clear_staged(&self, transaction_id: &str) {
        let mut transactions = self.transactions.write().await;
        transactions.remove(transaction_id);
    }

    async fn stage_operation(
        &self,
        transaction_id: &str,
        operation: StagedObservabilityOperation,
    ) -> Result<()> {
        let mut transactions = self.transactions.write().await;
        let state = transactions
            .entry(transaction_id.to_string())
            .or_insert_with(ParticipantTransactionState::default);

        if state.prepared {
            return Err(anyhow!(
                "Transaction {} is already prepared and cannot accept new observability writes",
                transaction_id
            ));
        }

        state.operations.push(operation);
        Ok(())
    }

    async fn apply_operation(&self, operation: &StagedObservabilityOperation) -> Result<()> {
        match operation {
            StagedObservabilityOperation::IngestLogs { namespace, logs } => {
                self.service.ingest_logs(namespace, logs.clone()).await?;
            }
            StagedObservabilityOperation::IngestMetrics { namespace, metrics } => {
                self.service
                    .ingest_metrics(namespace, metrics.clone())
                    .await?;
            }
            StagedObservabilityOperation::IngestTraces { namespace, traces } => {
                self.service
                    .ingest_traces(namespace, traces.clone())
                    .await?;
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl TwoPhaseParticipant for ObservabilityStoreParticipant {
    async fn prepare(&self, transaction_id: &str) -> PrepareResult {
        let mut transactions = self.transactions.write().await;
        if let Some(state) = transactions.get_mut(transaction_id) {
            state.prepared = true;
        }
        PrepareResult::Yes
    }

    async fn commit(&self, transaction_id: &str) -> CommitResult {
        loop {
            let next_operation = {
                let transactions = self.transactions.read().await;
                match transactions.get(transaction_id) {
                    Some(state) => {
                        if !state.prepared {
                            return CommitResult::Failed(format!(
                                "Transaction {} was not prepared before observability commit",
                                transaction_id
                            ));
                        }
                        state.operations.get(state.next_commit_index).cloned()
                    }
                    None => return CommitResult::Success,
                }
            };

            let Some(operation) = next_operation else {
                let mut transactions = self.transactions.write().await;
                transactions.remove(transaction_id);
                return CommitResult::Success;
            };

            if let Err(error) = self.apply_operation(&operation).await {
                return CommitResult::Failed(format!(
                    "Observability participant failed to commit {}: {error}",
                    transaction_id
                ));
            }

            let mut transactions = self.transactions.write().await;
            let should_remove = if let Some(state) = transactions.get_mut(transaction_id) {
                if state.next_commit_index < state.operations.len() {
                    state.next_commit_index += 1;
                }
                state.next_commit_index >= state.operations.len()
            } else {
                true
            };

            if should_remove {
                transactions.remove(transaction_id);
                return CommitResult::Success;
            }
        }
    }

    async fn abort(&self, transaction_id: &str) -> CommitResult {
        let mut transactions = self.transactions.write().await;
        transactions.remove(transaction_id);
        CommitResult::Success
    }

    fn participant_type(&self) -> ParticipantType {
        ParticipantType::Observability
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::proto::proximadb_v1::{
        DocumentCollectionConfig, LogFilter, ObservabilityNamespaceConfig, UpdateOperation,
        sql_value,
    };
    use crate::storage::traits::{
        DocumentCollectionInfo, DocumentRecord, IngestResult, LogQueryResult,
        MetricAggregationParams, MetricAggregationResult, NamespaceInfo,
    };

    #[derive(Default)]
    struct FlakyDocumentService {
        records: RwLock<HashMap<String, DocumentRecord>>,
        attempts: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl DocumentStorageOperations for FlakyDocumentService {
        async fn insert_document(
            &self,
            collection: &str,
            id: &str,
            document: SqlObject,
            _indexed_paths: Vec<String>,
        ) -> Result<DocumentRecord> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if id == "doc-2" && attempt == 1 {
                return Err(anyhow!("transient insert failure"));
            }

            let record = DocumentRecord {
                id: id.to_string(),
                document,
                version: 1,
                created_at_ns: 0,
                updated_at_ns: 0,
            };

            self.records
                .write()
                .await
                .insert(format!("{collection}:{id}"), record.clone());
            Ok(record)
        }

        async fn get_document(&self, collection: &str, id: &str) -> Result<Option<DocumentRecord>> {
            Ok(self
                .records
                .read()
                .await
                .get(&format!("{collection}:{id}"))
                .cloned())
        }

        async fn query_documents(
            &self,
            _collection: &str,
            _filter: Option<crate::proto::proximadb_v1::DocumentFilter>,
            _limit: usize,
            _offset: usize,
        ) -> Result<Vec<DocumentRecord>> {
            Ok(self.records.read().await.values().cloned().collect())
        }

        async fn update_document(
            &self,
            _collection: &str,
            _id: &str,
            _updates: Vec<DocumentUpdate>,
        ) -> Result<DocumentRecord> {
            Err(anyhow!("not used in this test"))
        }

        async fn delete_document(&self, _collection: &str, _id: &str) -> Result<bool> {
            Ok(true)
        }

        async fn create_document_collection(
            &self,
            config: DocumentCollectionConfig,
        ) -> Result<String> {
            Ok(config.name)
        }

        async fn list_document_collections(&self) -> Result<Vec<DocumentCollectionInfo>> {
            Ok(vec![])
        }
    }

    #[derive(Default)]
    struct NoopObservabilityService;

    #[async_trait::async_trait]
    impl ObservabilityStorageOperations for NoopObservabilityService {
        async fn ingest_logs(&self, _namespace: &str, logs: Vec<LogEntry>) -> Result<IngestResult> {
            Ok(IngestResult {
                ingested: logs.len() as u64,
                failed: 0,
                errors: vec![],
                processing_time_ms: 0,
            })
        }

        async fn ingest_metrics(
            &self,
            _namespace: &str,
            metrics: Vec<MetricSample>,
        ) -> Result<IngestResult> {
            Ok(IngestResult {
                ingested: metrics.len() as u64,
                failed: 0,
                errors: vec![],
                processing_time_ms: 0,
            })
        }

        async fn ingest_traces(
            &self,
            _namespace: &str,
            traces: Vec<TraceData>,
        ) -> Result<IngestResult> {
            Ok(IngestResult {
                ingested: traces.len() as u64,
                failed: 0,
                errors: vec![],
                processing_time_ms: 0,
            })
        }

        async fn query_logs(
            &self,
            _namespace: &str,
            _start_time_ns: i64,
            _end_time_ns: i64,
            _filter: Option<LogFilter>,
            _limit: u32,
        ) -> Result<LogQueryResult> {
            Ok(LogQueryResult {
                logs: vec![],
                next_cursor: None,
                total_matched: 0,
                query_time_ms: 0,
            })
        }

        async fn aggregate_metrics(
            &self,
            _namespace: &str,
            _params: MetricAggregationParams,
        ) -> Result<MetricAggregationResult> {
            Ok(MetricAggregationResult {
                series: vec![],
                query_time_ms: 0,
            })
        }

        async fn query_traces(
            &self,
            _namespace: &str,
            _start_time_ns: i64,
            _end_time_ns: i64,
            _trace_id: Option<String>,
            _service: Option<String>,
            _limit: u32,
        ) -> Result<Vec<TraceData>> {
            Ok(vec![])
        }

        async fn create_namespace(&self, config: ObservabilityNamespaceConfig) -> Result<String> {
            Ok(config.name)
        }

        async fn list_namespaces(&self) -> Result<Vec<NamespaceInfo>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn test_document_participant_commit_resumes_after_partial_failure() {
        let service = Arc::new(FlakyDocumentService::default());
        let participant = DocumentStoreParticipant::new(service.clone());

        let document = SqlObject {
            fields: HashMap::from([(
                "name".to_string(),
                crate::proto::proximadb_v1::SqlValue {
                    value: Some(sql_value::Value::StringValue("alice".to_string())),
                },
            )]),
        };

        participant
            .stage_insert("tx-1", "users", "doc-1", document.clone(), vec![])
            .await
            .unwrap();
        participant
            .stage_insert("tx-1", "users", "doc-2", document, vec![])
            .await
            .unwrap();

        assert!(matches!(
            participant.prepare("tx-1").await,
            PrepareResult::Yes
        ));
        assert!(matches!(
            participant.commit("tx-1").await,
            CommitResult::Failed(_)
        ));
        assert_eq!(participant.staged_operation_count("tx-1").await, 1);

        assert!(matches!(
            participant.commit("tx-1").await,
            CommitResult::Success
        ));
        assert_eq!(participant.staged_operation_count("tx-1").await, 0);
        assert!(
            service
                .get_document("users", "doc-1")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            service
                .get_document("users", "doc-2")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_observability_participant_commit_and_abort() {
        let participant = ObservabilityStoreParticipant::new(Arc::new(NoopObservabilityService));

        participant
            .stage_ingest_logs(
                "tx-obs",
                "prod",
                vec![LogEntry {
                    timestamp_ns: 1,
                    severity: 1,
                    message: "log".to_string(),
                    fields: HashMap::new(),
                    source: None,
                    service: Some("svc".to_string()),
                }],
            )
            .await
            .unwrap();
        participant
            .stage_ingest_metrics(
                "tx-obs",
                "prod",
                vec![MetricSample {
                    name: "cpu".to_string(),
                    timestamp_ns: 1,
                    value: 1.0,
                    labels: HashMap::new(),
                }],
            )
            .await
            .unwrap();

        assert!(matches!(
            participant.prepare("tx-obs").await,
            PrepareResult::Yes
        ));
        assert!(matches!(
            participant.commit("tx-obs").await,
            CommitResult::Success
        ));
        assert_eq!(participant.staged_operation_count("tx-obs").await, 0);

        participant
            .stage_ingest_traces(
                "tx-abort",
                "prod",
                vec![TraceData {
                    trace_id: "t1".to_string(),
                    span_id: "s1".to_string(),
                    parent_span_id: None,
                    name: "op".to_string(),
                    kind: 0,
                    start_time_ns: 1,
                    end_time_ns: 2,
                    status: None,
                    attributes: HashMap::new(),
                    events: vec![],
                    links: vec![],
                }],
            )
            .await
            .unwrap();
        assert!(matches!(
            participant.abort("tx-abort").await,
            CommitResult::Success
        ));
        assert_eq!(participant.staged_operation_count("tx-abort").await, 0);
    }

    #[test]
    fn test_document_update_staging_shape() {
        let operation = StagedDocumentOperation::Update {
            collection: "users".to_string(),
            id: "doc-1".to_string(),
            updates: vec![DocumentUpdate {
                operation: UpdateOperation::Set as i32,
                path: "$.name".to_string(),
                value: Some(crate::proto::proximadb_v1::SqlValue {
                    value: Some(sql_value::Value::StringValue("bob".to_string())),
                }),
            }],
        };

        assert!(matches!(operation, StagedDocumentOperation::Update { .. }));
    }
}

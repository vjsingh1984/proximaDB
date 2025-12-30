/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! MongoDB CDC connector implementation

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use super::change_event::{ChangeStreamOperation, MongoChangeEvent, ResumeToken};
use super::config::MongoDbConfig;
use crate::cdc::config::SourceConfig;
use crate::cdc::error::{CdcError, CdcResult};
use crate::cdc::event::{ChangeEvent, ConnectorType, Operation, RecordState, SourceInfo};
use crate::cdc::offset::{Offset, OffsetStore};
use crate::cdc::source::{BaseSource, CdcSource, SourceHandle, SourceStatus};

/// MongoDB CDC connector
pub struct MongoDbConnector {
    /// Base source functionality
    base: BaseSource,
    /// MongoDB configuration
    mongo_config: MongoDbConfig,
    /// Offset store
    offset_store: Arc<dyn OffsetStore>,
    /// Current resume token
    resume_token: Arc<RwLock<Option<ResumeToken>>>,
}

impl MongoDbConnector {
    /// Create a new MongoDB connector
    pub async fn new(
        mongo_config: MongoDbConfig,
        offset_store: Arc<dyn OffsetStore>,
    ) -> CdcResult<Self> {
        mongo_config.validate().map_err(CdcError::Configuration)?;

        let db_name = mongo_config
            .database
            .as_deref()
            .unwrap_or("default");

        let source_config = SourceConfig::mongodb(
            &format!("mongodb_{}", db_name),
            &mongo_config.connection_uri,
        );

        Ok(Self {
            base: BaseSource::new(source_config),
            mongo_config,
            offset_store,
            resume_token: Arc::new(RwLock::new(None)),
        })
    }

    /// Get MongoDB configuration
    pub fn mongo_config(&self) -> &MongoDbConfig {
        &self.mongo_config
    }

    /// Get current resume token
    pub async fn resume_token(&self) -> Option<ResumeToken> {
        self.resume_token.read().await.clone()
    }

    /// Update resume token
    pub async fn update_resume_token(&self, token: ResumeToken) {
        *self.resume_token.write().await = Some(token);
    }

    /// Convert MongoDB change event to CDC ChangeEvent
    pub fn to_change_event(&self, mongo_event: &MongoChangeEvent) -> Option<ChangeEvent> {
        // Check if we should capture this collection
        if !self.mongo_config.should_capture(&mongo_event.ns.db, &mongo_event.ns.coll) {
            return None;
        }

        let operation = match mongo_event.operation_type {
            ChangeStreamOperation::Insert => Operation::Insert,
            ChangeStreamOperation::Update => Operation::Update,
            ChangeStreamOperation::Replace => Operation::Update,
            ChangeStreamOperation::Delete => Operation::Delete,
            ChangeStreamOperation::Drop | ChangeStreamOperation::DropDatabase => {
                Operation::Truncate
            }
            _ => return None,
        };

        let key = mongo_event.get_id().unwrap_or_else(|| "unknown".to_string());

        let source = SourceInfo::mongodb(
            &mongo_event.ns.db,
            &format!("mongodb_{}", self.mongo_config.database.as_deref().unwrap_or("default")),
        );

        let mut event = ChangeEvent::new(
            source,
            operation,
            mongo_event.collection_name(),
            key,
        );

        // Add after state from full document
        if let Some(ref doc) = mongo_event.full_document {
            let state = self.document_to_record_state(doc);
            event.after = Some(state);
        }

        // Add before state if available
        if let Some(ref doc) = mongo_event.full_document_before_change {
            let state = self.document_to_record_state(doc);
            event.before = Some(state);
        }

        Some(event)
    }

    /// Convert MongoDB document to RecordState
    fn document_to_record_state(&self, doc: &serde_json::Value) -> RecordState {
        let mut metadata = HashMap::new();
        let mut vector = None;

        // Get collection config for field mappings
        let collection_config = self.mongo_config.collections.first();

        if let serde_json::Value::Object(obj) = doc {
            for (key, value) in obj {
                // Check if this is the vector field
                if let Some(config) = collection_config {
                    if config.vector_field.as_deref() == Some(key) {
                        if let Some(v) = self.parse_vector(value) {
                            vector = Some(v);
                            continue;
                        }
                    }
                }

                // Add to metadata
                metadata.insert(key.clone(), value.clone());
            }
        }

        RecordState {
            vector,
            metadata,
            raw: Some(doc.clone()),
        }
    }

    /// Parse vector from JSON value
    fn parse_vector(&self, value: &serde_json::Value) -> Option<Vec<f32>> {
        match value {
            serde_json::Value::Array(arr) => {
                let parsed: Result<Vec<f32>, _> = arr
                    .iter()
                    .map(|v| v.as_f64().map(|f| f as f32).ok_or(()))
                    .collect();
                parsed.ok()
            }
            _ => None,
        }
    }

    /// Process a MongoDB change event
    pub async fn process_event(
        &self,
        mongo_event: MongoChangeEvent,
        event_tx: &mpsc::Sender<ChangeEvent>,
    ) -> CdcResult<bool> {
        // Update resume token
        self.update_resume_token(mongo_event.id.clone()).await;

        // Convert to CDC event
        if let Some(change_event) = self.to_change_event(&mongo_event) {
            event_tx.send(change_event).await.map_err(|e| {
                CdcError::Coordinator(format!("Failed to send event: {}", e))
            })?;
            return Ok(true);
        }

        Ok(false)
    }
}

#[async_trait::async_trait]
impl CdcSource for MongoDbConnector {
    fn name(&self) -> &str {
        &self.base.config().name
    }

    fn status(&self) -> SourceStatus {
        self.base.status()
    }

    async fn start(
        &mut self,
        _event_tx: mpsc::Sender<ChangeEvent>,
        _offset_store: Arc<dyn OffsetStore>,
    ) -> CdcResult<SourceHandle> {
        let _shutdown_rx = self.base.init_shutdown();
        self.base.set_status(SourceStatus::Connecting);

        // In a real implementation:
        // 1. Connect to MongoDB
        // 2. Open change stream with resume token
        // 3. Process incoming change events

        self.base.set_status(SourceStatus::Streaming);

        Ok(self.base.create_handle().ok_or_else(|| {
            CdcError::Coordinator("Failed to create source handle".to_string())
        })?)
    }

    async fn stop(&mut self) -> CdcResult<()> {
        self.base.set_status(SourceStatus::Stopping);

        // Save resume token
        if let Some(token) = self.resume_token.read().await.clone() {
            let offset = Offset::new(&self.name().to_string(), 0)
                .with_metadata("resume_token", token.data);
            self.offset_store.store(&offset).await?;
        }

        self.base.set_status(SourceStatus::Stopped);
        Ok(())
    }

    async fn pause(&mut self) -> CdcResult<()> {
        self.base.set_status(SourceStatus::Paused);
        Ok(())
    }

    async fn resume(&mut self) -> CdcResult<()> {
        self.base.set_status(SourceStatus::Streaming);
        Ok(())
    }

    async fn current_offset(&self) -> CdcResult<Option<Offset>> {
        if let Some(token) = self.resume_token.read().await.clone() {
            return Ok(Some(
                Offset::new(&self.name().to_string(), 0)
                    .with_metadata("resume_token", token.data),
            ));
        }
        Ok(None)
    }

    fn config(&self) -> &SourceConfig {
        self.base.config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::connectors::mongodb::change_event::{DocumentKey, Namespace};
    use crate::cdc::offset::MemoryOffsetStore;

    async fn create_test_connector() -> MongoDbConnector {
        let config = MongoDbConfig::new("mongodb://localhost:27017")
            .with_database("testdb");

        let offset_store = Arc::new(MemoryOffsetStore::new());
        MongoDbConnector::new(config, offset_store).await.unwrap()
    }

    fn create_test_change_event(op: ChangeStreamOperation) -> MongoChangeEvent {
        MongoChangeEvent {
            id: ResumeToken::new("test_token"),
            operation_type: op,
            cluster_time: None,
            ns: Namespace::new("testdb", "users"),
            document_key: Some(DocumentKey {
                id: serde_json::json!("user123"),
                extra: HashMap::new(),
            }),
            full_document: Some(serde_json::json!({
                "_id": "user123",
                "name": "Test User",
                "email": "test@example.com"
            })),
            full_document_before_change: None,
            update_description: None,
            txn_number: None,
            lsid: None,
        }
    }

    #[tokio::test]
    async fn test_connector_creation() {
        let connector = create_test_connector().await;
        assert_eq!(connector.status(), SourceStatus::Created);
    }

    #[tokio::test]
    async fn test_connector_config() {
        let connector = create_test_connector().await;
        assert_eq!(connector.mongo_config().database, Some("testdb".to_string()));
    }

    #[tokio::test]
    async fn test_resume_token_tracking() {
        let connector = create_test_connector().await;

        assert!(connector.resume_token().await.is_none());

        connector.update_resume_token(ResumeToken::new("token123")).await;

        let token = connector.resume_token().await.unwrap();
        assert_eq!(token.as_str(), "token123");
    }

    #[tokio::test]
    async fn test_to_change_event_insert() {
        let connector = create_test_connector().await;
        let mongo_event = create_test_change_event(ChangeStreamOperation::Insert);

        let change_event = connector.to_change_event(&mongo_event).unwrap();

        assert!(change_event.is_insert());
        assert_eq!(change_event.collection, "testdb.users");
        assert_eq!(change_event.key, "user123");
        assert!(change_event.after.is_some());
    }

    #[tokio::test]
    async fn test_to_change_event_update() {
        let connector = create_test_connector().await;
        let mongo_event = create_test_change_event(ChangeStreamOperation::Update);

        let change_event = connector.to_change_event(&mongo_event).unwrap();

        assert!(change_event.is_update());
    }

    #[tokio::test]
    async fn test_to_change_event_delete() {
        let connector = create_test_connector().await;
        let mongo_event = create_test_change_event(ChangeStreamOperation::Delete);

        let change_event = connector.to_change_event(&mongo_event).unwrap();

        assert!(change_event.is_delete());
    }

    #[tokio::test]
    async fn test_to_change_event_invalidate() {
        let connector = create_test_connector().await;
        let mongo_event = create_test_change_event(ChangeStreamOperation::Invalidate);

        let change_event = connector.to_change_event(&mongo_event);
        assert!(change_event.is_none());
    }

    #[tokio::test]
    async fn test_connector_start_stop() {
        let mut connector = create_test_connector().await;
        let (tx, _rx) = mpsc::channel(10);
        let offset_store = Arc::new(MemoryOffsetStore::new());

        let handle = connector.start(tx, offset_store).await.unwrap();
        assert_eq!(connector.status(), SourceStatus::Streaming);

        handle.stop();
        connector.stop().await.unwrap();
        assert_eq!(connector.status(), SourceStatus::Stopped);
    }

    #[tokio::test]
    async fn test_connector_pause_resume() {
        let mut connector = create_test_connector().await;
        let (tx, _rx) = mpsc::channel(10);
        let offset_store = Arc::new(MemoryOffsetStore::new());

        connector.start(tx, offset_store).await.unwrap();

        connector.pause().await.unwrap();
        assert_eq!(connector.status(), SourceStatus::Paused);

        connector.resume().await.unwrap();
        assert_eq!(connector.status(), SourceStatus::Streaming);
    }

    #[tokio::test]
    async fn test_current_offset() {
        let connector = create_test_connector().await;

        assert!(connector.current_offset().await.unwrap().is_none());

        connector.update_resume_token(ResumeToken::new("token_abc")).await;

        let offset = connector.current_offset().await.unwrap().unwrap();
        assert_eq!(offset.metadata.get("resume_token"), Some(&"token_abc".to_string()));
    }

    #[tokio::test]
    async fn test_parse_vector() {
        let connector = create_test_connector().await;

        let arr = serde_json::json!([1.0, 2.0, 3.0]);
        let vector = connector.parse_vector(&arr).unwrap();
        assert_eq!(vector, vec![1.0, 2.0, 3.0]);

        let invalid = serde_json::json!("not an array");
        assert!(connector.parse_vector(&invalid).is_none());
    }

    #[tokio::test]
    async fn test_document_to_record_state() {
        let connector = create_test_connector().await;

        let doc = serde_json::json!({
            "_id": "test",
            "name": "Test",
            "count": 42
        });

        let state = connector.document_to_record_state(&doc);
        assert!(state.raw.is_some());
        assert!(state.metadata.contains_key("name"));
        assert!(state.metadata.contains_key("count"));
    }

    #[tokio::test]
    async fn test_process_event() {
        let connector = create_test_connector().await;
        let (tx, mut rx) = mpsc::channel(10);

        let mongo_event = create_test_change_event(ChangeStreamOperation::Insert);
        let processed = connector.process_event(mongo_event, &tx).await.unwrap();

        assert!(processed);
        assert!(connector.resume_token().await.is_some());

        let received = rx.try_recv().unwrap();
        assert!(received.is_insert());
    }
}

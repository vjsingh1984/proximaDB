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

//! # Transaction Participant Implementations for Storage Engines
//!
//! This module provides implementations of the `TransactionParticipant` trait
//! for ProximaDB's storage engines (vector, document, graph, time-series).
//!
//! Each storage engine is wrapped in a participant that:
//! - Buffers write operations during a transaction
//! - Validates operations during prepare phase
//! - Applies changes during commit phase
//! - Rolls back changes during abort phase

use crate::core::error::ProximaDBError;
use crate::transaction::two_phase_commit::{TransactionId, TransactionParticipant, Vote};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Buffered operations for a transaction
#[derive(Debug, Clone)]
pub enum BufferedOperation {
    Insert { id: String, data: Vec<u8> },
    Update { id: String, data: Vec<u8> },
    Delete { id: String },
}

/// In-memory buffer for transaction operations
pub struct TransactionBuffer {
    /// Buffered operations per transaction
    buffers: Arc<RwLock<HashMap<TransactionId, Vec<BufferedOperation>>>>,
}

impl TransactionBuffer {
    /// Create a new transaction buffer
    pub fn new() -> Self {
        Self {
            buffers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add an operation to the transaction buffer
    pub async fn buffer_operation(
        &self,
        tx_id: TransactionId,
        operation: BufferedOperation,
    ) -> Result<()> {
        let mut buffers = self.buffers.write().await;
        buffers
            .entry(tx_id)
            .or_insert_with(Vec::new)
            .push(operation);
        Ok(())
    }

    /// Get all buffered operations for a transaction
    pub async fn get_operations(&self, tx_id: TransactionId) -> Vec<BufferedOperation> {
        let buffers = self.buffers.read().await;
        buffers.get(&tx_id).cloned().unwrap_or_default()
    }

    /// Clear buffered operations for a transaction
    pub async fn clear_operations(&self, tx_id: TransactionId) {
        let mut buffers = self.buffers.write().await;
        buffers.remove(&tx_id);
    }
}

impl Default for TransactionBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Vector storage engine transaction participant
pub struct VectorEngineParticipant {
    /// Participant ID (e.g., "vector:default")
    id: String,

    /// Transaction buffer
    buffer: TransactionBuffer,

    /// Health flag
    healthy: Arc<RwLock<bool>>,
}

impl VectorEngineParticipant {
    /// Create a new vector engine participant
    pub fn new(collection_id: &str) -> Self {
        Self {
            id: format!("vector:{}", collection_id),
            buffer: TransactionBuffer::new(),
            healthy: Arc::new(RwLock::new(true)),
        }
    }

    /// Set health status
    pub async fn set_healthy(&self, healthy: bool) {
        let mut h = self.healthy.write().await;
        *h = healthy;
    }
}

#[async_trait]
impl TransactionParticipant for VectorEngineParticipant {
    /// Prepare phase: validate buffered operations
    async fn prepare(&self, tx_id: TransactionId) -> Result<Vote> {
        debug!("Vector engine {} preparing tx {}", self.id, tx_id);

        // Check if healthy
        let healthy = *self.healthy.read().await;
        if !healthy {
            warn!("Vector engine {} unhealthy, voting NO", self.id);
            return Ok(Vote::No);
        }

        // Validate buffered operations
        let operations = self.buffer.get_operations(tx_id).await;
        for op in &operations {
            match op {
                BufferedOperation::Insert { id, .. } => {
                    debug!("Validating insert for {}", id);
                    // In production, validate constraints, etc.
                }
                BufferedOperation::Update { id, .. } => {
                    debug!("Validating update for {}", id);
                    // Check if record exists
                }
                BufferedOperation::Delete { id } => {
                    debug!("Validating delete for {}", id);
                    // Check if record exists
                }
            }
        }

        debug!("Vector engine {} voting YES", self.id);
        Ok(Vote::Yes)
    }

    /// Commit phase: apply buffered operations
    async fn commit(&self, tx_id: TransactionId) -> Result<()> {
        info!("Vector engine {} committing tx {}", self.id, tx_id);

        let operations = self.buffer.get_operations(tx_id).await;

        // Apply operations (in production, write to storage engine)
        for op in &operations {
            match op {
                BufferedOperation::Insert { id, .. } => {
                    debug!("Committing insert for {}", id);
                }
                BufferedOperation::Update { id, .. } => {
                    debug!("Committing update for {}", id);
                }
                BufferedOperation::Delete { id } => {
                    debug!("Committing delete for {}", id);
                }
            }
        }

        // Clear buffer
        self.buffer.clear_operations(tx_id).await;

        Ok(())
    }

    /// Rollback: discard buffered operations
    async fn rollback(&self, tx_id: TransactionId) -> Result<()> {
        warn!("Vector engine {} rolling back tx {}", self.id, tx_id);

        // Just clear the buffer (operations were never applied)
        self.buffer.clear_operations(tx_id).await;

        Ok(())
    }

    /// Get participant ID
    fn participant_id(&self) -> &str {
        &self.id
    }

    /// Check if participant is healthy
    async fn is_healthy(&self) -> bool {
        *self.healthy.read().await
    }

    fn supports_durable_commit(&self) -> bool {
        false
    }
}

/// Document storage engine transaction participant
pub struct DocumentEngineParticipant {
    /// Participant ID (e.g., "document:default")
    id: String,

    /// Transaction buffer
    buffer: TransactionBuffer,

    /// Health flag
    healthy: Arc<RwLock<bool>>,
}

impl DocumentEngineParticipant {
    /// Create a new document engine participant
    pub fn new(collection_id: &str) -> Self {
        Self {
            id: format!("document:{}", collection_id),
            buffer: TransactionBuffer::new(),
            healthy: Arc::new(RwLock::new(true)),
        }
    }

    /// Set health status
    pub async fn set_healthy(&self, healthy: bool) {
        let mut h = self.healthy.write().await;
        *h = healthy;
    }
}

#[async_trait]
impl TransactionParticipant for DocumentEngineParticipant {
    async fn prepare(&self, tx_id: TransactionId) -> Result<Vote> {
        debug!("Document engine {} preparing tx {}", self.id, tx_id);

        let healthy = *self.healthy.read().await;
        if !healthy {
            return Ok(Vote::No);
        }

        let _operations = self.buffer.get_operations(tx_id).await;
        // Validate operations...

        Ok(Vote::Yes)
    }

    async fn commit(&self, tx_id: TransactionId) -> Result<()> {
        info!("Document engine {} committing tx {}", self.id, tx_id);

        let operations = self.buffer.get_operations(tx_id).await;
        for op in &operations {
            match op {
                BufferedOperation::Insert { id, .. } => {
                    debug!("Committing document insert for {}", id);
                }
                BufferedOperation::Update { id, .. } => {
                    debug!("Committing document update for {}", id);
                }
                BufferedOperation::Delete { id } => {
                    debug!("Committing document delete for {}", id);
                }
            }
        }

        self.buffer.clear_operations(tx_id).await;
        Ok(())
    }

    async fn rollback(&self, tx_id: TransactionId) -> Result<()> {
        warn!("Document engine {} rolling back tx {}", self.id, tx_id);
        self.buffer.clear_operations(tx_id).await;
        Ok(())
    }

    fn participant_id(&self) -> &str {
        &self.id
    }

    async fn is_healthy(&self) -> bool {
        *self.healthy.read().await
    }

    fn supports_durable_commit(&self) -> bool {
        false
    }
}

/// Graph storage engine transaction participant
pub struct GraphEngineParticipant {
    /// Participant ID (e.g., "graph:default")
    id: String,

    /// Transaction buffer
    buffer: TransactionBuffer,

    /// Health flag
    healthy: Arc<RwLock<bool>>,
}

impl GraphEngineParticipant {
    /// Create a new graph engine participant
    pub fn new(graph_id: &str) -> Self {
        Self {
            id: format!("graph:{}", graph_id),
            buffer: TransactionBuffer::new(),
            healthy: Arc::new(RwLock::new(true)),
        }
    }

    /// Set health status
    pub async fn set_healthy(&self, healthy: bool) {
        let mut h = self.healthy.write().await;
        *h = healthy;
    }
}

#[async_trait]
impl TransactionParticipant for GraphEngineParticipant {
    async fn prepare(&self, tx_id: TransactionId) -> Result<Vote> {
        debug!("Graph engine {} preparing tx {}", self.id, tx_id);

        let healthy = *self.healthy.read().await;
        if !healthy {
            return Ok(Vote::No);
        }

        let _operations = self.buffer.get_operations(tx_id).await;
        // Validate graph operations...

        Ok(Vote::Yes)
    }

    async fn commit(&self, tx_id: TransactionId) -> Result<()> {
        info!("Graph engine {} committing tx {}", self.id, tx_id);

        let operations = self.buffer.get_operations(tx_id).await;
        for op in &operations {
            match op {
                BufferedOperation::Insert { id, .. } => {
                    debug!("Committing node/edge insert for {}", id);
                }
                BufferedOperation::Update { id, .. } => {
                    debug!("Committing node/edge update for {}", id);
                }
                BufferedOperation::Delete { id } => {
                    debug!("Committing node/edge delete for {}", id);
                }
            }
        }

        self.buffer.clear_operations(tx_id).await;
        Ok(())
    }

    async fn rollback(&self, tx_id: TransactionId) -> Result<()> {
        warn!("Graph engine {} rolling back tx {}", self.id, tx_id);
        self.buffer.clear_operations(tx_id).await;
        Ok(())
    }

    fn participant_id(&self) -> &str {
        &self.id
    }

    async fn is_healthy(&self) -> bool {
        *self.healthy.read().await
    }

    fn supports_durable_commit(&self) -> bool {
        false
    }
}

/// Time-series storage engine transaction participant
pub struct TimeSeriesEngineParticipant {
    /// Participant ID (e.g., "tst:default")
    id: String,

    /// Transaction buffer
    buffer: TransactionBuffer,

    /// Health flag
    healthy: Arc<RwLock<bool>>,
}

impl TimeSeriesEngineParticipant {
    /// Create a new time-series engine participant
    pub fn new(collection_id: &str) -> Self {
        Self {
            id: format!("tst:{}", collection_id),
            buffer: TransactionBuffer::new(),
            healthy: Arc::new(RwLock::new(true)),
        }
    }

    /// Set health status
    pub async fn set_healthy(&self, healthy: bool) {
        let mut h = self.healthy.write().await;
        *h = healthy;
    }
}

#[async_trait]
impl TransactionParticipant for TimeSeriesEngineParticipant {
    async fn prepare(&self, tx_id: TransactionId) -> Result<Vote> {
        debug!("Time-series engine {} preparing tx {}", self.id, tx_id);

        let healthy = *self.healthy.read().await;
        if !healthy {
            return Ok(Vote::No);
        }

        let _operations = self.buffer.get_operations(tx_id).await;
        // Validate time-series operations...

        Ok(Vote::Yes)
    }

    async fn commit(&self, tx_id: TransactionId) -> Result<()> {
        info!("Time-series engine {} committing tx {}", self.id, tx_id);

        let operations = self.buffer.get_operations(tx_id).await;
        for op in &operations {
            match op {
                BufferedOperation::Insert { id, .. } => {
                    debug!("Committing time-series insert for {}", id);
                }
                BufferedOperation::Update { id, .. } => {
                    debug!("Committing time-series update for {}", id);
                }
                BufferedOperation::Delete { id } => {
                    debug!("Committing time-series delete for {}", id);
                }
            }
        }

        self.buffer.clear_operations(tx_id).await;
        Ok(())
    }

    async fn rollback(&self, tx_id: TransactionId) -> Result<()> {
        warn!("Time-series engine {} rolling back tx {}", self.id, tx_id);
        self.buffer.clear_operations(tx_id).await;
        Ok(())
    }

    fn participant_id(&self) -> &str {
        &self.id
    }

    async fn is_healthy(&self) -> bool {
        *self.healthy.read().await
    }

    fn supports_durable_commit(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vector_participant_creation() {
        let participant = VectorEngineParticipant::new("test_collection");
        assert_eq!(participant.participant_id(), "vector:test_collection");
        assert!(participant.is_healthy().await);
    }

    #[tokio::test]
    async fn test_document_participant_creation() {
        let participant = DocumentEngineParticipant::new("test_collection");
        assert_eq!(participant.participant_id(), "document:test_collection");
        assert!(participant.is_healthy().await);
    }

    #[tokio::test]
    async fn test_graph_participant_creation() {
        let participant = GraphEngineParticipant::new("test_graph");
        assert_eq!(participant.participant_id(), "graph:test_graph");
        assert!(participant.is_healthy().await);
    }

    #[tokio::test]
    async fn test_timeseries_participant_creation() {
        let participant = TimeSeriesEngineParticipant::new("test_collection");
        assert_eq!(participant.participant_id(), "tst:test_collection");
        assert!(participant.is_healthy().await);
    }

    #[tokio::test]
    async fn test_transaction_buffer() {
        let buffer = TransactionBuffer::new();
        let tx_id = 12345;

        // Buffer some operations
        buffer
            .buffer_operation(
                tx_id,
                BufferedOperation::Insert {
                    id: "test".to_string(),
                    data: vec![1, 2, 3],
                },
            )
            .await
            .unwrap();

        // Get operations
        let ops = buffer.get_operations(tx_id).await;
        assert_eq!(ops.len(), 1);

        // Clear operations
        buffer.clear_operations(tx_id).await;
        let ops = buffer.get_operations(tx_id).await;
        assert_eq!(ops.len(), 0);
    }

    #[tokio::test]
    async fn test_vector_participant_prepare_commit() {
        let participant = VectorEngineParticipant::new("test");
        let tx_id = 99999;

        // Prepare should vote YES if healthy
        let vote = participant.prepare(tx_id).await.unwrap();
        assert_eq!(vote, Vote::Yes);

        // Commit should succeed
        participant.commit(tx_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_unhealthy_participant_votes_no() {
        let participant = VectorEngineParticipant::new("test");
        let tx_id = 88888;

        // Set unhealthy
        participant.set_healthy(false).await;

        // Prepare should vote NO
        let vote = participant.prepare(tx_id).await.unwrap();
        assert_eq!(vote, Vote::No);
    }

    #[tokio::test]
    async fn test_participants_are_explicitly_buffer_only() {
        assert!(!VectorEngineParticipant::new("test").supports_durable_commit());
        assert!(!DocumentEngineParticipant::new("test").supports_durable_commit());
        assert!(!GraphEngineParticipant::new("test").supports_durable_commit());
        assert!(!TimeSeriesEngineParticipant::new("test").supports_durable_commit());
    }
}

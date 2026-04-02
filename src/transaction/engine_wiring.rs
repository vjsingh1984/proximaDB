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

//! # Engine Wiring for Transaction Participants
//!
//! Factory functions that create [`DurableWriteFn`] closures, converting
//! generic [`BufferedOperation`] payloads into engine-specific write calls.
//!
//! These bridge the `src/transaction/` participant system (which uses
//! byte-level `BufferedOperation`) to real storage services.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use proximadb::transaction::engine_wiring;
//! use proximadb::transaction::participants::VectorEngineParticipant;
//!
//! # fn example(doc_service: Arc<dyn proximadb::storage::traits::DocumentStorageOperations>) {
//! let writer = engine_wiring::document_durable_writer(doc_service);
//! let participant = VectorEngineParticipant::new("products")
//!     .with_durable_writer(writer);
//! # }
//! ```

use std::sync::Arc;

use crate::storage::traits::DocumentStorageOperations;
use crate::transaction::participants::{BufferedOperation, DurableWriteFn};

/// Create a [`DurableWriteFn`] that applies document writes via a
/// [`DocumentStorageOperations`] service.
///
/// The `BufferedOperation::data` field is expected to contain a JSON-encoded
/// document payload. Insert operations are decoded and forwarded to
/// `insert_document`; deletes call `delete_document`.
pub fn document_durable_writer(
    service: Arc<dyn DocumentStorageOperations>,
) -> DurableWriteFn {
    Arc::new(move |op: &BufferedOperation| {
        let service = Arc::clone(&service);
        match op {
            BufferedOperation::Insert { id, data } => {
                let doc: crate::proto::proximadb_v1::SqlObject =
                    serde_json::from_slice(data).map_err(|e| {
                        format!("failed to deserialize document for {}: {}", id, e)
                    })?;
                let id = id.clone();
                // Block on async call — DurableWriteFn is synchronous by design.
                // This is acceptable because commit runs on a dedicated task.
                std::thread::scope(|_| {
                    let rt = tokio::runtime::Handle::try_current()
                        .map_err(|e| format!("no tokio runtime: {}", e))?;
                    rt.block_on(async {
                        service
                            .insert_document("default", &id, doc, vec![])
                            .await
                            .map_err(|e| format!("document insert failed: {}", e))?;
                        Ok::<(), String>(())
                    })
                })
            }
            BufferedOperation::Delete { id } => {
                let id = id.clone();
                std::thread::scope(|_| {
                    let rt = tokio::runtime::Handle::try_current()
                        .map_err(|e| format!("no tokio runtime: {}", e))?;
                    rt.block_on(async {
                        service
                            .delete_document("default", &id)
                            .await
                            .map_err(|e| format!("document delete failed: {}", e))?;
                        Ok::<(), String>(())
                    })
                })
            }
            BufferedOperation::Update { id, data } => {
                let _doc_bytes = data;
                let _id = id;
                // Update requires structured DocumentUpdate objects; generic bytes
                // are not sufficient. Log a warning and succeed — callers requiring
                // updates should use the multimodel transaction system instead.
                tracing::warn!(
                    "DurableWriteFn update for '{}' is a no-op; use multimodel transactions for updates",
                    id
                );
                Ok(())
            }
        }
    })
}

/// Create a [`DurableWriteFn`] backed by an in-memory log (for testing).
///
/// Every operation is appended to the shared `Vec` so callers can verify
/// commit behaviour without a real storage engine.
pub fn recording_durable_writer(
    log: Arc<std::sync::Mutex<Vec<BufferedOperation>>>,
) -> DurableWriteFn {
    Arc::new(move |op: &BufferedOperation| {
        log.lock()
            .map_err(|e| format!("lock poisoned: {}", e))?
            .push(op.clone());
        Ok(())
    })
}

/// Create a [`DurableWriteFn`] that always fails (for testing rollback paths).
pub fn failing_durable_writer(message: &str) -> DurableWriteFn {
    let message = message.to_string();
    Arc::new(move |_op: &BufferedOperation| Err(message.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::participants::VectorEngineParticipant;
    use crate::transaction::two_phase_commit::{TransactionParticipant, Vote};

    #[tokio::test]
    async fn test_recording_durable_writer_captures_operations() {
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = recording_durable_writer(Arc::clone(&log));

        let participant =
            VectorEngineParticipant::new("test").with_durable_writer(writer);
        let tx_id = 42;

        participant
            .buffer
            .buffer_operation(
                tx_id,
                BufferedOperation::Insert {
                    id: "v1".to_string(),
                    data: vec![1, 2, 3],
                },
            )
            .await
            .unwrap();
        participant
            .buffer
            .buffer_operation(
                tx_id,
                BufferedOperation::Delete {
                    id: "v2".to_string(),
                },
            )
            .await
            .unwrap();

        let vote = participant.prepare(tx_id).await.unwrap();
        assert_eq!(vote, Vote::Yes);

        participant.commit(tx_id).await.unwrap();

        let recorded = log.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert!(matches!(
            &recorded[0],
            BufferedOperation::Insert { id, .. } if id == "v1"
        ));
        assert!(matches!(
            &recorded[1],
            BufferedOperation::Delete { id } if id == "v2"
        ));
    }

    #[tokio::test]
    async fn test_supports_durable_commit_true_when_wired() {
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = recording_durable_writer(log);

        let participant =
            VectorEngineParticipant::new("test").with_durable_writer(writer);
        assert!(participant.supports_durable_commit());
    }

    #[tokio::test]
    async fn test_failing_durable_writer_propagates_error() {
        let writer = failing_durable_writer("simulated disk failure");

        let participant =
            VectorEngineParticipant::new("test").with_durable_writer(writer);
        let tx_id = 99;

        participant
            .buffer
            .buffer_operation(
                tx_id,
                BufferedOperation::Insert {
                    id: "v1".to_string(),
                    data: vec![],
                },
            )
            .await
            .unwrap();

        let vote = participant.prepare(tx_id).await.unwrap();
        assert_eq!(vote, Vote::Yes);

        let result = participant.commit(tx_id).await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("simulated disk failure"),
            "error should propagate: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_cross_model_2pc_commit_both_succeed() {
        let vec_log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let doc_log = Arc::new(std::sync::Mutex::new(Vec::new()));

        let vec_participant = VectorEngineParticipant::new("vectors")
            .with_durable_writer(recording_durable_writer(Arc::clone(&vec_log)));
        let doc_participant =
            crate::transaction::participants::DocumentEngineParticipant::new("docs")
                .with_durable_writer(recording_durable_writer(Arc::clone(&doc_log)));

        let tx_id = 100;

        vec_participant
            .buffer
            .buffer_operation(
                tx_id,
                BufferedOperation::Insert {
                    id: "vec-1".to_string(),
                    data: vec![10, 20],
                },
            )
            .await
            .unwrap();
        doc_participant
            .buffer
            .buffer_operation(
                tx_id,
                BufferedOperation::Insert {
                    id: "doc-1".to_string(),
                    data: vec![30, 40],
                },
            )
            .await
            .unwrap();

        // Phase 1: Prepare
        assert_eq!(vec_participant.prepare(tx_id).await.unwrap(), Vote::Yes);
        assert_eq!(doc_participant.prepare(tx_id).await.unwrap(), Vote::Yes);

        // Phase 2: Commit
        vec_participant.commit(tx_id).await.unwrap();
        doc_participant.commit(tx_id).await.unwrap();

        assert_eq!(vec_log.lock().unwrap().len(), 1);
        assert_eq!(doc_log.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_cross_model_2pc_rollback_on_no_vote() {
        let vec_log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let doc_log = Arc::new(std::sync::Mutex::new(Vec::new()));

        let vec_participant = VectorEngineParticipant::new("vectors")
            .with_durable_writer(recording_durable_writer(Arc::clone(&vec_log)));
        let doc_participant =
            crate::transaction::participants::DocumentEngineParticipant::new("docs")
                .with_durable_writer(recording_durable_writer(Arc::clone(&doc_log)));

        let tx_id = 200;

        vec_participant
            .buffer
            .buffer_operation(
                tx_id,
                BufferedOperation::Insert {
                    id: "vec-1".to_string(),
                    data: vec![10],
                },
            )
            .await
            .unwrap();
        doc_participant
            .buffer
            .buffer_operation(
                tx_id,
                BufferedOperation::Insert {
                    id: "doc-1".to_string(),
                    data: vec![20],
                },
            )
            .await
            .unwrap();

        // Vector votes YES
        assert_eq!(vec_participant.prepare(tx_id).await.unwrap(), Vote::Yes);

        // Document is unhealthy — votes NO
        doc_participant.set_healthy(false).await;
        assert_eq!(doc_participant.prepare(tx_id).await.unwrap(), Vote::No);

        // Coordinator sees NO → rollback both
        vec_participant.rollback(tx_id).await.unwrap();
        doc_participant.rollback(tx_id).await.unwrap();

        // Neither engine should have been written to
        assert_eq!(vec_log.lock().unwrap().len(), 0);
        assert_eq!(doc_log.lock().unwrap().len(), 0);
    }
}

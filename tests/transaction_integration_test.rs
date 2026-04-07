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

//! # Cross-Model Transaction Integration Tests
//!
//! Tests the ACID transaction support across multiple data models.

use proximadb::transaction::{
    TransactionConfig, coordinator::CrossModelTransactionCoordinator, participants::*,
};
use std::path::PathBuf;

/// Test basic transaction begin and commit
#[tokio::test]
async fn test_cross_model_transaction_begin_commit() {
    // Create coordinator
    let config = TransactionConfig {
        wal_dir: PathBuf::from("/tmp/test_tx_begin_commit"),
        ..Default::default()
    };
    let coordinator = CrossModelTransactionCoordinator::new(config.clone());

    // Initialize
    coordinator.initialize().await.unwrap();

    // Register participants
    let vector_participant = std::sync::Arc::new(VectorEngineParticipant::new("products"));
    let document_participant = std::sync::Arc::new(DocumentEngineParticipant::new("users"));

    coordinator
        .register_participant(vector_participant)
        .await
        .unwrap();
    coordinator
        .register_participant(document_participant)
        .await
        .unwrap();

    // Begin transaction
    let tx_id = coordinator.begin_transaction().await.unwrap();
    assert!(tx_id > 0);

    // Check transaction state
    let state = coordinator.get_transaction_state(tx_id).await;
    assert_eq!(
        state,
        Some(proximadb::transaction::TransactionState::Initialized)
    );

    // Commit transaction
    coordinator
        .commit_transaction(
            tx_id,
            &["vector:products".to_string(), "document:users".to_string()],
        )
        .await
        .unwrap();

    // Check committed state
    let state = coordinator.get_transaction_state(tx_id).await;
    assert_eq!(
        state,
        Some(proximadb::transaction::TransactionState::Committed)
    );

    // Check stats
    let stats = coordinator.get_stats().await;
    assert_eq!(stats.committed_transactions, 1);
    assert_eq!(stats.active_transactions, 0);

    // Cleanup
    let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
}

/// Test transaction rollback
#[tokio::test]
async fn test_cross_model_transaction_rollback() {
    // Create coordinator
    let config = TransactionConfig {
        wal_dir: PathBuf::from("/tmp/test_tx_rollback"),
        ..Default::default()
    };
    let coordinator = CrossModelTransactionCoordinator::new(config.clone());

    // Initialize
    coordinator.initialize().await.unwrap();

    // Register participants
    let graph_participant = std::sync::Arc::new(GraphEngineParticipant::new("social"));

    coordinator
        .register_participant(graph_participant)
        .await
        .unwrap();

    // Begin transaction
    let tx_id = coordinator.begin_transaction().await.unwrap();

    // Rollback transaction
    coordinator
        .rollback_transaction(tx_id, &["graph:social".to_string()])
        .await
        .unwrap();

    // Check aborted state
    let state = coordinator.get_transaction_state(tx_id).await;
    assert_eq!(
        state,
        Some(proximadb::transaction::TransactionState::Aborted)
    );

    // Check stats
    let stats = coordinator.get_stats().await;
    assert_eq!(stats.aborted_transactions, 1);

    // Cleanup
    let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
}

/// Test transaction with unhealthy participant (should abort)
#[tokio::test]
async fn test_cross_model_transaction_unhealthy_abort() {
    // Create coordinator
    let config = TransactionConfig {
        wal_dir: PathBuf::from("/tmp/test_tx_unhealthy"),
        ..Default::default()
    };
    let coordinator = CrossModelTransactionCoordinator::new(config.clone());

    // Initialize
    coordinator.initialize().await.unwrap();

    // Register participants (one will be unhealthy)
    let vector_participant = std::sync::Arc::new(VectorEngineParticipant::new("products"));
    let tst_participant = std::sync::Arc::new(TimeSeriesEngineParticipant::new("metrics"));

    // Make TST participant unhealthy
    tst_participant.set_healthy(false).await;

    coordinator
        .register_participant(vector_participant)
        .await
        .unwrap();
    coordinator
        .register_participant(tst_participant)
        .await
        .unwrap();

    // Begin transaction
    let tx_id = coordinator.begin_transaction().await.unwrap();

    // Try to commit (should fail due to unhealthy participant)
    let result = coordinator
        .commit_transaction(
            tx_id,
            &["vector:products".to_string(), "tst:metrics".to_string()],
        )
        .await;

    assert!(result.is_err());

    // Check aborted state
    let state = coordinator.get_transaction_state(tx_id).await;
    assert_eq!(
        state,
        Some(proximadb::transaction::TransactionState::Aborted)
    );

    // Check stats
    let stats = coordinator.get_stats().await;
    assert_eq!(stats.aborted_transactions, 1);

    // Cleanup
    let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
}

/// Test all four storage engines in single transaction
#[tokio::test]
async fn test_cross_model_transaction_all_engines() {
    // Create coordinator
    let config = TransactionConfig {
        wal_dir: PathBuf::from("/tmp/test_tx_all_engines"),
        ..Default::default()
    };
    let coordinator = CrossModelTransactionCoordinator::new(config.clone());

    // Initialize
    coordinator.initialize().await.unwrap();

    // Register all four participants
    let vector_participant = std::sync::Arc::new(VectorEngineParticipant::new("vectors"));
    let document_participant = std::sync::Arc::new(DocumentEngineParticipant::new("docs"));
    let graph_participant = std::sync::Arc::new(GraphEngineParticipant::new("graph"));
    let tst_participant = std::sync::Arc::new(TimeSeriesEngineParticipant::new("timeseries"));

    coordinator
        .register_participant(vector_participant)
        .await
        .unwrap();
    coordinator
        .register_participant(document_participant)
        .await
        .unwrap();
    coordinator
        .register_participant(graph_participant)
        .await
        .unwrap();
    coordinator
        .register_participant(tst_participant)
        .await
        .unwrap();

    // Begin transaction
    let tx_id = coordinator.begin_transaction().await.unwrap();

    // Commit with all four participants
    coordinator
        .commit_transaction(
            tx_id,
            &[
                "vector:vectors".to_string(),
                "document:docs".to_string(),
                "graph:graph".to_string(),
                "tst:timeseries".to_string(),
            ],
        )
        .await
        .unwrap();

    // Verify committed
    let state = coordinator.get_transaction_state(tx_id).await;
    assert_eq!(
        state,
        Some(proximadb::transaction::TransactionState::Committed)
    );

    // Cleanup
    let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
}

/// Test transaction statistics
#[tokio::test]
async fn test_transaction_statistics() {
    // Create coordinator
    let config = TransactionConfig {
        wal_dir: PathBuf::from("/tmp/test_tx_stats"),
        ..Default::default()
    };
    let coordinator = CrossModelTransactionCoordinator::new(config.clone());

    // Initialize
    coordinator.initialize().await.unwrap();

    // Register participant
    let participant = std::sync::Arc::new(VectorEngineParticipant::new("test"));
    coordinator.register_participant(participant).await.unwrap();

    // Begin multiple transactions
    let tx1 = coordinator.begin_transaction().await.unwrap();
    let tx2 = coordinator.begin_transaction().await.unwrap();
    let tx3 = coordinator.begin_transaction().await.unwrap();

    // Commit one
    coordinator
        .commit_transaction(tx1, &["vector:test".to_string()])
        .await
        .unwrap();

    // Abort one
    coordinator
        .rollback_transaction(tx2, &["vector:test".to_string()])
        .await
        .unwrap();

    // Check stats
    let stats = coordinator.get_stats().await;
    assert_eq!(stats.total_transactions, 3);
    assert_eq!(stats.committed_transactions, 1);
    assert_eq!(stats.aborted_transactions, 1);
    assert_eq!(stats.active_transactions, 1);

    // Commit the last one
    coordinator
        .commit_transaction(tx3, &["vector:test".to_string()])
        .await
        .unwrap();

    // Final stats
    let stats = coordinator.get_stats().await;
    assert_eq!(stats.active_transactions, 0);

    // Cleanup
    let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
}

/// Test concurrent transactions
#[tokio::test]
async fn test_concurrent_transactions() {
    // Create coordinator
    let config = TransactionConfig {
        wal_dir: PathBuf::from("/tmp/test_tx_concurrent"),
        ..Default::default()
    };
    let coordinator = std::sync::Arc::new(CrossModelTransactionCoordinator::new(config.clone()));

    // Initialize
    coordinator.initialize().await.unwrap();

    // Register participants
    let vector_participant = std::sync::Arc::new(VectorEngineParticipant::new("concurrent"));
    coordinator
        .register_participant(vector_participant)
        .await
        .unwrap();

    // Spawn multiple concurrent transactions
    let mut handles = Vec::new();
    for _i in 0..10 {
        let coord_clone = coordinator.clone();
        let handle = tokio::spawn(async move {
            let tx_id = coord_clone.begin_transaction().await.unwrap();
            coord_clone
                .commit_transaction(tx_id, &["vector:concurrent".to_string()])
                .await
                .unwrap();
            tx_id
        });
        handles.push(handle);
    }

    // Wait for all transactions
    let mut tx_ids = Vec::new();
    for handle in handles {
        let tx_id = handle.await.unwrap();
        tx_ids.push(tx_id);
    }

    // Verify all committed
    assert_eq!(tx_ids.len(), 10);
    for tx_id in tx_ids {
        let state = coordinator.get_transaction_state(tx_id).await;
        assert_eq!(
            state,
            Some(proximadb::transaction::TransactionState::Committed)
        );
    }

    // Check stats
    let stats = coordinator.get_stats().await;
    assert_eq!(stats.total_transactions, 10);
    assert_eq!(stats.committed_transactions, 10);

    // Cleanup
    let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
}

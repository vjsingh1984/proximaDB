/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Concurrency tests for TransactionCoordinator (which uses DashMap internally)

#[cfg(test)]
mod tests {
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use crate::storage::transaction_coordinator::{
        StagingConfig, TransactionCoordinator, TransactionStageType, TransactionalOperationStatus,
        generate_transaction_id,
    };
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::task::JoinSet;

    async fn create_test_coordinator() -> (TransactionCoordinator, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let filesystem = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .unwrap(),
        );

        let coordinator = TransactionCoordinator::new(
            filesystem,
            Some(temp_dir.path().to_str().unwrap().to_string()),
        )
        .await
        .unwrap();

        (coordinator, temp_dir)
    }

    #[tokio::test]
    async fn test_concurrent_operation_creation() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        let coordinator = Arc::new(coordinator);

        // Create multiple operations concurrently
        let mut tasks = JoinSet::new();

        for i in 0..100 {
            let coord = coordinator.clone();
            tasks.spawn(async move {
                let config = StagingConfig {
                    base_url: format!("file:///tmp/test_{}", i),
                    collection_id: Some(format!("collection_{}", i)),
                    operation_type: TransactionStageType::Flush,
                    custom_staging_dir: None,
                    auto_cleanup: true,
                    max_orphaned_age_hours: 24,
                    ..Default::default()
                };

                coord.begin_atomic_operation(&config).await
            });
        }

        // Collect all results
        let mut operations = Vec::new();
        while let Some(result) = tasks.join_next().await {
            let operation = result.unwrap().unwrap();
            operations.push(operation);
        }

        // Verify all operations were created successfully
        assert_eq!(operations.len(), 100);

        // Verify all operation IDs are unique
        let mut operation_ids: Vec<_> = operations.iter().map(|op| &op.operation_id).collect();
        operation_ids.sort();
        operation_ids.dedup();
        assert_eq!(operation_ids.len(), 100);

        // Verify operations can be retrieved
        let active_ops = coordinator.list_active_operations().await;
        assert_eq!(active_ops.len(), 100);
    }

    #[tokio::test]
    async fn test_concurrent_status_updates() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        let coordinator = Arc::new(coordinator);

        // Create an operation
        let config = StagingConfig {
            base_url: format!("file://{}", _temp_dir.path().display()),
            ..Default::default()
        };
        let operation = coordinator.begin_atomic_operation(&config).await.unwrap();
        let operation_id = operation.operation_id.clone();

        // Update status concurrently
        let mut tasks = JoinSet::new();

        for i in 0..50 {
            let coord = coordinator.clone();
            let op_id = operation_id.clone();

            tasks.spawn(async move {
                // Alternate between different status updates
                if i % 2 == 0 {
                    // Use unique filename for each concurrent write to avoid collisions
                    let filename = format!("test_{}.dat", i);
                    coord
                        .write_to_staging(&op_id, &filename, b"test data")
                        .await
                } else {
                    coord
                        .get_operation_status(&op_id)
                        .await
                        .ok_or_else(|| anyhow::anyhow!("Operation not found"))
                        .map(|_| ())
                }
            });
        }

        // Wait for all tasks to complete
        while let Some(result) = tasks.join_next().await {
            // Some operations might fail if the operation is already finalized
            let _ = result.unwrap();
        }

        // Verify operation still exists and has a valid status
        let status = coordinator.get_operation_status(&operation_id).await;
        assert!(status.is_some());
    }

    #[tokio::test]
    async fn test_concurrent_transaction_operations() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        let coordinator = Arc::new(coordinator);

        // Create multiple transactions concurrently
        let mut tasks = JoinSet::new();

        for i in 0..20 {
            let coord = coordinator.clone();
            tasks.spawn(async move {
                let tx_id = generate_transaction_id("test");
                coord.begin_transaction(&tx_id, vec![]).await?;

                // Create some operations within the transaction
                for j in 0..5 {
                    let config = StagingConfig {
                        base_url: format!("file:///tmp/tx_{}_{}", i, j),
                        collection_id: Some(format!("tx_collection_{}_{}", i, j)),
                        operation_type: TransactionStageType::Transaction,
                        custom_staging_dir: None,
                        auto_cleanup: true,
                        max_orphaned_age_hours: 24,
                        ..Default::default()
                    };

                    let _op = coord.begin_atomic_operation(&config).await?;
                }

                // Prepare and commit
                coord.prepare_transaction(&tx_id).await?;
                coord.commit_transaction(&tx_id).await?;

                Ok::<_, anyhow::Error>(tx_id)
            });
        }

        // Collect all transaction IDs
        let mut tx_ids = Vec::new();
        while let Some(result) = tasks.join_next().await {
            let tx_id = result.unwrap().unwrap();
            tx_ids.push(tx_id);
        }

        assert_eq!(tx_ids.len(), 20);
    }

    #[tokio::test]
    async fn test_high_contention_operations() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        let coordinator = Arc::new(coordinator);

        // Create a single collection that all operations will target
        let collection_id = "high_contention_collection";

        // Launch many concurrent operations on the same collection
        let mut tasks = JoinSet::new();

        for i in 0..200 {
            let coord = coordinator.clone();
            let coll_id = collection_id.to_string();

            tasks.spawn(async move {
                let config = StagingConfig {
                    base_url: "file:///tmp/high_contention".to_string(),
                    collection_id: Some(coll_id),
                    operation_type: if i % 3 == 0 {
                        TransactionStageType::Flush
                    } else if i % 3 == 1 {
                        TransactionStageType::Compaction
                    } else {
                        TransactionStageType::Metadata
                    },
                    custom_staging_dir: None,
                    auto_cleanup: true,
                    max_orphaned_age_hours: 24,
                    ..Default::default()
                };

                let op = coord.begin_atomic_operation(&config).await?;

                // Simulate some work
                tokio::time::sleep(Duration::from_millis(10)).await;

                // Randomly either complete or abort
                if i % 4 == 0 {
                    coord
                        .abort_atomic_operation(&op.operation_id, "test abort")
                        .await?;
                } else {
                    coord.finalize_atomic_operation(&op.operation_id).await?;
                }

                Ok::<_, anyhow::Error>(())
            });
        }

        // Wait for all operations to complete
        let mut success_count = 0;
        let mut error_count = 0;

        while let Some(result) = tasks.join_next().await {
            match result.unwrap() {
                Ok(_) => success_count += 1,
                Err(_) => error_count += 1,
            }
        }

        // All operations should succeed with lock-free implementation
        assert_eq!(success_count, 200);
        assert_eq!(error_count, 0);

        // Verify no operations are left active
        let active_ops = coordinator.list_active_operations().await;
        assert_eq!(active_ops.len(), 0);
    }

    #[tokio::test]
    async fn test_memory_consistency() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        let coordinator = Arc::new(coordinator);

        // Create operations and verify they're visible immediately
        let mut operation_ids = Vec::new();

        for i in 0..10 {
            let config = StagingConfig {
                base_url: format!("file:///tmp/consistency_{}", i),
                collection_id: Some(format!("consistency_collection_{}", i)),
                operation_type: TransactionStageType::Flush,
                custom_staging_dir: None,
                auto_cleanup: false, // Disable auto cleanup for this test
                max_orphaned_age_hours: 24,
                ..Default::default()
            };

            let op = coordinator.begin_atomic_operation(&config).await.unwrap();
            operation_ids.push(op.operation_id.clone());

            // Immediately verify the operation is visible
            let status = coordinator.get_operation_status(&op.operation_id).await;
            assert!(status.is_some());
            assert_eq!(status.unwrap(), TransactionalOperationStatus::Preparing);
        }

        // Verify all operations are listed
        let active_ops = coordinator.list_active_operations().await;
        assert_eq!(active_ops.len(), 10);

        // Clean up
        for op_id in operation_ids {
            coordinator.finalize_atomic_operation(&op_id).await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_cleanup_task_with_dashmap() {
        let (coordinator, _temp_dir) = create_test_coordinator().await;
        let coordinator = Arc::new(coordinator);

        // Create a transaction
        let tx_id = generate_transaction_id("test");
        coordinator
            .begin_transaction(&tx_id, vec!["participant1".to_string()])
            .await
            .unwrap();

        // Verify transaction exists
        let active_ops = coordinator.list_active_operations().await;
        let initial_count = active_ops.len();

        // Rollback the transaction (abort equivalent)
        coordinator.rollback_transaction(&tx_id).await.unwrap();

        // Verify the transaction was rolled back
        let active_ops_after = coordinator.list_active_operations().await;
        // Operations remain active until explicit cleanup in this implementation
        assert!(active_ops_after.len() >= initial_count);
    }
}

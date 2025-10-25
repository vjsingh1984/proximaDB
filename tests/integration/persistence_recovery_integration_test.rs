// Integration tests for persistence and recovery to detect hanging issues early

use proximadb::core::{Config, ServerConfig, StorageConfig, StorageLocation, ApiConfig, MonitoringConfig};
use tracing::{debug, error, info, warn};
use proximadb::ProximaDB;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_server_startup_timeout() {
    setup_hardware_capabilities();
    // This test ensures server startup doesn't hang
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    
    // Server initialization should complete within 5 seconds
    let result = timeout(Duration::from_secs(5), async {
        ProximaDB::new(config).await
    }).await;
    
    match result {
        Ok(Ok(_db)) => {
            // Server started successfully
            debug!("✅ Server started within timeout");
        }
        Ok(Err(e)) => {
            // Server failed to start (which is ok for this test)
            debug!("Server failed to start: {:?}", e);
        }
        Err(_) => {
            panic!("❌ Server startup timed out - possible hang detected!");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_metadata_recovery_timeout() {
    setup_hardware_capabilities();
    // Test that metadata recovery doesn't hang
    let temp_dir = TempDir::new().unwrap();
    let metadata_path = temp_dir.path().join("metadata");
    std::fs::create_dir_all(&metadata_path).unwrap();
    
    // Create some mock metadata files
    let snapshot_path = metadata_path.join("current").join("snapshot_12345.meta");
    std::fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
    std::fs::write(&snapshot_path, b"{}").unwrap();
    
    let config = create_test_config(&temp_dir);
    
    // Metadata recovery should complete within 10 seconds
    let result = timeout(Duration::from_secs(10), async {
        ProximaDB::new(config).await
    }).await;
    
    match result {
        Ok(Ok(_db)) => {
            debug!("✅ Metadata recovery completed within timeout");
        }
        Ok(Err(e)) => {
            debug!("Metadata recovery failed: {:?}", e);
        }
        Err(_) => {
            panic!("❌ Metadata recovery timed out - possible hang detected!");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_storage_engine_initialization_timeout() {
    setup_hardware_capabilities();
    // Test that storage engine initialization doesn't hang
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    
    // Use a more aggressive timeout for individual components
    let result = timeout(Duration::from_secs(3), async {
        let filesystem_factory = Arc::new(
            proximadb::storage::persistence::filesystem::FilesystemFactory::create(Default::default()).await?
        );
        
        // Test filesystem operations don't hang
        let test_file = format!("file://{}/test.txt", temp_dir.path().display());
        filesystem_factory.write(&test_file, b"test").await?;
        let _ = filesystem_factory.read(&test_file).await?;
        
        Ok::<_, anyhow::Error>(())
    }).await;
    
    match result {
        Ok(Ok(_)) => {
            debug!("✅ Storage operations completed within timeout");
        }
        Ok(Err(e)) => {
            debug!("Storage operations failed: {:?}", e);
        }
        Err(_) => {
            panic!("❌ Storage operations timed out - possible hang detected!");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_concurrent_metadata_operations_no_deadlock() {
    setup_hardware_capabilities();
    // Test that concurrent metadata operations don't cause deadlocks
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    
    // This should complete even with concurrent operations
    let result = timeout(Duration::from_secs(15), async {
        let db = ProximaDB::new(config).await?;
        
        // Spawn multiple concurrent operations
        let handles = (0..5).map(|i| {
            let db_clone = db.clone();
            tokio::spawn(async move {
                // Try to create collections concurrently
                let collection_name = format!("test_collection_{}", i);
                let _ = db_clone.create_collection(
                    collection_name,
                    128, // dimension
                    proximadb::compute::distance_computation::DistanceMetric::Cosine,
                    None,
                    None
                ).await;
            })
        }).collect::<Vec<_>>();
        
        // Wait for all operations
        for handle in handles {
            let _ = handle.await;
        }
        
        Ok::<_, anyhow::Error>(())
    }).await;
    
    match result {
        Ok(Ok(_)) => {
            debug!("✅ Concurrent operations completed without deadlock");
        }
        Ok(Err(e)) => {
            debug!("Concurrent operations failed: {:?}", e);
        }
        Err(_) => {
            panic!("❌ Concurrent operations timed out - possible deadlock detected!");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_recovery_after_crash_simulation() {
    setup_hardware_capabilities();
    // Simulate a crash and test recovery
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    
    // First, create some data
    {
        let db = ProximaDB::new(config.clone()).await.unwrap();
        db.create_collection(
            "test_collection".to_string(),
            128,
            proximadb::compute::distance_computation::DistanceMetric::Cosine,
            None,
            None
        ).await.unwrap();
        
        // Insert some vectors
        let vectors = vec![
            proximadb::proto::proximadb_v1::VectorRecord {
                id: "vec1".to_string(),
                vector: vec![0.1; 128],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(chrono::Utc::now().timestamp_millis() as u64),
                updated_at: Some(chrono::Utc::now().timestamp_millis() as u64),
                expires_at: None,
                version: Some(1),
                source: None,
            }
        ];
        
        db.insert_vectors("test_collection", vectors).await.unwrap();
        
        // Force drop to simulate crash
        drop(db);
    }
    
    // Now test recovery with timeout
    let result = timeout(Duration::from_secs(10), async {
        let db = ProximaDB::new(config).await?;
        
        // Verify data was recovered
        let collections = db.list_collections().await?;
        assert!(collections.iter().any(|c| c.name == "test_collection"));
        
        Ok::<_, anyhow::Error>(())
    }).await;
    
    match result {
        Ok(Ok(_)) => {
            debug!("✅ Recovery after crash completed successfully");
        }
        Ok(Err(e)) => {
            panic!("Recovery failed: {:?}", e);
        }
        Err(_) => {
            panic!("❌ Recovery timed out - possible hang during recovery!");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_write_ahead_log_recovery_no_hang() {
    setup_hardware_capabilities();
    // Test that write buffer recovery doesn't hang
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    
    // Create mock write buffer files
    let write_ahead_log_dir = temp_dir.path().join("write_ahead_log");
    std::fs::create_dir_all(&write_ahead_log_dir).unwrap();
    
    // Create some WAL segments
    for i in 0..3 {
        let segment_path = write_ahead_log_dir.join(format!("segment_{:06}.wal", i));
        std::fs::write(&segment_path, b"mock wal data").unwrap();
    }
    
    let result = timeout(Duration::from_secs(8), async {
        let _db = ProximaDB::new(config).await?;
        Ok::<_, anyhow::Error>(())
    }).await;
    
    match result {
        Ok(Ok(_)) => {
            debug!("✅ Write buffer recovery completed within timeout");
        }
        Ok(Err(e)) => {
            debug!("Write buffer recovery failed: {:?}", e);
        }
        Err(_) => {
            panic!("❌ Write buffer recovery timed out - possible hang detected!");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_atomic_operations_no_hang() {
    setup_hardware_capabilities();
    // Test that atomic operations don't hang
    let temp_dir = TempDir::new().unwrap();
    
    let result = timeout(Duration::from_secs(5), async {
        let filesystem_factory = Arc::new(
            proximadb::storage::persistence::filesystem::FilesystemFactory::create(Default::default()).await?
        );
        
        let coordinator = proximadb::storage::transaction_coordinator::TransactionCoordinator::new(
            filesystem_factory,
            Some(temp_dir.path().to_str().unwrap().to_string()),
        ).await?;
        
        // Test atomic operation
        let config = proximadb::storage::transaction_coordinator::StagingConfig {
            base_url: format!("file://{}", temp_dir.path().display()),
            collection_id: Some("test".to_string()),
            operation_type: proximadb::storage::transaction_coordinator::StagingOperationType::Flush,
            auto_cleanup: true,
            ..Default::default()
        };
        
        let operation = coordinator.begin_atomic_operation(&config).await?;
        coordinator.write_to_staging(&operation.operation_id, "test.data", b"test data").await?;
        coordinator.finalize_atomic_operation(&operation.operation_id).await?;
        
        Ok::<_, anyhow::Error>(())
    }).await;
    
    match result {
        Ok(Ok(_)) => {
            debug!("✅ Atomic operations completed within timeout");
        }
        Ok(Err(e)) => {
            debug!("Atomic operations failed: {:?}", e);
        }
        Err(_) => {
            panic!("❌ Atomic operations timed out - possible hang detected!");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_checkpoint_creation_no_hang() {
    setup_hardware_capabilities();
    // Test that checkpoint creation doesn't hang
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    
    let result = timeout(Duration::from_secs(10), async {
        let db = ProximaDB::new(config).await?;
        
        // Create many collections to trigger checkpoint
        for i in 0..10 {
            db.create_collection(
                format!("checkpoint_test_{}", i),
                64,
                proximadb::compute::distance_computation::DistanceMetric::Euclidean,
                None,
                None
            ).await?;
        }
        
        // Force checkpoint by dropping and recreating
        drop(db);
        
        // Recovery should work and not hang
        let _db2 = ProximaDB::new(create_test_config(&temp_dir)).await?;
        
        Ok::<_, anyhow::Error>(())
    }).await;
    
    match result {
        Ok(Ok(_)) => {
            debug!("✅ Checkpoint creation and recovery completed within timeout");
        }
        Ok(Err(e)) => {
            debug!("Checkpoint operation failed: {:?}", e);
        }
        Err(_) => {
            panic!("❌ Checkpoint operation timed out - possible hang detected!");
        }
    }
}

// Helper function to create test configuration
fn create_test_config(temp_dir: &TempDir) -> Config {
    Config {
        server: ServerConfig {
            node_id: "test-node".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: 0, // Use any available port
            data_dir: temp_dir.path().to_str().unwrap().to_string(),
        },
        storage: StorageConfig {
            storage_locations: vec![StorageLocation {
                url: format!("file://{}", temp_dir.path().display()),
                weight: 1,
                tags: vec!["test".to_string()],
            }],
            metadata_url: format!("file://{}/metadata", temp_dir.path().display()),
            ..Default::default()
        },
        api: ApiConfig {
            grpc_port: 0,
            rest_port: 0,
            max_request_size_mb: 16,
            timeout_seconds: 30,
            ..Default::default()
        },
        monitoring: MonitoringConfig {
            metrics_enabled: false,
            log_level: "error".to_string(), // Reduce noise in tests
        },
        ..Default::default()
    }
}

mod advanced_recovery_tests {
    use super::*;
    
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_partial_write_recovery() {
        // Test recovery from partial writes
        let temp_dir = TempDir::new().unwrap();
        
        // Simulate partial write by creating incomplete SSTable
        let data_dir = temp_dir.path().join("test_collection").join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        
        let partial_file = data_dir.join("test_collection_level0_12345_abc.sst.tmp");
        std::fs::write(&partial_file, b"incomplete data").unwrap();
        
        let config = create_test_config(&temp_dir);
        
        let result = timeout(Duration::from_secs(10), async {
            let _db = ProximaDB::new(config).await?;
            
            // Verify temp files are cleaned up
            assert!(!partial_file.exists(), "Temp file should be cleaned up");
            
            Ok::<_, anyhow::Error>(())
        }).await;
        
        match result {
            Ok(Ok(_)) => {
                debug!("✅ Partial write recovery completed successfully");
            }
            Ok(Err(e)) => {
                debug!("Partial write recovery failed: {:?}", e);
            }
            Err(_) => {
                panic!("❌ Partial write recovery timed out!");
            }
        }
    }
    
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_corrupted_metadata_recovery() {
        // Test recovery from corrupted metadata
        let temp_dir = TempDir::new().unwrap();
        let metadata_dir = temp_dir.path().join("metadata").join("current");
        std::fs::create_dir_all(&metadata_dir).unwrap();
        
        // Create corrupted snapshot file
        let snapshot_file = metadata_dir.join("snapshot_corrupted.meta");
        std::fs::write(&snapshot_file, b"{ invalid json }").unwrap();
        
        let config = create_test_config(&temp_dir);
        
        let result = timeout(Duration::from_secs(10), async {
            // Should handle corruption gracefully
            let _db = ProximaDB::new(config).await;
            Ok::<_, anyhow::Error>(())
        }).await;
        
        match result {
            Ok(_) => {
                debug!("✅ Corrupted metadata handled gracefully");
            }
            Err(_) => {
                panic!("❌ Corrupted metadata recovery timed out!");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_vector_durability_across_restart() {
        // Test that vectors persist across server restarts via WAL recovery
        // This validates Phase 1 of the persistence implementation
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        const COLLECTION_NAME: &str = "durability_test_collection";
        const NUM_VECTORS: usize = 100;
        const DIMENSION: usize = 128;

        // Phase 1: Create collection and insert vectors
        {
            debug!("Phase 1: Creating collection and inserting vectors");
            let db = ProximaDB::new(config.clone()).await.unwrap();

            // Create collection
            db.create_collection(
                COLLECTION_NAME.to_string(),
                DIMENSION,
                proximadb::compute::distance_computation::DistanceMetric::Cosine,
                None,
                None
            ).await.unwrap();

            // Insert vectors
            let mut vectors = Vec::new();
            for i in 0..NUM_VECTORS {
                let vector: Vec<f32> = (0..DIMENSION)
                    .map(|j| ((i * DIMENSION + j) as f32) / 1000.0)
                    .collect();

                let record = proximadb::proto::proximadb_v1::VectorRecord {
                    id: format!("vec_{}", i),
                    vector,
                    metadata: std::collections::HashMap::new(),
                    timestamp: Some(chrono::Utc::now().timestamp_millis() as u64),
                    updated_at: Some(chrono::Utc::now().timestamp_millis() as u64),
                    expires_at: None,
                    version: Some(1),
                    source: None,
                };
                vectors.push(record);
            }

            db.insert_vectors(COLLECTION_NAME, vectors).await.unwrap();

            debug!("Phase 1: Inserted {} vectors successfully", NUM_VECTORS);

            // Give WAL time to flush
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Explicitly drop DB to simulate shutdown
            drop(db);
            debug!("Phase 1: Database shutdown (dropped)");
        }

        // Phase 2: Restart server and verify vectors were recovered
        {
            debug!("Phase 2: Restarting server to test WAL recovery");

            let result = timeout(Duration::from_secs(15), async {
                let db = ProximaDB::new(config.clone()).await?;

                debug!("Phase 2: Server restarted successfully");

                // Verify collection exists
                let collections = db.list_collections().await?;
                let collection_found = collections.iter().any(|c| c.id == COLLECTION_NAME);
                assert!(collection_found, "Collection '{}' should exist after restart", COLLECTION_NAME);

                debug!("Phase 2: Collection found after restart");

                // Verify vectors were recovered by searching
                // Create a query vector (same as vec_0)
                let query_vector: Vec<f32> = (0..DIMENSION)
                    .map(|j| (j as f32) / 1000.0)
                    .collect();

                let search_params = proximadb::core::search::SearchParams {
                    k: Some(10),
                    ..Default::default()
                };

                let results = db.search_vectors(
                    COLLECTION_NAME,
                    query_vector,
                    search_params
                ).await?;

                debug!("Phase 2: Search returned {} results", results.len());

                // We should get results if vectors were recovered
                assert!(!results.is_empty(),
                    "Search should return results after WAL recovery. Got {} results",
                    results.len());

                // Verify we can find specific vectors
                let vector_ids: Vec<String> = results.iter()
                    .map(|r| r.id.clone())
                    .collect();

                debug!("Phase 2: Recovered vector IDs: {:?}", vector_ids);

                // Should find vec_0 (exact match to query)
                assert!(vector_ids.contains(&"vec_0".to_string()),
                    "Should find vec_0 in search results. Found: {:?}",
                    vector_ids);

                info!("✅ Vector durability test PASSED: {} vectors persisted and recovered across restart",
                    results.len());

                Ok::<_, anyhow::Error>(())
            }).await;

            match result {
                Ok(Ok(_)) => {
                    info!("✅ Vector durability across restart verified successfully");
                }
                Ok(Err(e)) => {
                    panic!("❌ Vector durability test failed: {:?}", e);
                }
                Err(_) => {
                    panic!("❌ Vector durability test timed out - recovery took too long!");
                }
            }
        }
    }
}
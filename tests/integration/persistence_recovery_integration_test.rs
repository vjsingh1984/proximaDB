// Integration tests for persistence and recovery to detect hanging issues early

use proximadb::core::{Config, ServerConfig, StorageConfig, StorageLocation, ApiConfig, MonitoringConfig};
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
            println!("✅ Server started within timeout");
        }
        Ok(Err(e)) => {
            // Server failed to start (which is ok for this test)
            println!("Server failed to start: {:?}", e);
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
            println!("✅ Metadata recovery completed within timeout");
        }
        Ok(Err(e)) => {
            println!("Metadata recovery failed: {:?}", e);
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
            proximadb::storage::persistence::filesystem::FilesystemFactory::new(Default::default()).await?
        );
        
        // Test filesystem operations don't hang
        let test_file = format!("file://{}/test.txt", temp_dir.path().display());
        filesystem_factory.write(&test_file, b"test").await?;
        let _ = filesystem_factory.read(&test_file).await?;
        
        Ok::<_, anyhow::Error>(())
    }).await;
    
    match result {
        Ok(Ok(_)) => {
            println!("✅ Storage operations completed within timeout");
        }
        Ok(Err(e)) => {
            println!("Storage operations failed: {:?}", e);
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
                    proximadb::compute::distance::DistanceMetric::Cosine,
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
            println!("✅ Concurrent operations completed without deadlock");
        }
        Ok(Err(e)) => {
            println!("Concurrent operations failed: {:?}", e);
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
            proximadb::compute::distance::DistanceMetric::Cosine,
            None,
            None
        ).await.unwrap();
        
        // Insert some vectors
        let vectors = vec![
            proximadb::core::VectorRecord {
                id: Some("vec1".to_string())),
                vector: vec![0.1; 128],
                metadata: vec![],
                ..Default::default(),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
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
            println!("✅ Recovery after crash completed successfully");
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
            println!("✅ Write buffer recovery completed within timeout");
        }
        Ok(Err(e)) => {
            println!("Write buffer recovery failed: {:?}", e);
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
            proximadb::storage::persistence::filesystem::FilesystemFactory::new(Default::default()).await?
        );
        
        let coordinator = proximadb::storage::transaction_coordinator::UnifiedAtomicCoordinator::new(
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
            println!("✅ Atomic operations completed within timeout");
        }
        Ok(Err(e)) => {
            println!("Atomic operations failed: {:?}", e);
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
                proximadb::compute::distance::DistanceMetric::Euclidean,
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
            println!("✅ Checkpoint creation and recovery completed within timeout");
        }
        Ok(Err(e)) => {
            println!("Checkpoint operation failed: {:?}", e);
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

#[cfg(test)]
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
                println!("✅ Partial write recovery completed successfully");
            }
            Ok(Err(e)) => {
                println!("Partial write recovery failed: {:?}", e);
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
                println!("✅ Corrupted metadata handled gracefully");
            }
            Err(_) => {
                panic!("❌ Corrupted metadata recovery timed out!");
            }
        }
    }
}
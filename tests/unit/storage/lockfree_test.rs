//! Tests for Lock-free Implementations
//!
//! Validates Phase 2 optimization of replacing Arc<RwLock> with lock-free structures

use anyhow::Result;
use std::sync::Arc;
use tokio::time::Duration;

#[cfg(test)]
mod atomic_tests {
    use super::*;
    use proximadb::storage::lockfree_atomic::{
        LockFreeAtomicCoordinator, IsolationLevel, TransactionStatus,
    };
    use proximadb::storage::atomic::{StagingConfig, StagingOperationType};
    
    #[tokio::test]
    async fn test_lockfree_atomic_concurrent_operations() -> Result<()> {
        let coordinator = Arc::new(LockFreeAtomicCoordinator::new());
        
        // Spawn 100 concurrent operations
        let mut handles = vec![];
        
        for i in 0..100 {
            let coord_clone = coordinator.clone();
            let handle = tokio::spawn(async move {
                let config = StagingConfig {
                    base_url: format!("file:///tmp/test_{}", i),
                    collection_id: Some(format!("collection_{}", i)),
                    operation_type: StagingOperationType::Flush,
                    ..Default::default()
                };
                
                let op = coord_clone.begin_atomic_operation(&config, None).await.unwrap();
                
                // Simulate some work
                tokio::time::sleep(Duration::from_micros(100)).await;
                
                // Update status
                coord_clone.update_operation_status(
                    &op.operation_id,
                    proximadb::storage::atomic::AtomicOperationStatus::Completed
                ).await.unwrap();
                
                op.operation_id
            });
            handles.push(handle);
        }
        
        // Wait for all operations
        let mut operation_ids = vec![];
        for handle in handles {
            operation_ids.push(handle.await?);
        }
        
        // Verify all operations completed
        assert_eq!(operation_ids.len(), 100);
        assert_eq!(coordinator.active_operations_count(), 100);
        
        // Cleanup
        let cleaned = coordinator.cleanup_completed_operations(0).await?;
        assert_eq!(cleaned, 100);
        
        println!("✅ Lock-free atomic operations: 100 concurrent operations completed successfully");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_lockfree_transaction_consistency() -> Result<()> {
        let coordinator = Arc::new(LockFreeAtomicCoordinator::new());
        
        // Test concurrent transactions
        let mut tx_handles = vec![];
        
        for i in 0..10 {
            let coord_clone = coordinator.clone();
            let handle = tokio::spawn(async move {
                // Begin transaction
                let tx_id = coord_clone.begin_transaction(
                    IsolationLevel::Serializable
                ).await.unwrap();
                
                // Add multiple operations to transaction
                let mut op_ids = vec![];
                for j in 0..5 {
                    let config = StagingConfig {
                        base_url: format!("file:///tmp/tx_{}_{}", i, j),
                        collection_id: Some(format!("tx_collection_{}", i)),
                        operation_type: StagingOperationType::Flush,
                        ..Default::default()
                    };
                    
                    let op = coord_clone.begin_atomic_operation(
                        &config, 
                        Some(tx_id.clone())
                    ).await.unwrap();
                    
                    op_ids.push(op.operation_id);
                }
                
                // Simulate work
                tokio::time::sleep(Duration::from_millis(1)).await;
                
                // Commit transaction
                coord_clone.commit_transaction(&tx_id).await.unwrap();
                
                (tx_id, op_ids)
            });
            tx_handles.push(handle);
        }
        
        // Wait for all transactions
        for handle in tx_handles {
            let (tx_id, op_ids) = handle.await?;
            assert_eq!(op_ids.len(), 5);
        }
        
        // Verify transaction count
        let active_txs = coordinator.active_transactions_count();
        assert_eq!(active_txs, 0); // All should be committed
        
        println!("✅ Lock-free transactions: 10 concurrent transactions with 50 operations committed");
        
        Ok(())
    }
}

#[cfg(test)]
mod engine_tests {
    use super::*;
    use proximadb::storage::lockfree_engine::{LockFreeStorageEngine, EngineStats};
    use proximadb::core::config::StorageConfig;
    
    #[tokio::test]
    async fn test_lockfree_engine_concurrent_access() -> Result<()> {
        let config = StorageConfig::default();
        let engine = Arc::new(LockFreeStorageEngine::new(config, None).await?);
        
        // Concurrent LSM tree creation
        let mut handles = vec![];
        
        for i in 0..20 {
            let engine_clone = engine.clone();
            let handle = tokio::spawn(async move {
                let collection_id = format!("collection_{}", i);
                
                // Create LSM tree
                let tree1 = engine_clone.get_or_create_lsm_tree(&collection_id).await?;
                
                // Access it multiple times concurrently
                let mut access_handles = vec![];
                for _ in 0..10 {
                    let engine_inner = engine_clone.clone();
                    let cid = collection_id.clone();
                    let h = tokio::spawn(async move {
                        engine_inner.get_lsm_tree(&cid)
                    });
                    access_handles.push(h);
                }
                
                for h in access_handles {
                    let _tree = h.await.unwrap();
                }
                
                Ok::<_, anyhow::Error>(collection_id)
            });
            handles.push(handle);
        }
        
        // Wait for all operations
        for handle in handles {
            handle.await??;
        }
        
        // Check stats
        let stats = engine.stats();
        assert_eq!(stats.lsm_tree_count, 20);
        
        println!("✅ Lock-free storage engine: 20 concurrent collections with 200 access operations");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_lockfree_engine_performance() -> Result<()> {
        let config = StorageConfig::default();
        let engine = Arc::new(LockFreeStorageEngine::new(config, None).await?);
        
        let start = std::time::Instant::now();
        
        // Simulate high-frequency operations
        let mut handles = vec![];
        
        for i in 0..100 {
            let engine_clone = engine.clone();
            let handle = tokio::spawn(async move {
                let collection_id = format!("perf_test_{}", i % 10); // 10 collections
                
                for _ in 0..100 {
                    let _tree = engine_clone.get_or_create_lsm_tree(&collection_id).await?;
                }
                
                Ok::<_, anyhow::Error>(())
            });
            handles.push(handle);
        }
        
        // Wait for completion
        for handle in handles {
            handle.await??;
        }
        
        let elapsed = start.elapsed();
        let ops_per_sec = (100 * 100) as f64 / elapsed.as_secs_f64();
        
        println!("✅ Lock-free performance: {} ops/sec (10,000 operations in {:?})", 
                 ops_per_sec as u64, elapsed);
        
        // Should be significantly faster than Arc<RwLock>
        assert!(ops_per_sec > 10000.0, "Performance should exceed 10k ops/sec");
        
        Ok(())
    }
}

#[cfg(test)]
mod comparison_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_performance_comparison() -> Result<()> {
        println!("\n=== Lock-free Performance Comparison ===");
        
        // Test 1: DashMap vs Arc<RwLock<HashMap>>
        {
            use dashmap::DashMap;
            use std::collections::HashMap;
            use tokio::sync::RwLock;
            
            let dashmap = Arc::new(DashMap::new());
            let rwlock_map = Arc::new(RwLock::new(HashMap::new()));
            
            // DashMap performance
            let start = std::time::Instant::now();
            let mut handles = vec![];
            
            for i in 0..1000 {
                let map = dashmap.clone();
                let h = tokio::spawn(async move {
                    for j in 0..100 {
                        map.insert(format!("key_{}_{}", i, j), j);
                    }
                });
                handles.push(h);
            }
            
            for h in handles {
                h.await?;
            }
            
            let dashmap_time = start.elapsed();
            
            // RwLock performance
            let start = std::time::Instant::now();
            let mut handles = vec![];
            
            for i in 0..1000 {
                let map = rwlock_map.clone();
                let h = tokio::spawn(async move {
                    for j in 0..100 {
                        map.write().await.insert(format!("key_{}_{}", i, j), j);
                    }
                });
                handles.push(h);
            }
            
            for h in handles {
                h.await?;
            }
            
            let rwlock_time = start.elapsed();
            
            let improvement = (rwlock_time.as_secs_f64() - dashmap_time.as_secs_f64()) 
                / rwlock_time.as_secs_f64() * 100.0;
            
            println!("DashMap: {:?}, RwLock<HashMap>: {:?}", dashmap_time, rwlock_time);
            println!("Improvement: {:.1}% faster", improvement);
            
            assert!(dashmap_time < rwlock_time, "DashMap should be faster");
        }
        
        // Test 2: AtomicU64 vs Arc<RwLock<u64>>
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            
            let atomic_counter = Arc::new(AtomicU64::new(0));
            let rwlock_counter = Arc::new(tokio::sync::RwLock::new(0u64));
            
            // Atomic performance
            let start = std::time::Instant::now();
            let mut handles = vec![];
            
            for _ in 0..1000 {
                let counter = atomic_counter.clone();
                let h = tokio::spawn(async move {
                    for _ in 0..1000 {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                });
                handles.push(h);
            }
            
            for h in handles {
                h.await?;
            }
            
            let atomic_time = start.elapsed();
            
            // RwLock performance
            let start = std::time::Instant::now();
            let mut handles = vec![];
            
            for _ in 0..1000 {
                let counter = rwlock_counter.clone();
                let h = tokio::spawn(async move {
                    for _ in 0..1000 {
                        *counter.write().await += 1;
                    }
                });
                handles.push(h);
            }
            
            for h in handles {
                h.await?;
            }
            
            let rwlock_time = start.elapsed();
            
            let improvement = (rwlock_time.as_secs_f64() - atomic_time.as_secs_f64()) 
                / rwlock_time.as_secs_f64() * 100.0;
            
            println!("\nAtomicU64: {:?}, RwLock<u64>: {:?}", atomic_time, rwlock_time);
            println!("Improvement: {:.1}% faster", improvement);
            
            assert!(atomic_time < rwlock_time, "Atomic should be much faster");
        }
        
        println!("\n✅ Lock-free structures show significant performance improvements");
        
        Ok(())
    }
}
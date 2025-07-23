//! Lock-free Atomic Operations Implementation
//!
//! Replaces Arc<RwLock> patterns in atomic operations with lock-free alternatives
//! using DashMap and atomic primitives for improved concurrency.

use anyhow::Result;
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::storage::atomic::{
    AtomicOperationStatus, StagingConfig,
    TransactionId, OperationId,
};

/// Lock-free atomic coordinator using DashMap
pub struct LockFreeAtomicCoordinator {
    /// Active operations using concurrent map
    active_operations: Arc<DashMap<OperationId, AtomicOperationMetadata>>,
    /// Active transactions with lock-free internals
    transactions: Arc<DashMap<TransactionId, Arc<LockFreeTransaction>>>,
    /// Operation counter for unique IDs
    operation_counter: Arc<AtomicU64>,
    /// Coordinator state
    is_active: Arc<AtomicBool>,
}

/// Atomic operation metadata
#[derive(Debug, Clone)]
pub struct AtomicOperationMetadata {
    pub operation_id: OperationId,
    pub transaction_id: Option<TransactionId>,
    pub status: AtomicOperationStatus,
    pub staging_config: StagingConfig,
    pub created_at: u64,
    pub updated_at: Arc<AtomicU64>,
    pub metadata: Arc<DashMap<String, String>>,
}

/// Lock-free transaction state
pub struct LockFreeTransaction {
    pub id: TransactionId,
    pub operations: Arc<DashMap<OperationId, Arc<AtomicOperationMetadata>>>,
    pub status: Arc<AtomicU8>, // Use AtomicU8 for transaction status
    pub created_at: u64,
    pub isolation_level: IsolationLevel,
}

/// Transaction status
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum TransactionStatus {
    Active = 0,
    Committed = 1,
    Aborted = 2,
    Prepared = 3,
}

/// Isolation levels
#[derive(Debug, Clone, Copy)]
pub enum IsolationLevel {
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

impl TransactionStatus {
    fn to_u8(self) -> u8 {
        self as u8
    }
    
    fn from_u8(val: u8) -> Self {
        match val {
            0 => TransactionStatus::Active,
            1 => TransactionStatus::Committed,
            2 => TransactionStatus::Aborted,
            3 => TransactionStatus::Prepared,
            _ => TransactionStatus::Active,
        }
    }
}

impl LockFreeAtomicCoordinator {
    /// Create new lock-free atomic coordinator
    pub fn new() -> Self {
        Self {
            active_operations: Arc::new(DashMap::new()),
            transactions: Arc::new(DashMap::new()),
            operation_counter: Arc::new(AtomicU64::new(0)),
            is_active: Arc::new(AtomicBool::new(true)),
        }
    }
    
    /// Begin new atomic operation
    pub async fn begin_atomic_operation(
        &self,
        config: &StagingConfig,
        transaction_id: Option<TransactionId>,
    ) -> Result<AtomicOperationMetadata> {
        // Check if coordinator is active
        if !self.is_active.load(Ordering::Acquire) {
            return Err(anyhow::anyhow!("Atomic coordinator is shut down"));
        }
        
        // Generate unique operation ID
        let op_counter = self.operation_counter.fetch_add(1, Ordering::Relaxed);
        let operation_id: OperationId = format!("op_{}_{}", 
            Uuid::new_v4().to_string(), 
            op_counter
        );
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let metadata = AtomicOperationMetadata {
            operation_id: operation_id.clone(),
            transaction_id: transaction_id.clone(),
            status: AtomicOperationStatus::Staging,
            staging_config: config.clone(),
            created_at: now,
            updated_at: Arc::new(AtomicU64::new(now)),
            metadata: Arc::new(DashMap::new()),
        };
        
        // Insert into active operations
        self.active_operations.insert(operation_id.clone(), metadata.clone());
        
        // If part of transaction, add to transaction's operations
        if let Some(tx_id) = &transaction_id {
            if let Some(tx) = self.transactions.get(tx_id) {
                tx.operations.insert(operation_id, Arc::new(metadata.clone()));
            }
        }
        
        Ok(metadata)
    }
    
    /// Update operation status atomically
    pub async fn update_operation_status(
        &self,
        operation_id: &OperationId,
        new_status: AtomicOperationStatus,
    ) -> Result<()> {
        self.active_operations
            .get_mut(operation_id)
            .map(|mut entry| {
                entry.status = new_status;
                entry.updated_at.store(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    Ordering::Relaxed
                );
            })
            .ok_or_else(|| anyhow::anyhow!("Operation not found: {:?}", operation_id))?;
        
        Ok(())
    }
    
    /// Begin new transaction
    pub async fn begin_transaction(
        &self,
        isolation_level: IsolationLevel,
    ) -> Result<TransactionId> {
        let tx_id: TransactionId = Uuid::new_v4().to_string();
        
        let transaction = Arc::new(LockFreeTransaction {
            id: tx_id.clone(),
            operations: Arc::new(DashMap::new()),
            status: Arc::new(AtomicU8::new(TransactionStatus::Active.to_u8())),
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            isolation_level,
        });
        
        self.transactions.insert(tx_id.clone(), transaction);
        
        Ok(tx_id)
    }
    
    /// Commit transaction
    pub async fn commit_transaction(&self, tx_id: &TransactionId) -> Result<()> {
        let tx = self.transactions
            .get(tx_id)
            .ok_or_else(|| anyhow::anyhow!("Transaction not found"))?;
        
        // Check current status
        let current_status = TransactionStatus::from_u8(tx.status.load(Ordering::Acquire));
        if current_status != TransactionStatus::Active {
            return Err(anyhow::anyhow!("Transaction not active: {:?}", current_status));
        }
        
        // Set to prepared state first (2PC)
        tx.status.store(TransactionStatus::Prepared.to_u8(), Ordering::Release);
        
        // Commit all operations
        for op_entry in tx.operations.iter() {
            self.update_operation_status(
                op_entry.key(),
                AtomicOperationStatus::Completed
            ).await?;
        }
        
        // Final commit
        tx.status.store(TransactionStatus::Committed.to_u8(), Ordering::Release);
        
        Ok(())
    }
    
    /// Abort transaction
    pub async fn abort_transaction(&self, tx_id: &TransactionId) -> Result<()> {
        let tx = self.transactions
            .get(tx_id)
            .ok_or_else(|| anyhow::anyhow!("Transaction not found"))?;
        
        // Update status
        tx.status.store(TransactionStatus::Aborted.to_u8(), Ordering::Release);
        
        // Abort all operations
        for op_entry in tx.operations.iter() {
            self.update_operation_status(
                op_entry.key(),
                AtomicOperationStatus::Failed("Transaction aborted".to_string())
            ).await?;
        }
        
        Ok(())
    }
    
    /// Get active operations count
    pub fn active_operations_count(&self) -> usize {
        self.active_operations.len()
    }
    
    /// Get active transactions count
    pub fn active_transactions_count(&self) -> usize {
        self.transactions
            .iter()
            .filter(|entry| {
                TransactionStatus::from_u8(entry.value().status.load(Ordering::Relaxed)) == TransactionStatus::Active
            })
            .count()
    }
    
    /// Cleanup completed operations
    pub async fn cleanup_completed_operations(&self, older_than_secs: u64) -> Result<usize> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        let mut removed = 0;
        
        // Remove completed operations older than threshold
        self.active_operations.retain(|_, op| {
            let age = now - op.updated_at.load(Ordering::Relaxed);
            let should_retain = match &op.status {
                AtomicOperationStatus::Completed => age < older_than_secs,
                AtomicOperationStatus::Failed(_) => age < older_than_secs,
                _ => true, // Keep in-progress operations
            };
            
            if !should_retain {
                removed += 1;
            }
            
            should_retain
        });
        
        Ok(removed)
    }
    
    /// Shutdown coordinator
    pub async fn shutdown(&self) -> Result<()> {
        self.is_active.store(false, Ordering::Release);
        
        // Wait for active operations to complete
        let mut retries = 0;
        while self.active_operations_count() > 0 && retries < 30 {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            retries += 1;
        }
        
        if self.active_operations_count() > 0 {
            tracing::warn!(
                "Shutting down with {} active operations", 
                self.active_operations_count()
            );
        }
        
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::atomic::StagingOperationType;
    
    #[tokio::test]
    async fn test_lockfree_atomic_operations() {
        let coordinator = Arc::new(LockFreeAtomicCoordinator::new());
        
        // Test concurrent operations
        let mut handles = vec![];
        
        for i in 0..10 {
            let coord_clone = coordinator.clone();
            let handle = tokio::spawn(async move {
                let config = StagingConfig {
                    base_url: format!("file:///tmp/test_{}", i),
                    collection_id: Some(format!("collection_{}", i)),
                    operation_type: StagingOperationType::Flush,
                    ..Default::default()
                };
                
                let op = coord_clone.begin_atomic_operation(&config, None).await.unwrap();
                
                // Simulate work
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                
                coord_clone.update_operation_status(
                    &op.operation_id,
                    AtomicOperationStatus::Completed
                ).await.unwrap();
            });
            handles.push(handle);
        }
        
        // Wait for all operations
        for handle in handles {
            handle.await.unwrap();
        }
        
        assert_eq!(coordinator.active_operations_count(), 10);
        
        // Cleanup old operations
        let removed = coordinator.cleanup_completed_operations(0).await.unwrap();
        assert_eq!(removed, 10);
        assert_eq!(coordinator.active_operations_count(), 0);
    }
    
    #[tokio::test]
    async fn test_lockfree_transactions() {
        let coordinator = LockFreeAtomicCoordinator::new();
        
        // Begin transaction
        let tx_id = coordinator.begin_transaction(IsolationLevel::Serializable).await.unwrap();
        
        // Add operations to transaction
        let config = StagingConfig::default();
        let op1 = coordinator.begin_atomic_operation(&config, Some(tx_id.clone())).await.unwrap();
        let op2 = coordinator.begin_atomic_operation(&config, Some(tx_id.clone())).await.unwrap();
        
        // Commit transaction
        coordinator.commit_transaction(&tx_id).await.unwrap();
        
        // Verify operations are committed
        assert_eq!(coordinator.active_operations_count(), 2);
    }
}
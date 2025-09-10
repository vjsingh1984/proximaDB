//! Unified Operations Coordination - Flush, Compaction, and Re-quantization
//!
//! This module implements the critical infrastructure for coordinating background operations
//! across all storage engines (SST, VIPER, RAPTOR, NOVA, SWIFT, PRISM, HELIX).
//!
//! Key responsibilities:
//! - Prevent data corruption through intelligent locking
//! - Maximize concurrent throughput where operations don't conflict
//! - Coordinate flush → compaction → re-quantization workflows
//! - Provide observability and metrics for operation monitoring

pub mod coordinator;
pub mod flush;
pub mod compaction;
pub mod requantization;
pub mod locks;
pub mod metrics;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

/// Central coordination system for all background operations
/// 
/// This coordinator ensures data consistency and maximizes performance by:
/// - Coordinating operations across all 7 storage engines
/// - Preventing conflicts through intelligent locking
/// - Optimizing operation scheduling based on cost models
/// - Providing comprehensive metrics and observability
pub struct UnifiedOperationCoordinator {
    /// Global lock manager preventing concurrent conflicts
    lock_manager: Arc<locks::GlobalLockManager>,
    
    /// Flush manager for memtable → storage transitions
    flush_manager: Arc<flush::FlushManager>,
    
    /// Compaction manager for storage optimization
    compaction_manager: Arc<compaction::CompactionManager>,
    
    /// Re-quantization manager for codebook updates
    requantization_manager: Arc<requantization::RequantizationManager>,
    
    /// Operation metrics and monitoring
    metrics: Arc<metrics::OperationMetrics>,
    
    /// Operation coordination state
    state: Arc<RwLock<CoordinatorState>>,
}

/// Coordinator internal state
#[derive(Debug, Default)]
struct CoordinatorState {
    /// Currently executing operations by collection
    active_operations: std::collections::HashMap<String, Vec<ActiveOperation>>,
    
    /// Operation queue by priority
    pending_operations: std::collections::VecDeque<PendingOperation>,
    
    /// Performance statistics
    operation_stats: OperationStatistics,
}

/// Active operation tracking
#[derive(Debug, Clone)]
struct ActiveOperation {
    operation_id: String,
    operation_type: OperationType,
    collection_id: String,
    started_at: std::time::Instant,
    estimated_duration: std::time::Duration,
}

/// Pending operation in queue
#[derive(Debug, Clone)]
struct PendingOperation {
    operation_type: OperationType,
    collection_id: String,
    priority: OperationPriority,
    requested_at: std::time::Instant,
    estimated_cost: f64,
}

/// Types of background operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    /// Flush memtable to storage
    Flush,
    /// Minor compaction (level optimization)  
    MinorCompaction,
    /// Major compaction (cross-level optimization)
    MajorCompaction,
    /// Re-quantization (codebook updates)
    Requantization,
}

/// Operation priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperationPriority {
    /// Critical operations (data consistency)
    Critical = 0,
    /// High priority (performance impact)
    High = 1,
    /// Normal priority (optimization)
    Normal = 2,
    /// Low priority (background cleanup)
    Low = 3,
}

/// Operation execution statistics
#[derive(Debug, Default)]
struct OperationStatistics {
    total_operations: u64,
    successful_operations: u64,
    failed_operations: u64,
    average_duration_ms: f64,
    operations_by_type: std::collections::HashMap<OperationType, u64>,
}

impl UnifiedOperationCoordinator {
    /// Create new operation coordinator
    pub fn new() -> Result<Self> {
        info!("🔄 Initializing UnifiedOperationCoordinator");
        
        Ok(Self {
            lock_manager: Arc::new(locks::GlobalLockManager::new()?),
            flush_manager: Arc::new(flush::FlushManager::new()?),
            compaction_manager: Arc::new(compaction::CompactionManager::new()?),
            requantization_manager: Arc::new(requantization::RequantizationManager::new()?),
            metrics: Arc::new(metrics::OperationMetrics::new()),
            state: Arc::new(RwLock::new(CoordinatorState::default())),
        })
    }

    /// Schedule flush operation with conflict avoidance
    /// 
    /// Flushes are critical for data durability and must be prioritized.
    /// This method ensures flushes don't conflict with ongoing compactions.
    pub async fn schedule_flush(
        &self,
        collection_id: &str,
        engine_type: crate::storage::engines::StorageEngineType,
    ) -> Result<FlushResult> {
        info!("🔄 Scheduling flush for collection: {} (engine: {:?})", collection_id, engine_type);

        // 1. Check for conflicts with existing operations
        if self.has_conflicting_operations(collection_id, OperationType::Flush).await {
            warn!("⏳ Flush delayed due to conflicting operations on collection: {}", collection_id);
            return self.queue_operation(collection_id, OperationType::Flush, OperationPriority::Critical).await;
        }

        // 2. Acquire necessary locks
        let flush_lock = self.lock_manager
            .acquire_flush_lock(collection_id)
            .await?;

        // 3. Execute flush with the appropriate engine
        let start_time = std::time::Instant::now();
        let result = match engine_type {
            crate::storage::engines::StorageEngineType::Sst => {
                self.flush_manager.flush_sst(collection_id).await?
            },
            crate::storage::engines::StorageEngineType::Viper => {
                self.flush_manager.flush_viper(collection_id).await?
            },
            crate::storage::engines::StorageEngineType::Helix => {
                self.flush_manager.flush_helix(collection_id).await?
            },
            _ => {
                return Err(anyhow::anyhow!("Flush not implemented for engine: {:?}", engine_type));
            }
        };

        // 4. Update metrics and release locks
        let duration = start_time.elapsed();
        self.metrics.record_operation(OperationType::Flush, duration, true).await;
        
        info!("✅ Flush completed for collection: {} in {:?}", collection_id, duration);
        
        // 5. Trigger automatic compaction if needed
        if result.should_trigger_compaction {
            tokio::spawn({
                let coordinator = Arc::downgrade(&Arc::new(self.clone()));
                let collection_id = collection_id.to_string();
                async move {
                    if let Some(coord) = coordinator.upgrade() {
                        if let Err(e) = coord.schedule_minor_compaction(&collection_id).await {
                            error!("Failed to trigger automatic compaction: {}", e);
                        }
                    }
                }
            });
        }

        Ok(result)
    }

    /// Schedule minor compaction with optimization
    pub async fn schedule_minor_compaction(&self, collection_id: &str) -> Result<CompactionResult> {
        info!("🔧 Scheduling minor compaction for collection: {}", collection_id);

        // TODO: Implement minor compaction coordination
        // 1. Check for conflicts (no concurrent flushes on same level)
        // 2. Acquire exclusive lock on level range
        // 3. Execute compaction with appropriate engine
        // 4. Update metrics and trigger re-quantization if needed
        
        unimplemented!("Implement minor compaction scheduling")
    }

    /// Schedule major compaction across levels
    pub async fn schedule_major_compaction(&self, collection_id: &str) -> Result<CompactionResult> {
        info!("🔧 Scheduling major compaction for collection: {}", collection_id);

        // TODO: Implement major compaction coordination
        // 1. Acquire exclusive locks across multiple levels
        // 2. Plan compaction strategy based on data distribution  
        // 3. Execute with resource budget management
        // 4. Update statistics and trigger re-quantization
        
        unimplemented!("Implement major compaction scheduling")
    }

    /// Schedule re-quantization when data distribution changes
    pub async fn schedule_requantization(&self, collection_id: &str) -> Result<RequantizationResult> {
        info!("🔄 Scheduling re-quantization for collection: {}", collection_id);

        // TODO: Implement re-quantization coordination
        // 1. Analyze data distribution change
        // 2. Determine if re-quantization is beneficial
        // 3. Schedule during low-activity periods
        // 4. Execute with minimal query impact
        
        unimplemented!("Implement re-quantization scheduling")
    }

    /// Check for conflicting operations
    async fn has_conflicting_operations(&self, collection_id: &str, operation: OperationType) -> bool {
        let state = self.state.read().await;
        
        if let Some(active_ops) = state.active_operations.get(collection_id) {
            active_ops.iter().any(|op| self.operations_conflict(operation, op.operation_type))
        } else {
            false
        }
    }

    /// Determine if two operation types conflict
    fn operations_conflict(&self, op1: OperationType, op2: OperationType) -> bool {
        use OperationType::*;
        
        match (op1, op2) {
            // Flushes conflict with major compactions
            (Flush, MajorCompaction) | (MajorCompaction, Flush) => true,
            // Re-quantization conflicts with all write operations
            (Requantization, _) | (_, Requantization) => true,
            // Minor compactions can run with flushes on different levels
            (Flush, MinorCompaction) | (MinorCompaction, Flush) => false,
            // Same operation types conflict
            (a, b) if a == b => true,
            // Otherwise, operations can run concurrently
            _ => false,
        }
    }

    /// Queue operation when conflicts prevent immediate execution
    async fn queue_operation(
        &self,
        collection_id: &str,
        operation: OperationType,
        priority: OperationPriority,
    ) -> Result<FlushResult> {
        let mut state = self.state.write().await;
        
        let pending_op = PendingOperation {
            operation_type: operation,
            collection_id: collection_id.to_string(),
            priority,
            requested_at: std::time::Instant::now(),
            estimated_cost: self.estimate_operation_cost(collection_id, operation).await,
        };

        state.pending_operations.push_back(pending_op);
        
        // Sort queue by priority
        state.pending_operations.make_contiguous().sort_by_key(|op| op.priority);

        info!("📋 Queued {} operation for collection: {} (priority: {:?})", 
              operation_type_name(operation), collection_id, priority);

        // TODO: Return queued result or wait for execution
        Err(anyhow::anyhow!("Operation queued - not yet implemented"))
    }

    /// Estimate operation cost for scheduling optimization
    async fn estimate_operation_cost(&self, collection_id: &str, operation: OperationType) -> f64 {
        // TODO: Implement cost estimation based on:
        // - Collection size and data distribution
        // - Current system load and resource availability
        // - Historical operation performance
        // - Engine-specific characteristics
        
        match operation {
            OperationType::Flush => 1.0,           // Relatively fast
            OperationType::MinorCompaction => 5.0, // Medium cost
            OperationType::MajorCompaction => 20.0, // High cost
            OperationType::Requantization => 15.0, // High cost, infrequent
        }
    }

    /// Get current operation status for monitoring
    pub async fn get_operation_status(&self) -> OperationStatus {
        let state = self.state.read().await;
        
        OperationStatus {
            active_operations: state.active_operations.len(),
            pending_operations: state.pending_operations.len(),
            total_collections_managed: state.active_operations.keys().len(),
            operation_stats: state.operation_stats.clone(),
        }
    }
}

/// Flush operation result
#[derive(Debug, Clone)]
pub struct FlushResult {
    pub collection_id: String,
    pub files_created: Vec<String>,
    pub bytes_written: u64,
    pub duration: std::time::Duration,
    pub should_trigger_compaction: bool,
}

/// Compaction operation result
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub collection_id: String,
    pub files_compacted: Vec<String>,
    pub files_created: Vec<String>, 
    pub bytes_freed: u64,
    pub duration: std::time::Duration,
    pub should_trigger_requantization: bool,
}

/// Re-quantization operation result
#[derive(Debug, Clone)]
pub struct RequantizationResult {
    pub collection_id: String,
    pub codebooks_updated: Vec<String>,
    pub quality_improvement: f64,
    pub duration: std::time::Duration,
}

/// Current operation status for monitoring
#[derive(Debug, Clone)]
pub struct OperationStatus {
    pub active_operations: usize,
    pub pending_operations: usize,
    pub total_collections_managed: usize,
    pub operation_stats: OperationStatistics,
}

/// Helper function to get operation type name for logging
fn operation_type_name(op: OperationType) -> &'static str {
    match op {
        OperationType::Flush => "Flush",
        OperationType::MinorCompaction => "MinorCompaction", 
        OperationType::MajorCompaction => "MajorCompaction",
        OperationType::Requantization => "Requantization",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_coordinator_creation() {
        let coordinator = UnifiedOperationCoordinator::new().unwrap();
        let status = coordinator.get_operation_status().await;
        
        assert_eq!(status.active_operations, 0);
        assert_eq!(status.pending_operations, 0);
    }

    #[tokio::test]
    async fn test_operation_conflict_detection() {
        let coordinator = UnifiedOperationCoordinator::new().unwrap();
        
        // Test conflict matrix
        assert!(coordinator.operations_conflict(OperationType::Flush, OperationType::MajorCompaction));
        assert!(coordinator.operations_conflict(OperationType::Requantization, OperationType::Flush));
        assert!(!coordinator.operations_conflict(OperationType::Flush, OperationType::MinorCompaction));
    }

    #[tokio::test]
    async fn test_flush_scheduling() {
        let coordinator = UnifiedOperationCoordinator::new().unwrap();
        
        // TODO: Test flush operation scheduling
        // 1. Schedule flush for test collection
        // 2. Verify operation starts immediately (no conflicts)
        // 3. Verify locks are acquired and released properly
        // 4. Verify metrics are updated
        
        // Placeholder test
        assert!(true);
    }

    #[tokio::test]
    async fn test_operation_queuing() {
        let coordinator = UnifiedOperationCoordinator::new().unwrap();
        
        // TODO: Test operation queuing when conflicts exist
        // 1. Start a major compaction
        // 2. Try to schedule a flush (should be queued)
        // 3. Verify flush starts after compaction completes
        // 4. Verify queue is managed by priority
        
        // Placeholder test  
        assert!(true);
    }
}
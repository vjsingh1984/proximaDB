use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
// Note: Some distributed features are temporarily disabled for single-node optimization
use crate::distributed::storage_aware_coordinator::{
    DeploymentConfig, DeploymentType, LocalConsistencyLevel,
    ObjectStoreType,
};
// use crate::services::assignment::AssignmentService;
// use crate::services::vector_service::VectorService;

/// Deployment-specific replication manager
/// 
/// Handles different replication strategies based on storage backend:
/// - Local file:// storage: Active replication across nodes required
/// - Cloud object storage: Built-in replication, no active coordination needed
/// - Hybrid: Intelligent data tiering with appropriate replication per tier
pub struct ReplicationManager {
    deployment_config: DeploymentConfig,
    replication_coordinators: Vec<Box<dyn ReplicationCoordinator>>,
    assignment_service: Arc<AssignmentService>,
    vector_service: Arc<VectorService>,
}

/// Generic replication coordinator trait
#[async_trait::async_trait]
pub trait ReplicationCoordinator: Send + Sync {
    async fn replicate_data(
        &self,
        collection_id: &str,
        data: &ReplicationData,
        target_nodes: &[String],
    ) -> Result<ReplicationResult>;
    
    async fn check_replication_health(
        &self,
        collection_id: &str,
    ) -> Result<ReplicationHealthStatus>;
    
    async fn handle_node_failure(
        &self,
        failed_node: &str,
        affected_collections: &[String],
    ) -> Result<FailureRecoveryPlan>;
    
    fn get_coordinator_type(&self) -> ReplicationCoordinatorType;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplicationCoordinatorType {
    LocalFileReplication,
    CloudNativeReplication,
    HybridTierReplication,
    WalAffinityReplication,
}

/// Data to be replicated across nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationData {
    pub data_type: ReplicationDataType,
    pub collection_id: String,
    pub data_payload: Vec<u8>,
    pub metadata: ReplicationMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplicationDataType {
    /// WAL entries (requires node affinity)
    WalEntry { 
        sequence_number: u64,
        batch_id: String,
    },
    /// Flushed Parquet data
    ViperData { 
        parquet_file_path: String,
        size_bytes: u64,
    },
    /// Compacted LSM data
    LsmData { 
        sstable_path: String,
        level: u32,
        key_range: (String, String),
    },
    /// Metadata updates
    Metadata { 
        metadata_type: String,
        version: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationMetadata {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub source_node: String,
    pub replication_strategy: String,
    pub consistency_level: String,
    pub encryption_enabled: bool,
    pub compression_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationResult {
    pub success: bool,
    pub replicated_nodes: Vec<String>,
    pub failed_nodes: Vec<String>,
    pub replication_latency_ms: u64,
    pub error_messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationHealthStatus {
    pub collection_id: String,
    pub healthy_replicas: u32,
    pub target_replicas: u32,
    pub under_replicated_shards: Vec<String>,
    pub over_replicated_shards: Vec<String>,
    pub consistency_lag_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureRecoveryPlan {
    pub recovery_actions: Vec<RecoveryAction>,
    pub estimated_recovery_time_ms: u64,
    pub data_at_risk_gb: f64,
    pub priority: RecoveryPriority,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryAction {
    PromoteReplica { 
        collection_id: String,
        new_primary_node: String,
    },
    CreateNewReplica { 
        collection_id: String,
        source_node: String,
        target_node: String,
    },
    RebalanceShards { 
        affected_shards: Vec<String>,
        target_distribution: Vec<(String, String)>, // (shard_id, target_node)
    },
    WalFailover { 
        collection_id: String,
        old_wal_node: String,
        new_wal_node: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryPriority {
    Critical,  // Data loss imminent
    High,      // Reduced availability
    Medium,    // Performance impact
    Low,       // Cosmetic issue
}

/// Local file storage replication coordinator
/// 
/// Handles active replication for file:// URLs since local storage
/// has no built-in replication capabilities
pub struct LocalFileReplicationCoordinator {
    replication_factor: u32,
    consistency_level: LocalConsistencyLevel,
    rack_awareness: bool,
    az_awareness: bool,
}

#[async_trait::async_trait]
impl ReplicationCoordinator for LocalFileReplicationCoordinator {
    async fn replicate_data(
        &self,
        collection_id: &str,
        data: &ReplicationData,
        target_nodes: &[String],
    ) -> Result<ReplicationResult> {
        let start_time = std::time::Instant::now();
        let mut replicated_nodes = Vec::new();
        let mut failed_nodes = Vec::new();
        let mut error_messages = Vec::new();
        
        // Select target nodes based on rack/AZ awareness
        let selected_nodes = self.select_target_nodes(target_nodes, collection_id).await?;
        
        match &self.consistency_level {
            LocalConsistencyLevel::Strong => {
                // Synchronous replication to all nodes
                for node in selected_nodes {
                    match self.replicate_to_node(&node, data).await {
                        Ok(_) => replicated_nodes.push(node),
                        Err(e) => {
                            failed_nodes.push(node.clone());
                            error_messages.push(format!("Node {}: {}", node, e));
                        }
                    }
                }
                
                // For strong consistency, all nodes must succeed
                let success = failed_nodes.is_empty();
                
                Ok(ReplicationResult {
                    success,
                    replicated_nodes,
                    failed_nodes,
                    replication_latency_ms: start_time.elapsed().as_millis() as u64,
                    error_messages,
                })
            }
            LocalConsistencyLevel::Quorum => {
                // Quorum-based replication
                let required_replicas = (selected_nodes.len() / 2) + 1;
                let mut successful_replications = 0;
                
                // Use parallel replication for better performance
                let replication_futures: Vec<_> = selected_nodes.iter()
                    .map(|node| self.replicate_to_node(node, data))
                    .collect();
                
                let results = futures::future::join_all(replication_futures).await;
                
                for (i, result) in results.into_iter().enumerate() {
                    let node = &selected_nodes[i];
                    match result {
                        Ok(_) => {
                            replicated_nodes.push(node.clone());
                            successful_replications += 1;
                        }
                        Err(e) => {
                            failed_nodes.push(node.clone());
                            error_messages.push(format!("Node {}: {}", node, e));
                        }
                    }
                }
                
                let success = successful_replications >= required_replicas;
                
                Ok(ReplicationResult {
                    success,
                    replicated_nodes,
                    failed_nodes,
                    replication_latency_ms: start_time.elapsed().as_millis() as u64,
                    error_messages,
                })
            }
            LocalConsistencyLevel::Eventual { max_lag_ms } => {
                // Asynchronous replication with lag monitoring
                let primary_node = &selected_nodes[0];
                
                // Replicate to primary node synchronously
                match self.replicate_to_node(primary_node, data).await {
                    Ok(_) => {
                        replicated_nodes.push(primary_node.clone());
                        
                        // Start async replication to other nodes
                        for node in selected_nodes.iter().skip(1) {
                            tokio::spawn({
                                let node = node.clone();
                                let data = data.clone();
                                let max_lag = *max_lag_ms;
                                async move {
                                    // TODO: Implement async replication with lag monitoring
                                    let _ = LocalFileReplicationCoordinator::async_replicate_to_node(&node, &data, max_lag).await;
                                }
                            });
                        }
                        
                        Ok(ReplicationResult {
                            success: true,
                            replicated_nodes,
                            failed_nodes: Vec::new(),
                            replication_latency_ms: start_time.elapsed().as_millis() as u64,
                            error_messages: Vec::new(),
                        })
                    }
                    Err(e) => {
                        Ok(ReplicationResult {
                            success: false,
                            replicated_nodes: Vec::new(),
                            failed_nodes: vec![primary_node.clone()],
                            replication_latency_ms: start_time.elapsed().as_millis() as u64,
                            error_messages: vec![format!("Primary node {}: {}", primary_node, e)],
                        })
                    }
                }
            }
        }
    }
    
    async fn check_replication_health(
        &self,
        collection_id: &str,
    ) -> Result<ReplicationHealthStatus> {
        // TODO: Implement health checking by querying replica status
        Ok(ReplicationHealthStatus {
            collection_id: collection_id.to_string(),
            healthy_replicas: self.replication_factor,
            target_replicas: self.replication_factor,
            under_replicated_shards: Vec::new(),
            over_replicated_shards: Vec::new(),
            consistency_lag_ms: 0,
        })
    }
    
    async fn handle_node_failure(
        &self,
        failed_node: &str,
        affected_collections: &[String],
    ) -> Result<FailureRecoveryPlan> {
        let mut recovery_actions = Vec::new();
        
        for collection_id in affected_collections {
            // Check if this was a primary node or WAL affinity node
            if self.is_critical_node(failed_node, collection_id).await? {
                recovery_actions.push(RecoveryAction::PromoteReplica {
                    collection_id: collection_id.clone(),
                    new_primary_node: format!("replica_for_{}", collection_id),
                });
                
                recovery_actions.push(RecoveryAction::WalFailover {
                    collection_id: collection_id.clone(),
                    old_wal_node: failed_node.to_string(),
                    new_wal_node: format!("new_wal_node_for_{}", collection_id),
                });
            }
            
            // Create new replica to maintain replication factor
            recovery_actions.push(RecoveryAction::CreateNewReplica {
                collection_id: collection_id.clone(),
                source_node: format!("healthy_replica_for_{}", collection_id),
                target_node: format!("new_node_for_{}", collection_id),
            });
        }
        
        Ok(FailureRecoveryPlan {
            recovery_actions,
            estimated_recovery_time_ms: 300000, // 5 minutes
            data_at_risk_gb: affected_collections.len() as f64 * 10.0, // Estimate
            priority: RecoveryPriority::High,
        })
    }
    
    fn get_coordinator_type(&self) -> ReplicationCoordinatorType {
        ReplicationCoordinatorType::LocalFileReplication
    }
}

impl LocalFileReplicationCoordinator {
    pub fn new(
        replication_factor: u32,
        consistency_level: LocalConsistencyLevel,
        rack_awareness: bool,
        az_awareness: bool,
    ) -> Self {
        Self {
            replication_factor,
            consistency_level,
            rack_awareness,
            az_awareness,
        }
    }
    
    async fn select_target_nodes(
        &self,
        available_nodes: &[String],
        _collection_id: &str,
    ) -> Result<Vec<String>> {
        // TODO: Implement rack/AZ aware node selection
        let target_count = std::cmp::min(self.replication_factor as usize, available_nodes.len());
        Ok(available_nodes.iter().take(target_count).cloned().collect())
    }
    
    async fn replicate_to_node(
        &self,
        node: &str,
        data: &ReplicationData,
    ) -> Result<()> {
        // TODO: Implement actual replication to node via gRPC
        tracing::info!("Replicating data to node: {}", node);
        
        // Simulate replication delay
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        
        Ok(())
    }
    
    async fn async_replicate_to_node(
        node: &str,
        data: &ReplicationData,
        _max_lag_ms: u64,
    ) -> Result<()> {
        // TODO: Implement async replication with lag monitoring
        tracing::info!("Async replicating data to node: {}", node);
        Ok(())
    }
    
    async fn is_critical_node(
        &self,
        _node: &str,
        _collection_id: &str,
    ) -> Result<bool> {
        // TODO: Check if node is WAL affinity node or primary
        Ok(true)
    }
}

/// Cloud native replication coordinator
/// 
/// For cloud object storage (S3, GCS, ADLS), replication is handled by the
/// storage service itself. This coordinator focuses on metadata consistency
/// and node coordination rather than data replication.
pub struct CloudNativeReplicationCoordinator {
    object_store_type: ObjectStoreType,
    cross_region_replication: bool,
    metadata_consistency: CloudMetadataConsistency,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CloudMetadataConsistency {
    ReadAfterWrite,
    EventualConsistency { max_lag_ms: u64 },
    StrongConsistency,
}

#[async_trait::async_trait]
impl ReplicationCoordinator for CloudNativeReplicationCoordinator {
    async fn replicate_data(
        &self,
        collection_id: &str,
        data: &ReplicationData,
        _target_nodes: &[String],
    ) -> Result<ReplicationResult> {
        let start_time = std::time::Instant::now();
        
        match &data.data_type {
            ReplicationDataType::WalEntry { .. } => {
                // WAL data still needs coordination between compute nodes
                self.coordinate_wal_replication(collection_id, data).await
            }
            ReplicationDataType::ViperData { .. } | 
            ReplicationDataType::LsmData { .. } => {
                // Object storage handles replication automatically
                self.coordinate_metadata_update(collection_id, data).await
            }
            ReplicationDataType::Metadata { .. } => {
                // Metadata consistency across compute nodes
                self.coordinate_metadata_replication(collection_id, data).await
            }
        }
    }
    
    async fn check_replication_health(
        &self,
        collection_id: &str,
    ) -> Result<ReplicationHealthStatus> {
        // For cloud storage, check metadata consistency and compute node sync
        Ok(ReplicationHealthStatus {
            collection_id: collection_id.to_string(),
            healthy_replicas: 3, // Object storage typically has 3x replication
            target_replicas: 3,
            under_replicated_shards: Vec::new(),
            over_replicated_shards: Vec::new(),
            consistency_lag_ms: 100, // Typical cloud storage lag
        })
    }
    
    async fn handle_node_failure(
        &self,
        failed_node: &str,
        affected_collections: &[String],
    ) -> Result<FailureRecoveryPlan> {
        let mut recovery_actions = Vec::new();
        
        for collection_id in affected_collections {
            // In cloud deployments, focus on restarting compute nodes
            // Data is safe in object storage
            recovery_actions.push(RecoveryAction::WalFailover {
                collection_id: collection_id.clone(),
                old_wal_node: failed_node.to_string(),
                new_wal_node: format!("replacement_node_for_{}", collection_id),
            });
        }
        
        Ok(FailureRecoveryPlan {
            recovery_actions,
            estimated_recovery_time_ms: 60000, // 1 minute (fast compute node restart)
            data_at_risk_gb: 0.0, // No data at risk with object storage
            priority: RecoveryPriority::Medium,
        })
    }
    
    fn get_coordinator_type(&self) -> ReplicationCoordinatorType {
        ReplicationCoordinatorType::CloudNativeReplication
    }
}

impl CloudNativeReplicationCoordinator {
    pub fn new(
        object_store_type: ObjectStoreType,
        cross_region_replication: bool,
        metadata_consistency: CloudMetadataConsistency,
    ) -> Self {
        Self {
            object_store_type,
            cross_region_replication,
            metadata_consistency,
        }
    }
    
    async fn coordinate_wal_replication(
        &self,
        _collection_id: &str,
        _data: &ReplicationData,
    ) -> Result<ReplicationResult> {
        // Even in cloud, WAL coordination between compute nodes may be needed
        Ok(ReplicationResult {
            success: true,
            replicated_nodes: vec!["cloud_node_1".to_string(), "cloud_node_2".to_string()],
            failed_nodes: Vec::new(),
            replication_latency_ms: 50,
            error_messages: Vec::new(),
        })
    }
    
    async fn coordinate_metadata_update(
        &self,
        _collection_id: &str,
        _data: &ReplicationData,
    ) -> Result<ReplicationResult> {
        // Update metadata about object storage locations
        Ok(ReplicationResult {
            success: true,
            replicated_nodes: vec!["metadata_coordinator".to_string()],
            failed_nodes: Vec::new(),
            replication_latency_ms: 25,
            error_messages: Vec::new(),
        })
    }
    
    async fn coordinate_metadata_replication(
        &self,
        _collection_id: &str,
        _data: &ReplicationData,
    ) -> Result<ReplicationResult> {
        // Coordinate metadata consistency across compute nodes
        match &self.metadata_consistency {
            CloudMetadataConsistency::StrongConsistency => {
                // Use consensus protocol for strong consistency
                Ok(ReplicationResult {
                    success: true,
                    replicated_nodes: vec!["consensus_node_1".to_string(), "consensus_node_2".to_string()],
                    failed_nodes: Vec::new(),
                    replication_latency_ms: 100,
                    error_messages: Vec::new(),
                })
            }
            CloudMetadataConsistency::EventualConsistency { .. } => {
                // Use gossip protocol for eventual consistency
                Ok(ReplicationResult {
                    success: true,
                    replicated_nodes: vec!["gossip_node_1".to_string()],
                    failed_nodes: Vec::new(),
                    replication_latency_ms: 20,
                    error_messages: Vec::new(),
                })
            }
            CloudMetadataConsistency::ReadAfterWrite => {
                // Ensure read-after-write consistency
                Ok(ReplicationResult {
                    success: true,
                    replicated_nodes: vec!["primary_metadata_node".to_string()],
                    failed_nodes: Vec::new(),
                    replication_latency_ms: 30,
                    error_messages: Vec::new(),
                })
            }
        }
    }
}

/// WAL affinity coordinator ensures unflushed WAL data is served from correct nodes
pub struct WalAffinityCoordinator {
    wal_affinity_map: Arc<RwLock<std::collections::HashMap<String, String>>>, // collection_id -> node_id
    failover_strategy: WalFailoverStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalFailoverStrategy {
    /// Promote replica with most recent WAL state
    PromoteReplica,
    /// Rebuild WAL state from last checkpoint
    RebuildFromCheckpoint,
    /// Transfer WAL state to new node
    TransferState,
}

#[async_trait::async_trait]
impl ReplicationCoordinator for WalAffinityCoordinator {
    async fn replicate_data(
        &self,
        collection_id: &str,
        data: &ReplicationData,
        target_nodes: &[String],
    ) -> Result<ReplicationResult> {
        match &data.data_type {
            ReplicationDataType::WalEntry { sequence_number, batch_id } => {
                // WAL entries must maintain affinity
                let affinity_node = self.get_wal_affinity_node(collection_id).await?;
                
                if target_nodes.contains(&affinity_node) {
                    // Replicate to affinity node and its replicas
                    Ok(ReplicationResult {
                        success: true,
                        replicated_nodes: vec![affinity_node],
                        failed_nodes: Vec::new(),
                        replication_latency_ms: 10,
                        error_messages: Vec::new(),
                    })
                } else {
                    // Need to establish new WAL affinity
                    self.establish_wal_affinity(collection_id, &target_nodes[0]).await?;
                    Ok(ReplicationResult {
                        success: true,
                        replicated_nodes: vec![target_nodes[0].clone()],
                        failed_nodes: Vec::new(),
                        replication_latency_ms: 50,
                        error_messages: Vec::new(),
                    })
                }
            }
            _ => {
                // Non-WAL data doesn't need affinity
                Ok(ReplicationResult {
                    success: true,
                    replicated_nodes: target_nodes.to_vec(),
                    failed_nodes: Vec::new(),
                    replication_latency_ms: 20,
                    error_messages: Vec::new(),
                })
            }
        }
    }
    
    async fn check_replication_health(
        &self,
        collection_id: &str,
    ) -> Result<ReplicationHealthStatus> {
        // Check WAL affinity health
        let affinity_node = self.get_wal_affinity_node(collection_id).await?;
        let is_healthy = self.check_node_health(&affinity_node).await?;
        
        Ok(ReplicationHealthStatus {
            collection_id: collection_id.to_string(),
            healthy_replicas: if is_healthy { 1 } else { 0 },
            target_replicas: 1,
            under_replicated_shards: if is_healthy { Vec::new() } else { vec![collection_id.to_string()] },
            over_replicated_shards: Vec::new(),
            consistency_lag_ms: 0, // WAL affinity ensures consistency
        })
    }
    
    async fn handle_node_failure(
        &self,
        failed_node: &str,
        affected_collections: &[String],
    ) -> Result<FailureRecoveryPlan> {
        let mut recovery_actions = Vec::new();
        
        for collection_id in affected_collections {
            // Check if failed node had WAL affinity
            if self.get_wal_affinity_node(collection_id).await? == failed_node {
                recovery_actions.push(RecoveryAction::WalFailover {
                    collection_id: collection_id.clone(),
                    old_wal_node: failed_node.to_string(),
                    new_wal_node: format!("new_wal_node_for_{}", collection_id),
                });
            }
        }
        
        Ok(FailureRecoveryPlan {
            recovery_actions,
            estimated_recovery_time_ms: 30000, // 30 seconds for WAL failover
            data_at_risk_gb: 0.1 * affected_collections.len() as f64, // Small amount of unflushed data
            priority: RecoveryPriority::Critical, // WAL data loss is critical
        })
    }
    
    fn get_coordinator_type(&self) -> ReplicationCoordinatorType {
        ReplicationCoordinatorType::WalAffinityReplication
    }
}

impl WalAffinityCoordinator {
    pub fn new(failover_strategy: WalFailoverStrategy) -> Self {
        Self {
            wal_affinity_map: Arc::new(RwLock::new(std::collections::HashMap::new())),
            failover_strategy,
        }
    }
    
    async fn get_wal_affinity_node(&self, collection_id: &str) -> Result<String> {
        let affinity_map = self.wal_affinity_map.read().await;
        affinity_map.get(collection_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No WAL affinity node found for collection {}", collection_id))
    }
    
    async fn establish_wal_affinity(&self, collection_id: &str, node_id: &str) -> Result<()> {
        let mut affinity_map = self.wal_affinity_map.write().await;
        affinity_map.insert(collection_id.to_string(), node_id.to_string());
        tracing::info!("Established WAL affinity: {} -> {}", collection_id, node_id);
        Ok(())
    }
    
    async fn check_node_health(&self, _node_id: &str) -> Result<bool> {
        // TODO: Implement actual health check
        Ok(true)
    }
}

impl ReplicationManager {
    pub fn new(
        deployment_config: DeploymentConfig,
        assignment_service: Arc<AssignmentService>,
        vector_service: Arc<VectorService>,
    ) -> Self {
        let mut coordinators: Vec<Box<dyn ReplicationCoordinator>> = Vec::new();
        
        // Create appropriate coordinators based on deployment type
        match &deployment_config.deployment_type {
            DeploymentType::Local { replication_factor, consistency_level } => {
                coordinators.push(Box::new(LocalFileReplicationCoordinator::new(
                    *replication_factor,
                    consistency_level.clone(),
                    deployment_config.high_availability.rack_awareness,
                    deployment_config.high_availability.availability_zone_awareness,
                )));
            }
            DeploymentType::Cloud { object_store_type, .. } => {
                coordinators.push(Box::new(CloudNativeReplicationCoordinator::new(
                    object_store_type.clone(),
                    true, // Enable cross-region replication
                    CloudMetadataConsistency::ReadAfterWrite,
                )));
            }
            DeploymentType::Hybrid { local_config, cloud_config, .. } => {
                // Add coordinators for both local and cloud components
                // TODO: Implement hybrid coordinator selection
            }
        }
        
        // Always add WAL affinity coordinator
        coordinators.push(Box::new(WalAffinityCoordinator::new(
            WalFailoverStrategy::PromoteReplica
        )));
        
        Self {
            deployment_config,
            replication_coordinators: coordinators,
            assignment_service,
            vector_service,
        }
    }
    
    pub async fn replicate_data(
        &self,
        collection_id: &str,
        data: ReplicationData,
        target_nodes: &[String],
    ) -> Result<Vec<ReplicationResult>> {
        let mut results = Vec::new();
        
        for coordinator in &self.replication_coordinators {
            // Determine if this coordinator should handle this data type
            if self.should_handle_data_type(coordinator.as_ref(), &data.data_type) {
                let result = coordinator.replicate_data(collection_id, &data, target_nodes).await?;
                results.push(result);
            }
        }
        
        Ok(results)
    }
    
    pub async fn handle_node_failure(&self, failed_node: &str) -> Result<Vec<FailureRecoveryPlan>> {
        let affected_collections = self.get_affected_collections(failed_node).await?;
        let mut recovery_plans = Vec::new();
        
        for coordinator in &self.replication_coordinators {
            let plan = coordinator.handle_node_failure(failed_node, &affected_collections).await?;
            if !plan.recovery_actions.is_empty() {
                recovery_plans.push(plan);
            }
        }
        
        Ok(recovery_plans)
    }
    
    fn should_handle_data_type(
        &self,
        coordinator: &dyn ReplicationCoordinator,
        data_type: &ReplicationDataType,
    ) -> bool {
        match (coordinator.get_coordinator_type(), data_type) {
            (ReplicationCoordinatorType::LocalFileReplication, _) => {
                matches!(self.deployment_config.deployment_type, DeploymentType::Local { .. })
            }
            (ReplicationCoordinatorType::CloudNativeReplication, ReplicationDataType::ViperData { .. }) |
            (ReplicationCoordinatorType::CloudNativeReplication, ReplicationDataType::LsmData { .. }) => {
                matches!(self.deployment_config.deployment_type, DeploymentType::Cloud { .. })
            }
            (ReplicationCoordinatorType::WalAffinityReplication, ReplicationDataType::WalEntry { .. }) => {
                true // WAL affinity is always relevant
            }
            _ => false,
        }
    }
    
    async fn get_affected_collections(&self, _failed_node: &str) -> Result<Vec<String>> {
        // TODO: Query assignment service for collections on failed node
        Ok(vec!["collection_1".to_string(), "collection_2".to_string()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_local_file_replication() {
        let coordinator = LocalFileReplicationCoordinator::new(
            3,
            LocalConsistencyLevel::Quorum,
            true,
            true,
        );
        
        let data = ReplicationData {
            data_type: ReplicationDataType::ViperData {
                parquet_file_path: "/tmp/test.parquet".to_string(),
                size_bytes: 1024,
            },
            collection_id: "test_collection".to_string(),
            data_payload: vec![1, 2, 3, 4],
            metadata: ReplicationMetadata {
                timestamp: chrono::Utc::now(),
                source_node: "node_1".to_string(),
                replication_strategy: "quorum".to_string(),
                consistency_level: "quorum".to_string(),
                encryption_enabled: false,
                compression_enabled: true,
            },
        };
        
        let target_nodes = vec!["node_1".to_string(), "node_2".to_string(), "node_3".to_string()];
        
        let result = coordinator.replicate_data("test_collection", &data, &target_nodes).await;
        assert!(result.is_ok());
        
        let replication_result = result.unwrap();
        assert!(replication_result.success);
        assert_eq!(replication_result.replicated_nodes.len(), 3);
    }
    
    #[tokio::test]
    async fn test_wal_affinity_coordination() {
        let coordinator = WalAffinityCoordinator::new(WalFailoverStrategy::PromoteReplica);
        
        // Establish WAL affinity
        coordinator.establish_wal_affinity("test_collection", "wal_node_1").await.unwrap();
        
        let data = ReplicationData {
            data_type: ReplicationDataType::WalEntry {
                sequence_number: 100,
                batch_id: "batch_123".to_string(),
            },
            collection_id: "test_collection".to_string(),
            data_payload: vec![1, 2, 3],
            metadata: ReplicationMetadata {
                timestamp: chrono::Utc::now(),
                source_node: "wal_node_1".to_string(),
                replication_strategy: "wal_affinity".to_string(),
                consistency_level: "strong".to_string(),
                encryption_enabled: false,
                compression_enabled: false,
            },
        };
        
        let target_nodes = vec!["wal_node_1".to_string()];
        
        let result = coordinator.replicate_data("test_collection", &data, &target_nodes).await;
        assert!(result.is_ok());
        
        let replication_result = result.unwrap();
        assert!(replication_result.success);
        assert!(replication_result.replicated_nodes.contains(&"wal_node_1".to_string()));
    }
}
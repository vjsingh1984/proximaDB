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

//! # PULSAR Replication Module
//!
//! Implements master-slave replication for fault tolerance in the distributed graph engine.
//! Supports configurable replication factors from 1 to 3.

use crate::core::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::{Node, Edge, NodeId, EdgeId};
use crate::graph::engines::orion::OrionGraphEngine;
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use dashmap::DashMap;
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant};

/// Replication manager for PULSAR engine
#[derive(Debug)]
pub struct ReplicationManager {
    /// Replication factor (1-3)
    replication_factor: u8,
    /// Mapping of primary shard to replica shards
    replica_mapping: Arc<RwLock<HashMap<u32, Vec<u32>>>>,
    /// Reference to all shard engines
    shards: Arc<DashMap<u32, Arc<OrionGraphEngine>>>,
    /// Replication statistics
    stats: Arc<RwLock<ReplicationStats>>,
}

/// Replication statistics
#[derive(Debug, Default)]
pub struct ReplicationStats {
    pub successful_replications: u64,
    pub failed_replications: u64,
    pub average_replication_time_ms: f64,
    pub replica_lag_ms: HashMap<u32, u64>,
    pub last_replication_times: HashMap<u32, Instant>,
}

/// Replication strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationStrategy {
    /// Synchronous replication - wait for all replicas
    Synchronous,
    /// Asynchronous replication - fire and forget
    Asynchronous,
    /// Semi-synchronous - wait for at least one replica
    SemiSynchronous,
}

impl ReplicationManager {
    /// Create a new replication manager
    pub fn new(
        replication_factor: u8,
        shards: &Arc<DashMap<u32, Arc<OrionGraphEngine>>>,
    ) -> Self {
        if replication_factor == 0 || replication_factor > 3 {
            panic!("Replication factor must be between 1 and 3");
        }
        
        let replica_mapping = Self::build_replica_mapping(replication_factor, shards);
        
        Self {
            replication_factor,
            replica_mapping: Arc::new(RwLock::new(replica_mapping)),
            shards: Arc::clone(shards),
            stats: Arc::new(RwLock::new(ReplicationStats::default())),
        }
    }
    
    /// Build initial replica mapping
    fn build_replica_mapping(
        replication_factor: u8,
        shards: &Arc<DashMap<u32, Arc<OrionGraphEngine>>>,
    ) -> HashMap<u32, Vec<u32>> {
        let mut mapping = HashMap::new();
        let all_shards: Vec<u32> = shards.iter().map(|entry| *entry.key()).collect();
        let shard_count = all_shards.len();
        
        if shard_count == 0 {
            return mapping;
        }
        
        for (i, &primary_shard) in all_shards.iter().enumerate() {
            let mut replicas = Vec::new();
            
            // Add replicas in a round-robin fashion
            for j in 1..replication_factor {
                let replica_index = (i + j as usize) % shard_count;
                let replica_shard = all_shards[replica_index];
                
                // Don't replicate to self
                if replica_shard != primary_shard {
                    replicas.push(replica_shard);
                }
            }
            
            mapping.insert(primary_shard, replicas);
        }
        
        mapping
    }
    
    /// Get replica shards for a primary shard
    pub async fn get_replicas(&self, primary_shard: u32) -> Result<Vec<u32>> {
        let mapping = self.replica_mapping.read().await;
        Ok(mapping.get(&primary_shard).cloned().unwrap_or_default())
    }
    
    /// Replicate node insertion to replica shards
    pub async fn replicate_node_insert(&self, node: Node) -> Result<()> {
        self.replicate_operation(
            ReplicationOperation::InsertNode(node),
            ReplicationStrategy::Asynchronous,
        ).await
    }
    
    /// Replicate edge insertion to replica shards
    pub async fn replicate_edge_insert(&self, edge: Edge) -> Result<()> {
        self.replicate_operation(
            ReplicationOperation::InsertEdge(edge),
            ReplicationStrategy::Asynchronous,
        ).await
    }
    
    /// Replicate node update to replica shards
    pub async fn replicate_node_update(&self, node: Node) -> Result<()> {
        self.replicate_operation(
            ReplicationOperation::UpdateNode(node),
            ReplicationStrategy::SemiSynchronous,
        ).await
    }
    
    /// Replicate edge update to replica shards
    pub async fn replicate_edge_update(&self, edge: Edge) -> Result<()> {
        self.replicate_operation(
            ReplicationOperation::UpdateEdge(edge),
            ReplicationStrategy::SemiSynchronous,
        ).await
    }
    
    /// Replicate node deletion to replica shards
    pub async fn replicate_node_delete(&self, node_id: NodeId) -> Result<()> {
        self.replicate_operation(
            ReplicationOperation::DeleteNode(node_id),
            ReplicationStrategy::Synchronous,
        ).await
    }
    
    /// Replicate edge deletion to replica shards
    pub async fn replicate_edge_delete(&self, edge_id: EdgeId) -> Result<()> {
        self.replicate_operation(
            ReplicationOperation::DeleteEdge(edge_id),
            ReplicationStrategy::Synchronous,
        ).await
    }
    
    /// Execute replication operation with specified strategy
    async fn replicate_operation(
        &self,
        operation: ReplicationOperation,
        strategy: ReplicationStrategy,
    ) -> Result<()> {
        let start_time = Instant::now();
        
        let primary_shard = self.get_primary_shard_for_operation(&operation)?;
        let replicas = self.get_replicas(primary_shard).await?;
        
        if replicas.is_empty() {
            // No replication needed
            return Ok(());
        }
        
        match strategy {
            ReplicationStrategy::Synchronous => {
                self.execute_synchronous_replication(operation, &replicas).await
            },
            ReplicationStrategy::Asynchronous => {
                self.execute_asynchronous_replication(operation, &replicas).await
            },
            ReplicationStrategy::SemiSynchronous => {
                self.execute_semi_synchronous_replication(operation, &replicas).await
            },
        }?;
        
        // Update statistics
        let duration = start_time.elapsed();
        let mut stats = self.stats.write().await;
        stats.successful_replications += 1;
        
        // Update average replication time
        let new_avg = if stats.successful_replications == 1 {
            duration.as_millis() as f64
        } else {
            (stats.average_replication_time_ms * (stats.successful_replications - 1) as f64
                + duration.as_millis() as f64) / stats.successful_replications as f64
        };
        stats.average_replication_time_ms = new_avg;
        
        // Update last replication time for replicas
        for &replica_shard in &replicas {
            stats.last_replication_times.insert(replica_shard, Instant::now());
        }
        
        Ok(())
    }
    
    /// Execute synchronous replication (wait for all replicas)
    async fn execute_synchronous_replication(
        &self,
        operation: ReplicationOperation,
        replicas: &[u32],
    ) -> Result<()> {
        let mut tasks = Vec::new();
        
        for &replica_shard in replicas {
            if let Some(shard) = self.shards.get(&replica_shard) {
                let shard = Arc::clone(&shard);
                let op = operation.clone();
                
                tasks.push(tokio::spawn(async move {
                    Self::execute_operation_on_shard(&shard, op).await
                }));
            }
        }
        
        // Wait for all replications to complete
        for task in tasks {
            task.await.map_err(|e| ProximaDBError::InternalError(e.to_string()))??;
        }
        
        Ok(())
    }
    
    /// Execute asynchronous replication (fire and forget)
    async fn execute_asynchronous_replication(
        &self,
        operation: ReplicationOperation,
        replicas: &[u32],
    ) -> Result<()> {
        for &replica_shard in replicas {
            if let Some(shard) = self.shards.get(&replica_shard) {
                let shard = Arc::clone(&shard);
                let op = operation.clone();
                let stats = Arc::clone(&self.stats);
                
                tokio::spawn(async move {
                    if let Err(e) = Self::execute_operation_on_shard(&shard, op).await {
                        tracing::error!("Async replication failed for shard {}: {:?}", replica_shard, e);
                        
                        // Update failure stats
                        let mut stats = stats.write().await;
                        stats.failed_replications += 1;
                    }
                });
            }
        }
        
        Ok(())
    }
    
    /// Execute semi-synchronous replication (wait for at least one replica)
    async fn execute_semi_synchronous_replication(
        &self,
        operation: ReplicationOperation,
        replicas: &[u32],
    ) -> Result<()> {
        if replicas.is_empty() {
            return Ok(());
        }
        
        let mut tasks = Vec::new();
        
        for &replica_shard in replicas {
            if let Some(shard) = self.shards.get(&replica_shard) {
                let shard = Arc::clone(&shard);
                let op = operation.clone();
                
                tasks.push(tokio::spawn(async move {
                    Self::execute_operation_on_shard(&shard, op).await
                }));
            }
        }
        
        if tasks.is_empty() {
            return Ok(());
        }
        
        // Wait for at least one replication to succeed
        let (result, _index, remaining) = futures::future::select_all(tasks).await;
        
        // Check if the first completed task succeeded
        result.map_err(|e| ProximaDBError::InternalError(e.to_string()))??;
        
        // Let remaining tasks complete in background
        for task in remaining {
            tokio::spawn(async move {
                if let Err(e) = task.await {
                    tracing::warn!("Background replication task failed: {:?}", e);
                }
            });
        }
        
        Ok(())
    }
    
    /// Execute operation on a specific shard
    async fn execute_operation_on_shard(
        shard: &OrionGraphEngine,
        operation: ReplicationOperation,
    ) -> Result<()> {
        match operation {
            ReplicationOperation::InsertNode(node) => {
                shard.insert_node(node)?;
            },
            ReplicationOperation::InsertEdge(edge) => {
                shard.insert_edge(edge)?;
            },
            ReplicationOperation::UpdateNode(node) => {
                shard.update_node(node)?;
            },
            ReplicationOperation::UpdateEdge(edge) => {
                shard.update_edge(edge)?;
            },
            ReplicationOperation::DeleteNode(node_id) => {
                shard.delete_node(&node_id)?;
            },
            ReplicationOperation::DeleteEdge(edge_id) => {
                shard.delete_edge(&edge_id)?;
            },
        }
        
        Ok(())
    }
    
    /// Get primary shard for an operation (for determining replicas)
    fn get_primary_shard_for_operation(&self, operation: &ReplicationOperation) -> Result<u32> {
        // For now, simple hash-based assignment
        // In a real implementation, this would use the consistent hash ring
        let key = match operation {
            ReplicationOperation::InsertNode(node) => &node.id,
            ReplicationOperation::UpdateNode(node) => &node.id,
            ReplicationOperation::DeleteNode(node_id) => node_id,
            ReplicationOperation::InsertEdge(edge) => &edge.from_node_id,
            ReplicationOperation::UpdateEdge(edge) => &edge.from_node_id,
            ReplicationOperation::DeleteEdge(edge_id) => edge_id,
        };
        
        // Simple hash function for demo purposes
        let hash = key.chars().map(|c| c as u32).sum::<u32>();
        let shard_count = self.shards.len() as u32;
        
        Ok(hash % shard_count)
    }
    
    /// Get replication statistics
    pub async fn get_stats(&self) -> ReplicationStats {
        let stats = self.stats.read().await;
        ReplicationStats {
            successful_replications: stats.successful_replications,
            failed_replications: stats.failed_replications,
            average_replication_time_ms: stats.average_replication_time_ms,
            replica_lag_ms: stats.replica_lag_ms.clone(),
            last_replication_times: stats.last_replication_times.clone(),
        }
    }
    
    /// Update replica mapping (for dynamic scaling)
    pub async fn update_replica_mapping(&self, new_mapping: HashMap<u32, Vec<u32>>) {
        let mut mapping = self.replica_mapping.write().await;
        *mapping = new_mapping;
    }
    
    /// Check replica health and detect lag
    pub async fn check_replica_health(&self) -> Result<HashMap<u32, ReplicaHealth>> {
        let mut health_map = HashMap::new();
        let stats = self.stats.read().await;
        let now = Instant::now();
        
        let mapping = self.replica_mapping.read().await;
        for (&primary_shard, replicas) in mapping.iter() {
            for &replica_shard in replicas {
                let last_replication = stats.last_replication_times.get(&replica_shard);
                let lag_ms = if let Some(last_time) = last_replication {
                    now.duration_since(*last_time).as_millis() as u64
                } else {
                    u64::MAX // Never replicated
                };
                
                let health = if lag_ms < 1000 {
                    ReplicaHealth::Healthy
                } else if lag_ms < 5000 {
                    ReplicaHealth::Lagging
                } else {
                    ReplicaHealth::Unhealthy
                };
                
                health_map.insert(replica_shard, health);
            }
        }
        
        Ok(health_map)
    }
}

/// Replication operations
#[derive(Debug, Clone)]
pub enum ReplicationOperation {
    InsertNode(Node),
    InsertEdge(Edge),
    UpdateNode(Node),
    UpdateEdge(Edge),
    DeleteNode(NodeId),
    DeleteEdge(EdgeId),
}

/// Replica health status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicaHealth {
    Healthy,
    Lagging,
    Unhealthy,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphMemoryPool;
    use crate::proto::proximadb_v1::property_value::Value;
    use crate::graph::PropertyValue;
    
    fn create_test_shards(count: u32) -> Arc<DashMap<u32, Arc<OrionGraphEngine>>> {
        let shards = Arc::new(DashMap::new());
        let memory_pool = Arc::new(GraphMemoryPool::new());
        
        for i in 0..count {
            let engine = Arc::new(OrionGraphEngine::with_memory_pool(Arc::clone(&memory_pool)));
            shards.insert(i, engine);
        }
        
        shards
    }
    
    #[tokio::test]
    async fn test_replication_manager_creation() {
        let shards = create_test_shards(4);
        let manager = ReplicationManager::new(2, &shards);
        
        assert_eq!(manager.replication_factor, 2);
        
        // Test replica mapping
        let replicas_0 = manager.get_replicas(0).await.unwrap();
        assert!(!replicas_0.is_empty());
        assert!(replicas_0.len() <= 1); // Replication factor - 1
    }
    
    #[tokio::test]
    async fn test_node_replication() {
        let shards = create_test_shards(3);
        let manager = ReplicationManager::new(2, &shards);
        
        let test_node = Node {
            id: "test_node".to_string(),
            labels: vec!["Test".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        
        // Test replication
        let result = manager.replicate_node_insert(test_node).await;
        assert!(result.is_ok());
        
        // Check stats
        tokio::time::sleep(Duration::from_millis(10)).await;
        let stats = manager.get_stats().await;
        assert_eq!(stats.successful_replications, 1);
    }
    
    #[tokio::test]
    async fn test_replica_health_check() {
        let shards = create_test_shards(2);
        let manager = ReplicationManager::new(2, &shards);
        
        // Initially no replication history
        let health = manager.check_replica_health().await.unwrap();
        
        // All replicas should be unhealthy (never replicated)
        for status in health.values() {
            assert_eq!(*status, ReplicaHealth::Unhealthy);
        }
    }
    
    #[test]
    fn test_replica_mapping_generation() {
        let shards = create_test_shards(4);
        let mapping = ReplicationManager::build_replica_mapping(2, &shards);
        
        // Should have mapping for all shards
        assert_eq!(mapping.len(), 4);
        
        // Each shard should have exactly 1 replica (replication_factor - 1)
        for replicas in mapping.values() {
            assert_eq!(replicas.len(), 1);
        }
        
        // No shard should replicate to itself
        for (&primary, replicas) in &mapping {
            assert!(!replicas.contains(&primary));
        }
    }
    
    #[test]
    fn test_replication_operation_cloning() {
        let node = Node {
            id: "test".to_string(),
            labels: vec!["Test".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        
        let op1 = ReplicationOperation::InsertNode(node);
        let op2 = op1.clone();
        
        match (op1, op2) {
            (ReplicationOperation::InsertNode(n1), ReplicationOperation::InsertNode(n2)) => {
                assert_eq!(n1.id, n2.id);
            },
            _ => panic!("Unexpected operation types"),
        }
    }
}
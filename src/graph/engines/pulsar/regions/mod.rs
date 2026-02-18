/*
 * Copyright 2025 ProximaDB
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

//! Multi-Region Graph Coordination
//!
//! This module provides region-aware routing and replication for distributed
//! graph databases spanning multiple geographic regions.
//!
//! # Design Principles
//!
//! - **Trait-Based**: RegionManager trait enables multiple region strategies
//! - **Reuse RAFT**: Leverages existing RAFT consensus for region coordination
//! - **Geo-Aware Routing**: Routes queries to nearest region for low latency
//! - **Async Replication**: Background replication with lag tracking
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │   MultiRegionCoordinator                │
//! │  ┌───────────────────────────────────┐  │
//! │  │   RegionManager (trait)           │  │ ← Pluggable
//! │  └───────────────────────────────────┘  │
//! │  ┌───────────────────────────────────┐  │
//! │  │   GraphRaftNode                   │  │ ← Reuse
//! │  │  (region failover via RAFT)       │  │
//! │  └───────────────────────────────────┘  │
//! │  ┌───────────────────────────────────┐  │
//! │  │   ReplicationLagTracker           │  │ ← NEW
//! │  └───────────────────────────────────┘  │
//! └─────────────────────────────────────────┘
//!          ↓ replicates to
//! ┌─────────────────────────────────────────┐
//! │   Peer Regions (US-EAST, EU-WEST, ...)  │
//! └─────────────────────────────────────────┘
//! ```

use crate::core::error::ProximaDBError;
use crate::graph::engines::GraphEngine;
use crate::graph::engines::pulsar::consensus::GraphCommand;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Type alias for region ID
pub type RegionId = String;

/// Region configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionConfig {
    /// Unique region identifier (e.g., "us-east-1", "eu-west-1")
    pub id: RegionId,
    /// Display name (e.g., "US East (N. Virginia)")
    pub name: String,
    /// Geographic location coordinates (lat, lon)
    pub location: (f64, f64),
    /// Endpoint URL for this region
    pub endpoint: String,
    /// Whether this region is currently active
    pub active: bool,
    /// Priority for read routing (lower = higher priority)
    pub read_priority: u32,
}

/// Replication lag information
#[derive(Debug, Clone)]
pub struct ReplicationLag {
    /// Target region
    pub region_id: RegionId,
    /// Lag in milliseconds
    pub lag_ms: u64,
    /// Last update timestamp
    pub last_updated: Instant,
    /// Number of pending operations
    pub pending_ops: usize,
}

/// Region manager trait
///
/// This trait enables multiple region management strategies:
/// - Active-Active: All regions accept writes
/// - Active-Passive: Primary region accepts writes, others are read-only
/// - Multi-Master: Each region can be primary for different shards
pub trait RegionManager: Send + Sync {
    /// Get the local region ID
    fn get_local_region(
        &self,
    ) -> impl std::future::Future<Output = Result<RegionId, ProximaDBError>> + Send;

    /// Get all peer regions
    fn get_peer_regions(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<RegionConfig>, ProximaDBError>> + Send;

    /// Replicate operations to a specific region
    fn replicate_to_region(
        &self,
        region: RegionId,
        ops: Vec<GraphCommand>,
    ) -> impl std::future::Future<Output = Result<(), ProximaDBError>> + Send;

    /// Promote a region to primary (for failover)
    fn promote_region(
        &self,
        region: RegionId,
    ) -> impl std::future::Future<Output = Result<(), ProximaDBError>> + Send;

    /// Get replication lag for all regions
    fn get_replication_lag(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<ReplicationLag>, ProximaDBError>> + Send;

    /// Route a read query to the best region
    fn route_read_query(
        &self,
        query_location: Option<(f64, f64)>,
    ) -> impl std::future::Future<Output = Result<RegionId, ProximaDBError>> + Send;
}

/// Replication lag tracker
///
/// Tracks replication lag for each peer region to enable:
/// - Read-your-writes consistency
/// - Stale read detection
/// - Smart routing based on lag
pub struct ReplicationLagTracker {
    /// Lag information per region
    lag_info: Arc<DashMap<RegionId, ReplicationLag>>,
    /// Maximum acceptable lag (ms)
    max_acceptable_lag_ms: u64,
}

impl ReplicationLagTracker {
    pub fn new(max_acceptable_lag_ms: u64) -> Self {
        Self {
            lag_info: Arc::new(DashMap::new()),
            max_acceptable_lag_ms,
        }
    }

    /// Update lag for a region
    pub fn update_lag(&self, region_id: RegionId, lag_ms: u64, pending_ops: usize) {
        self.lag_info.insert(
            region_id.clone(),
            ReplicationLag {
                region_id,
                lag_ms,
                last_updated: Instant::now(),
                pending_ops,
            },
        );
    }

    /// Get lag for a specific region
    pub fn get_lag(&self, region_id: &RegionId) -> Option<ReplicationLag> {
        self.lag_info.get(region_id).map(|entry| entry.clone())
    }

    /// Get all regions with acceptable lag
    pub fn get_healthy_regions(&self) -> Vec<RegionId> {
        self.lag_info
            .iter()
            .filter(|entry| entry.lag_ms <= self.max_acceptable_lag_ms)
            .map(|entry| entry.region_id.clone())
            .collect()
    }

    /// Get all lag information
    pub fn get_all_lags(&self) -> Vec<ReplicationLag> {
        self.lag_info.iter().map(|entry| entry.clone()).collect()
    }
}

/// Multi-region coordinator
///
/// Coordinates graph operations across multiple geographic regions.
pub struct MultiRegionCoordinator {
    /// Local region ID
    local_region: RegionId,

    /// Configuration for peer regions
    peer_regions: Arc<RwLock<HashMap<RegionId, RegionConfig>>>,

    /// Replication lag tracker
    lag_tracker: Arc<ReplicationLagTracker>,

    /// Pending operations for replication
    pending_operations: Arc<DashMap<RegionId, Vec<GraphCommand>>>,

    /// Maximum replication batch size
    max_batch_size: usize,

    /// Replication strategy
    strategy: ReplicationStrategy,
}

/// Replication strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationStrategy {
    /// Synchronous: Wait for all regions to acknowledge
    Synchronous,
    /// Asynchronous: Fire-and-forget to peer regions
    Asynchronous,
    /// Quorum: Wait for majority of regions
    Quorum,
}

impl MultiRegionCoordinator {
    /// Create a new multi-region coordinator
    pub fn new(
        local_region: RegionId,
        peer_regions: Vec<RegionConfig>,
        max_acceptable_lag_ms: u64,
        max_batch_size: usize,
        strategy: ReplicationStrategy,
    ) -> Self {
        let peer_map: HashMap<RegionId, RegionConfig> = peer_regions
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();

        Self {
            local_region,
            peer_regions: Arc::new(RwLock::new(peer_map)),
            lag_tracker: Arc::new(ReplicationLagTracker::new(max_acceptable_lag_ms)),
            pending_operations: Arc::new(DashMap::new()),
            max_batch_size,
            strategy,
        }
    }

    /// Add a peer region
    pub async fn add_peer_region(&self, region: RegionConfig) -> Result<(), ProximaDBError> {
        let mut regions = self.peer_regions.write().await;
        regions.insert(region.id.clone(), region);
        Ok(())
    }

    /// Remove a peer region
    pub async fn remove_peer_region(&self, region_id: &RegionId) -> Result<(), ProximaDBError> {
        let mut regions = self.peer_regions.write().await;
        regions.remove(region_id);
        self.pending_operations.remove(region_id);
        Ok(())
    }

    /// Queue an operation for replication
    pub async fn queue_operation(
        &self,
        region_id: RegionId,
        op: GraphCommand,
    ) -> Result<(), ProximaDBError> {
        self.pending_operations
            .entry(region_id)
            .or_insert_with(Vec::new)
            .push(op);

        Ok(())
    }

    /// Flush pending operations to a region
    pub async fn flush_to_region(&self, region_id: &RegionId) -> Result<(), ProximaDBError> {
        // Get and remove pending operations
        let ops = if let Some((_, ops)) = self.pending_operations.remove(region_id) {
            ops
        } else {
            return Ok(()); // No pending operations
        };

        if ops.is_empty() {
            return Ok(());
        }

        // In a real implementation, this would:
        // 1. Send ops to remote region via RPC
        // 2. Wait for acknowledgment based on strategy
        // 3. Update lag tracker
        //
        // For now, simulate successful replication
        self.lag_tracker.update_lag(region_id.clone(), 0, 0);

        Ok(())
    }

    /// Get the replication lag tracker (for testing)
    pub fn lag_tracker(&self) -> &Arc<ReplicationLagTracker> {
        &self.lag_tracker
    }

    /// Calculate distance between two coordinates (Haversine formula)
    fn calculate_distance(coord1: (f64, f64), coord2: (f64, f64)) -> f64 {
        let (lat1, lon1) = (coord1.0.to_radians(), coord1.1.to_radians());
        let (lat2, lon2) = (coord2.0.to_radians(), coord2.1.to_radians());

        let dlat = lat2 - lat1;
        let dlon = lon2 - lon1;

        let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
        let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

        // Earth's radius in kilometers
        6371.0 * c
    }

    /// Start background replication worker
    pub async fn start_replication_worker(&self) -> Result<(), ProximaDBError> {
        // In a real implementation, this would spawn a background task
        // that periodically flushes pending operations to all regions
        //
        // For now, just a placeholder
        Ok(())
    }
}

impl RegionManager for MultiRegionCoordinator {
    async fn get_local_region(&self) -> Result<RegionId, ProximaDBError> {
        Ok(self.local_region.clone())
    }

    async fn get_peer_regions(&self) -> Result<Vec<RegionConfig>, ProximaDBError> {
        let regions = self.peer_regions.read().await;
        Ok(regions.values().cloned().collect())
    }

    async fn replicate_to_region(
        &self,
        region: RegionId,
        ops: Vec<GraphCommand>,
    ) -> Result<(), ProximaDBError> {
        match self.strategy {
            ReplicationStrategy::Synchronous => {
                // Queue and immediately flush
                for op in ops {
                    self.queue_operation(region.clone(), op).await?;
                }
                self.flush_to_region(&region).await?;
            }
            ReplicationStrategy::Asynchronous => {
                // Just queue, will be flushed by background worker
                for op in ops {
                    self.queue_operation(region.clone(), op).await?;
                }
            }
            ReplicationStrategy::Quorum => {
                // Queue and flush to achieve quorum
                for op in ops {
                    self.queue_operation(region.clone(), op).await?;
                }
                // In full impl, would wait for quorum of acknowledgments
                self.flush_to_region(&region).await?;
            }
        }

        Ok(())
    }

    async fn promote_region(&self, region: RegionId) -> Result<(), ProximaDBError> {
        let mut regions = self.peer_regions.write().await;

        // Set all regions to read-only except the promoted one
        for (id, config) in regions.iter_mut() {
            if id == &region {
                config.active = true;
                config.read_priority = 0; // Highest priority
            } else {
                config.read_priority = 100; // Lower priority
            }
        }

        Ok(())
    }

    async fn get_replication_lag(&self) -> Result<Vec<ReplicationLag>, ProximaDBError> {
        Ok(self.lag_tracker.get_all_lags())
    }

    async fn route_read_query(
        &self,
        query_location: Option<(f64, f64)>,
    ) -> Result<RegionId, ProximaDBError> {
        let regions = self.peer_regions.read().await;

        // If no query location provided, return local region
        let query_loc = match query_location {
            Some(loc) => loc,
            None => return Ok(self.local_region.clone()),
        };

        // Find closest healthy region
        let healthy_regions = self.lag_tracker.get_healthy_regions();

        let mut best_region = self.local_region.clone();
        let mut best_distance = f64::MAX;

        for region_id in healthy_regions {
            if let Some(config) = regions.get(&region_id) {
                if !config.active {
                    continue;
                }

                let distance = Self::calculate_distance(query_loc, config.location);
                if distance < best_distance {
                    best_distance = distance;
                    best_region = region_id.clone();
                }
            }
        }

        Ok(best_region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replication_lag_tracker() {
        let tracker = ReplicationLagTracker::new(100);

        tracker.update_lag("us-east-1".to_string(), 50, 10);
        tracker.update_lag("eu-west-1".to_string(), 150, 20);

        let lag = tracker.get_lag(&"us-east-1".to_string()).unwrap();
        assert_eq!(lag.lag_ms, 50);
        assert_eq!(lag.pending_ops, 10);

        let healthy = tracker.get_healthy_regions();
        assert_eq!(healthy.len(), 1);
        assert!(healthy.contains(&"us-east-1".to_string()));
    }

    #[tokio::test]
    async fn test_multi_region_coordinator_creation() {
        let peer_regions = vec![
            RegionConfig {
                id: "us-east-1".to_string(),
                name: "US East".to_string(),
                location: (39.0, -77.0),
                endpoint: "https://us-east-1.example.com".to_string(),
                active: true,
                read_priority: 1,
            },
            RegionConfig {
                id: "eu-west-1".to_string(),
                name: "EU West".to_string(),
                location: (53.0, -8.0),
                endpoint: "https://eu-west-1.example.com".to_string(),
                active: true,
                read_priority: 2,
            },
        ];

        let coordinator = MultiRegionCoordinator::new(
            "us-west-1".to_string(),
            peer_regions,
            100,
            1000,
            ReplicationStrategy::Asynchronous,
        );

        let local = coordinator.get_local_region().await.unwrap();
        assert_eq!(local, "us-west-1");

        let peers = coordinator.get_peer_regions().await.unwrap();
        assert_eq!(peers.len(), 2);
    }

    #[tokio::test]
    async fn test_region_routing() {
        let peer_regions = vec![
            RegionConfig {
                id: "us-east-1".to_string(),
                name: "US East".to_string(),
                location: (39.0, -77.0), // Virginia
                endpoint: "https://us-east-1.example.com".to_string(),
                active: true,
                read_priority: 1,
            },
            RegionConfig {
                id: "eu-west-1".to_string(),
                name: "EU West".to_string(),
                location: (53.0, -8.0), // Ireland
                endpoint: "https://eu-west-1.example.com".to_string(),
                active: true,
                read_priority: 2,
            },
        ];

        let coordinator = MultiRegionCoordinator::new(
            "us-west-1".to_string(),
            peer_regions,
            100,
            1000,
            ReplicationStrategy::Asynchronous,
        );

        // Mark regions as healthy
        coordinator
            .lag_tracker
            .update_lag("us-east-1".to_string(), 50, 0);
        coordinator
            .lag_tracker
            .update_lag("eu-west-1".to_string(), 60, 0);

        // Query from New York (closer to us-east-1)
        let region = coordinator
            .route_read_query(Some((40.7, -74.0)))
            .await
            .unwrap();
        assert_eq!(region, "us-east-1");

        // Query from London (closer to eu-west-1)
        let region = coordinator
            .route_read_query(Some((51.5, -0.1)))
            .await
            .unwrap();
        assert_eq!(region, "eu-west-1");
    }

    #[test]
    fn test_distance_calculation() {
        // Distance from New York to London (approx 5570 km)
        let ny = (40.7, -74.0);
        let london = (51.5, -0.1);
        let distance = MultiRegionCoordinator::calculate_distance(ny, london);
        assert!(distance > 5500.0 && distance < 5600.0);
    }
}

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

//! # QUASAR Tiering Module
//!
//! Implements automatic hot/cold data tiering logic based on access patterns.
//! Manages data movement between memory (hot) and disk (cold) storage.

use crate::core::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::{Node, Edge, NodeId, EdgeId};
use crate::graph::engines::orion::OrionGraphEngine;
use super::{QuasarConfig, cache::AccessPatternCache, storage_backend::ColdStorageBackend};
use std::sync::Arc;
use std::collections::HashMap;
use tokio::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Manages data tiering between hot and cold storage
#[derive(Debug)]
pub struct TieringManager {
    /// Hot tier (ORION engine)
    hot_tier: Arc<OrionGraphEngine>,
    /// Cold tier storage
    cold_tier: Arc<ColdStorageBackend>,
    /// Access pattern tracker
    access_cache: Arc<AccessPatternCache>,
    /// Configuration
    config: QuasarConfig,
    /// Tiering statistics
    stats: Arc<RwLock<TieringStats>>,
}

/// Statistics for tiering operations
#[derive(Debug, Default)]
pub struct TieringStats {
    /// Number of items migrated to cold storage
    pub cold_migrations: u64,
    /// Number of items promoted to hot storage
    pub hot_promotions: u64,
    /// Total migration time (ms)
    pub total_migration_time_ms: u64,
    /// Last migration cycle time
    pub last_migration_cycle: Option<Instant>,
    /// Items considered for migration
    pub migration_candidates_evaluated: u64,
    /// Migration failures
    pub migration_failures: u64,
    /// Current hot tier utilization (0.0 - 1.0)
    pub hot_tier_utilization: f64,
}

/// Data movement operation
#[derive(Debug, Clone)]
pub enum DataMovement {
    /// Move node from hot to cold
    DemoteNode(NodeId),
    /// Move edge from hot to cold
    DemoteEdge(EdgeId),
    /// Move node from cold to hot
    PromoteNode(NodeId),
    /// Move edge from cold to hot
    PromoteEdge(EdgeId),
}

/// Migration candidate with priority score
#[derive(Debug, Clone)]
pub struct MigrationCandidate {
    pub item_id: String,
    pub item_type: ItemType,
    pub priority_score: f64,
    pub last_access: Instant,
    pub access_frequency: u32,
}

/// Item type for migration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemType {
    Node,
    Edge,
}

impl TieringManager {
    /// Create a new tiering manager
    pub fn new(
        hot_tier: Arc<OrionGraphEngine>,
        cold_tier: Arc<ColdStorageBackend>,
        access_cache: Arc<AccessPatternCache>,
        config: QuasarConfig,
    ) -> Self {
        Self {
            hot_tier,
            cold_tier,
            access_cache,
            config,
            stats: Arc::new(RwLock::new(TieringStats::default())),
        }
    }
    
    /// Perform a migration cycle
    pub async fn perform_migration_cycle(&self) -> Result<()> {
        let cycle_start = Instant::now();
        
        tracing::debug!("Starting migration cycle");
        
        // Update hot tier utilization
        self.update_hot_tier_utilization().await?;
        
        // Get migration candidates
        let candidates = self.get_migration_candidates().await?;
        
        if !candidates.is_empty() {
            tracing::debug!("Found {} migration candidates", candidates.len());
            
            // Execute migrations
            self.execute_migrations(candidates).await?;
        }
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.last_migration_cycle = Some(cycle_start);
            stats.total_migration_time_ms += cycle_start.elapsed().as_millis() as u64;
        }
        
        tracing::debug!("Migration cycle completed in {:?}", cycle_start.elapsed());
        Ok(())
    }
    
    /// Get candidates for migration based on access patterns
    async fn get_migration_candidates(&self) -> Result<Vec<MigrationCandidate>> {
        let mut candidates = Vec::new();
        let now = Instant::now();
        
        // Get hot tier utilization
        let utilization = {
            let stats = self.stats.read().await;
            stats.hot_tier_utilization
        };
        
        // Only migrate if hot tier is getting full (> 80%)
        if utilization < 0.8 {
            return Ok(candidates);
        }
        
        // Get access patterns for all items in hot tier
        let access_patterns = self.access_cache.get_all_access_patterns().await;
        
        for (item_id, access_info) in access_patterns {
            let time_since_access = now.duration_since(access_info.last_access);
            
            // Consider for cold migration if not accessed recently
            if time_since_access > self.config.cold_migration_threshold {
                let priority_score = self.calculate_cold_priority_score(
                    time_since_access,
                    access_info.access_count,
                );
                
                // Determine if this is a node or edge (simplified logic)
                let item_type = if self.hot_tier.get_node(&item_id).is_ok() {
                    ItemType::Node
                } else {
                    ItemType::Edge
                };
                
                candidates.push(MigrationCandidate {
                    item_id,
                    item_type,
                    priority_score,
                    last_access: access_info.last_access,
                    access_frequency: access_info.access_count,
                });
            }
        }
        
        // Sort by priority score (highest first = most suitable for cold storage)
        candidates.sort_by(|a, b| b.priority_score.partial_cmp(&a.priority_score).unwrap_or(std::cmp::Ordering::Equal));
        
        // Limit candidates to avoid overwhelming the system
        candidates.truncate(100);
        
        Ok(candidates)
    }
    
    /// Calculate priority score for cold migration
    fn calculate_cold_priority_score(&self, time_since_access: Duration, access_count: u32) -> f64 {
        let time_score = time_since_access.as_secs() as f64 / 3600.0; // Hours since access
        let frequency_score = 1.0 / (access_count as f64 + 1.0); // Lower frequency = higher score
        
        time_score * frequency_score
    }
    
    /// Execute migrations for candidates
    async fn execute_migrations(&self, candidates: Vec<MigrationCandidate>) -> Result<()> {
        let mut migration_count = 0;
        let max_migrations_per_cycle = 50; // Limit to avoid blocking
        
        for candidate in candidates.into_iter().take(max_migrations_per_cycle) {
            {
                let mut stats = self.stats.write().await;
                stats.migration_candidates_evaluated += 1;
            }
            
            let result = match candidate.item_type {
                ItemType::Node => self.migrate_node_to_cold(&candidate.item_id).await,
                ItemType::Edge => self.migrate_edge_to_cold(&candidate.item_id).await,
            };
            
            match result {
                Ok(()) => {
                    migration_count += 1;
                    tracing::debug!("Migrated {} to cold storage", candidate.item_id);
                },
                Err(e) => {
                    tracing::warn!("Failed to migrate {}: {:?}", candidate.item_id, e);
                    let mut stats = self.stats.write().await;
                    stats.migration_failures += 1;
                }
            }
        }
        
        if migration_count > 0 {
            let mut stats = self.stats.write().await;
            stats.cold_migrations += migration_count;
        }
        
        Ok(())
    }
    
    /// Migrate a node from hot to cold storage
    async fn migrate_node_to_cold(&self, node_id: &str) -> Result<()> {
        // Get node from hot tier
        if let Ok(Some(node)) = self.hot_tier.get_node(node_id) {
            // Store in cold tier
            self.cold_tier.store_node((*node).clone()).await?;
            
            // Remove from hot tier
            self.hot_tier.delete_node(node_id)?;
            
            Ok(())
        } else {
            Err(ProximaDBError::InternalError(
                format!("Node {} not found in hot tier", node_id)
            ))
        }
    }
    
    /// Migrate an edge from hot to cold storage
    async fn migrate_edge_to_cold(&self, edge_id: &str) -> Result<()> {
        // Get edge from hot tier
        if let Ok(Some(edge)) = self.hot_tier.get_edge(edge_id) {
            // Store in cold tier
            self.cold_tier.store_edge((*edge).clone()).await?;
            
            // Remove from hot tier
            self.hot_tier.delete_edge(edge_id)?;
            
            Ok(())
        } else {
            Err(ProximaDBError::InternalError(
                format!("Edge {} not found in hot tier", edge_id)
            ))
        }
    }
    
    /// Promote a node to hot tier
    pub async fn promote_to_hot(&self, node: &Node) -> Result<()> {
        // Check if hot tier has space
        let current_count = self.hot_tier.node_count()?;
        if current_count >= self.config.hot_tier_max_nodes {
            // Make space by migrating cold candidates
            self.migrate_cold_candidates().await?;
        }
        
        // Insert into hot tier
        self.hot_tier.insert_node(node.clone())?;
        
        // Remove from cold tier
        self.cold_tier.delete_node(&node.id).await?;
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.hot_promotions += 1;
        }
        
        tracing::debug!("Promoted node {} to hot tier", node.id);
        Ok(())
    }
    
    /// Migrate some items to cold storage to make space in hot tier
    pub async fn migrate_cold_candidates(&self) -> Result<()> {
        let candidates = self.get_migration_candidates().await?;
        
        // Take a few candidates to make space
        let space_needed = 10; // Migrate 10 items to make room
        let candidates_to_migrate = candidates.into_iter().take(space_needed).collect();
        
        self.execute_migrations(candidates_to_migrate).await
    }
    
    /// Update hot tier utilization statistics
    async fn update_hot_tier_utilization(&self) -> Result<()> {
        let current_nodes = self.hot_tier.node_count()? as f64;
        let max_nodes = self.config.hot_tier_max_nodes as f64;
        let utilization = current_nodes / max_nodes;
        
        {
            let mut stats = self.stats.write().await;
            stats.hot_tier_utilization = utilization;
        }
        
        Ok(())
    }
    
    /// Get tiering statistics
    pub async fn get_stats(&self) -> TieringStats {
        let stats = self.stats.read().await;
        TieringStats {
            cold_migrations: stats.cold_migrations,
            hot_promotions: stats.hot_promotions,
            total_migration_time_ms: stats.total_migration_time_ms,
            last_migration_cycle: stats.last_migration_cycle,
            migration_candidates_evaluated: stats.migration_candidates_evaluated,
            migration_failures: stats.migration_failures,
            hot_tier_utilization: stats.hot_tier_utilization,
        }
    }
    
    /// Manually trigger migration of specific items
    pub async fn manual_migrate(&self, movements: Vec<DataMovement>) -> Result<Vec<Result<()>>> {
        let mut results = Vec::new();
        
        for movement in movements {
            let result = match movement {
                DataMovement::DemoteNode(node_id) => {
                    self.migrate_node_to_cold(&node_id).await
                },
                DataMovement::DemoteEdge(edge_id) => {
                    self.migrate_edge_to_cold(&edge_id).await
                },
                DataMovement::PromoteNode(node_id) => {
                    // Need to get node from cold tier first
                    if let Ok(Some(node)) = self.cold_tier.get_node(&node_id).await {
                        self.promote_to_hot(&node).await
                    } else {
                        Err(ProximaDBError::InternalError(
                            format!("Node {} not found in cold tier", node_id)
                        ))
                    }
                },
                DataMovement::PromoteEdge(edge_id) => {
                    // Similar to node promotion, but for edges
                    if let Ok(Some(edge)) = self.cold_tier.get_edge(&edge_id).await {
                        self.hot_tier.insert_edge((*edge).clone())?;
                        self.cold_tier.delete_edge(&edge_id).await?;
                        Ok(())
                    } else {
                        Err(ProximaDBError::InternalError(
                            format!("Edge {} not found in cold tier", edge_id)
                        ))
                    }
                }
            };
            
            results.push(result);
        }
        
        Ok(results)
    }
    
    /// Get recommendations for manual tiering
    pub async fn get_tiering_recommendations(&self) -> Result<TieringRecommendations> {
        let hot_utilization = {
            let stats = self.stats.read().await;
            stats.hot_tier_utilization
        };
        
        let mut recommendations = TieringRecommendations {
            should_migrate_to_cold: Vec::new(),
            should_promote_to_hot: Vec::new(),
            hot_tier_utilization: hot_utilization,
            recommendation_reason: String::new(),
        };
        
        if hot_utilization > 0.9 {
            // Hot tier very full
            recommendations.recommendation_reason = "Hot tier is over 90% full. Consider migrating cold data.".to_string();
            
            // Get candidates for cold migration
            let cold_candidates = self.get_migration_candidates().await?;
            recommendations.should_migrate_to_cold = cold_candidates.into_iter()
                .take(20)
                .map(|c| c.item_id)
                .collect();
                
        } else if hot_utilization < 0.5 {
            // Hot tier has space, could promote some cold data
            recommendations.recommendation_reason = "Hot tier has available space. Consider promoting frequently accessed cold data.".to_string();
            
            // For now, leave promotion recommendations empty
            // In a real implementation, we'd analyze cold tier access patterns
        } else {
            recommendations.recommendation_reason = "Tiering is well balanced.".to_string();
        }
        
        Ok(recommendations)
    }
}

/// Tiering recommendations
#[derive(Debug)]
pub struct TieringRecommendations {
    pub should_migrate_to_cold: Vec<String>,
    pub should_promote_to_hot: Vec<String>,
    pub hot_tier_utilization: f64,
    pub recommendation_reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphMemoryPool;
    use crate::graph::engines::orion::OrionGraphEngine;
    use crate::proto::proximadb_v1::property_value::Value;
    use crate::graph::PropertyValue;
    use tempfile::TempDir;
    
    async fn create_test_setup() -> (TieringManager, Arc<OrionGraphEngine>) {
        let temp_dir = TempDir::new().unwrap();
        let hot_tier = Arc::new(OrionGraphEngine::new());
        
        let cold_tier = Arc::new(
            ColdStorageBackend::new(
                super::super::ColdStorageBackend::Json,
                temp_dir.path(),
            ).await.unwrap()
        );
        
        let access_cache = Arc::new(AccessPatternCache::new(1000));
        
        let config = QuasarConfig {
            hot_tier_max_nodes: 10,
            cold_migration_threshold: Duration::from_secs(60),
            ..QuasarConfig::default()
        };
        
        let manager = TieringManager::new(hot_tier.clone(), cold_tier, access_cache, config);
        
        (manager, hot_tier)
    }
    
    #[tokio::test]
    async fn test_tiering_manager_creation() {
        let (manager, _) = create_test_setup().await;
        
        let stats = manager.get_stats().await;
        assert_eq!(stats.cold_migrations, 0);
        assert_eq!(stats.hot_promotions, 0);
        assert_eq!(stats.hot_tier_utilization, 0.0);
    }
    
    #[tokio::test]
    async fn test_hot_tier_utilization_calculation() {
        let (manager, hot_tier) = create_test_setup().await;
        
        // Add some nodes to hot tier
        for i in 0..5 {
            let node = Node {
                id: format!("node_{}", i),
                labels: vec!["Test".to_string()],
                properties: std::collections::HashMap::new(),
                embedding: None,
                created_at: None,
                updated_at: None,
            };
            hot_tier.insert_node(node).unwrap();
        }
        
        // Update utilization
        manager.update_hot_tier_utilization().await.unwrap();
        
        let stats = manager.get_stats().await;
        assert_eq!(stats.hot_tier_utilization, 0.5); // 5/10 = 0.5
    }
    
    #[tokio::test]
    async fn test_migration_candidate_calculation() {
        let (manager, _) = create_test_setup().await;
        
        // With empty system, should have no candidates
        let candidates = manager.get_migration_candidates().await.unwrap();
        assert!(candidates.is_empty());
    }
    
    #[tokio::test]
    async fn test_priority_score_calculation() {
        let (manager, _) = create_test_setup().await;
        
        let score1 = manager.calculate_cold_priority_score(Duration::from_secs(3600), 1);
        let score2 = manager.calculate_cold_priority_score(Duration::from_secs(1800), 5);
        
        // Older access with fewer accesses should have higher priority
        assert!(score1 > score2);
    }
    
    #[tokio::test]
    async fn test_tiering_recommendations() {
        let (manager, hot_tier) = create_test_setup().await;
        
        // Fill up hot tier to trigger recommendations
        for i in 0..9 {
            let node = Node {
                id: format!("node_{}", i),
                labels: vec!["Test".to_string()],
                properties: std::collections::HashMap::new(),
                embedding: None,
                created_at: None,
                updated_at: None,
            };
            hot_tier.insert_node(node).unwrap();
        }
        
        manager.update_hot_tier_utilization().await.unwrap();
        
        let recommendations = manager.get_tiering_recommendations().await.unwrap();
        assert_eq!(recommendations.hot_tier_utilization, 0.9); // 9/10
        assert!(recommendations.recommendation_reason.contains("over 90%"));
    }
    
    #[test]
    fn test_migration_candidate_ordering() {
        let mut candidates = vec![
            MigrationCandidate {
                item_id: "low_priority".to_string(),
                item_type: ItemType::Node,
                priority_score: 0.1,
                last_access: Instant::now(),
                access_frequency: 10,
            },
            MigrationCandidate {
                item_id: "high_priority".to_string(),
                item_type: ItemType::Node,
                priority_score: 0.9,
                last_access: Instant::now(),
                access_frequency: 1,
            },
        ];
        
        candidates.sort_by(|a, b| b.priority_score.partial_cmp(&a.priority_score).unwrap_or(std::cmp::Ordering::Equal));
        
        assert_eq!(candidates[0].item_id, "high_priority");
        assert_eq!(candidates[1].item_id, "low_priority");
    }
}
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

//! # QUASAR Graph Engine - Hybrid Hot/Cold Storage
//!
//! QUASAR (Quantum Ultra-fast Storage with Adaptive Retrieval) is ProximaDB's
//! hybrid graph engine that automatically tiers data between hot (memory) and
//! cold (disk) storage based on access patterns for cost optimization.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │            QUASAR Engine                 │
//! ├─────────────────────────────────────────┤
//! │              Hot Tier                    │
//! │  ┌─────────────────────────────────────┐ │
//! │  │        ORION Engine                 │ │
//! │  │       (In-Memory CSR)               │ │
//! │  └─────────────────────────────────────┘ │
//! ├─────────────────────────────────────────┤
//! │             Tiering Logic                │
//! │  ┌─────────────────────────────────────┐ │
//! │  │      LRU Cache Manager              │ │
//! │  │    Access Pattern Tracker           │ │
//! │  │   Background Migration Worker       │ │
//! │  └─────────────────────────────────────┘ │
//! ├─────────────────────────────────────────┤
//! │              Cold Tier                   │
//! │  ┌─────────────────────────────────────┐ │
//! │  │      Disk Storage Backend           │ │
//! │  │       (SST/Parquet Files)           │ │
//! │  └─────────────────────────────────────┘ │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! - **Automatic Tiering**: Data moves between hot/cold based on access patterns
//! - **LRU Eviction**: Least recently used nodes/edges moved to cold storage
//! - **Transparent Access**: Applications access data transparently across tiers
//! - **Background Migration**: Asynchronous data movement doesn't block queries
//! - **Cost Optimization**: 80-90% storage cost savings for large, sparse graphs

pub mod cache;
pub mod storage_backend;
pub mod tiering;

use crate::core::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::engines::{GraphEngine, orion::OrionGraphEngine};
use crate::graph::{Edge, EdgeId, GraphMemoryPool, Node, NodeId};
use dashmap::DashMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::storage::cache::orchestrator::{CacheStatsProvider, CacheType, CrossCacheOrchestrator};
use tokio::time::{Duration, Instant};

/// QUASAR hybrid graph engine configuration
#[derive(Debug, Clone)]
pub struct QuasarConfig {
    /// Maximum size of hot tier (in number of nodes)
    pub hot_tier_max_nodes: usize,
    /// Maximum size of hot tier (in bytes)
    pub hot_tier_max_memory_mb: usize,
    /// Path for cold storage files
    pub cold_tier_path: PathBuf,
    /// Threshold for moving data to cold tier (access frequency)
    pub cold_migration_threshold: Duration,
    /// Threshold for promoting data to hot tier (access frequency)
    pub hot_promotion_threshold: Duration,
    /// Background migration interval
    pub migration_interval: Duration,
    /// Storage backend for cold tier
    pub cold_storage_backend: ColdStorageBackend,
}

/// Cold storage backend options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdStorageBackend {
    /// Use SST files for cold storage
    Sst,
    /// Use Parquet files for cold storage
    Parquet,
    /// Use simple JSON files (for testing)
    Json,
}

/// QUASAR hybrid graph engine
#[derive(Debug)]
pub struct QuasarGraphEngine {
    /// Engine configuration
    config: QuasarConfig,

    /// Hot tier (in-memory ORION engine)
    hot_tier: Arc<OrionGraphEngine>,

    /// Cold tier storage backend
    cold_tier: Arc<storage_backend::ColdStorageBackend>,

    /// Tiering manager for automatic data movement
    tiering_manager: Arc<tiering::TieringManager>,

    /// Access pattern cache for tracking usage
    access_cache: Arc<cache::AccessPatternCache>,

    /// Migration statistics
    stats: Arc<RwLock<QuasarStats>>,

    /// Background migration task handle
    migration_task: Option<tokio::task::JoinHandle<()>>,
}

/// QUASAR engine statistics
#[derive(Debug, Default, Clone)]
pub struct QuasarStats {
    pub hot_tier_nodes: u64,
    pub cold_tier_nodes: u64,
    pub hot_tier_edges: u64,
    pub cold_tier_edges: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub promotions_to_hot: u64,
    pub demotions_to_cold: u64,
    pub average_access_latency_ms: f64,
    pub storage_cost_savings_ratio: f64,
    pub migration_operations: u64,
}

impl Default for QuasarConfig {
    fn default() -> Self {
        Self {
            hot_tier_max_nodes: 100_000,
            hot_tier_max_memory_mb: 1024, // 1GB
            cold_tier_path: PathBuf::from("./quasar_cold"),
            cold_migration_threshold: Duration::from_secs(3600), // 1 hour
            hot_promotion_threshold: Duration::from_secs(300),   // 5 minutes
            migration_interval: Duration::from_secs(60),         // 1 minute
            cold_storage_backend: ColdStorageBackend::Sst,
        }
    }
}

impl QuasarGraphEngine {
    /// Create a new QUASAR hybrid graph engine
    pub async fn new(config: QuasarConfig) -> Result<Self> {
        // Initialize hot tier (ORION engine)
        let hot_tier = Arc::new(OrionGraphEngine::new());

        // Initialize cold storage backend
        let cold_tier = Arc::new(
            storage_backend::ColdStorageBackend::new(
                config.cold_storage_backend,
                &config.cold_tier_path,
            )
            .await?,
        );

        // Initialize access pattern cache
        let access_cache = Arc::new(cache::AccessPatternCache::new(
            config.hot_tier_max_nodes * 2, // Cache more access info than hot tier
        ));

        // Initialize tiering manager
        let tiering_manager = Arc::new(tiering::TieringManager::new(
            Arc::clone(&hot_tier),
            Arc::clone(&cold_tier),
            Arc::clone(&access_cache),
            config.clone(),
        ));

        let stats = Arc::new(RwLock::new(QuasarStats::default()));

        let mut engine = Self {
            config,
            hot_tier,
            cold_tier,
            tiering_manager,
            access_cache,
            stats,
            migration_task: None,
        };

        // Start background migration task
        engine.start_migration_task().await;

        // Register QUASAR access cache provider with orchestrator (GraphAdjacency)
        if let Some(orch) = CrossCacheOrchestrator::global() {
            let provider: Arc<dyn CacheStatsProvider + Send + Sync> =
                Arc::new(super::quasar::cache::QuasarAccessCacheStatsProvider::new(
                    engine.access_cache.clone(),
                ));
            orch.register_cache_provider(CacheType::GraphAdjacency, provider);
        }

        Ok(engine)
    }

    /// Start background migration task
    async fn start_migration_task(&mut self) {
        let tiering_manager = Arc::clone(&self.tiering_manager);
        let stats = Arc::clone(&self.stats);
        let migration_interval = self.config.migration_interval;

        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(migration_interval);

            loop {
                interval.tick().await;

                if let Err(e) = tiering_manager.perform_migration_cycle().await {
                    tracing::error!("Migration cycle failed: {:?}", e);
                    continue;
                }

                // Update migration stats
                {
                    let mut stats_guard = stats.write().await;
                    stats_guard.migration_operations += 1;
                }

                tracing::debug!("Completed migration cycle");
            }
        });

        self.migration_task = Some(task);
    }

    /// Get a node, checking hot tier first, then cold tier
    async fn get_node_from_tiers(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        let access_start = Instant::now();

        // Record access
        self.access_cache.record_access(id, access_start).await;

        // Check hot tier first
        if let Some(node) = self.hot_tier.get_node(id)? {
            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats.cache_hits += 1;
                self.update_access_latency(&mut stats, access_start.elapsed().as_millis() as f64);
            }

            return Ok(Some(node));
        }

        // Check cold tier
        if let Some(node) = self.cold_tier.get_node(id).await? {
            // Update stats
            {
                let mut stats = self.stats.write().await;
                stats.cache_misses += 1;
                self.update_access_latency(&mut stats, access_start.elapsed().as_millis() as f64);
            }

            // Consider promoting to hot tier if accessed frequently
            if self
                .access_cache
                .should_promote(id, self.config.hot_promotion_threshold)
                .await
            {
                if let Err(e) = self.tiering_manager.promote_to_hot(&node).await {
                    tracing::warn!("Failed to promote node {} to hot tier: {:?}", id, e);
                } else {
                    let mut stats = self.stats.write().await;
                    stats.promotions_to_hot += 1;
                }
            }

            return Ok(Some(node));
        }

        Ok(None)
    }

    /// Insert node into hot tier
    async fn insert_node_to_hot(&self, node: Node) -> Result<Arc<Node>> {
        let node_arc = self.hot_tier.insert_node(node)?;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.hot_tier_nodes += 1;
        }

        // Check if hot tier is getting full and needs migration
        if self.hot_tier.node_count()? > self.config.hot_tier_max_nodes {
            tokio::spawn({
                let tiering_manager = Arc::clone(&self.tiering_manager);
                async move {
                    if let Err(e) = tiering_manager.migrate_cold_candidates().await {
                        tracing::error!("Failed to migrate cold candidates: {:?}", e);
                    }
                }
            });
        }

        Ok(node_arc)
    }

    /// Get an edge, checking both tiers
    async fn get_edge_from_tiers(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        let access_start = Instant::now();

        // Check hot tier first
        if let Some(edge) = self.hot_tier.get_edge(id)? {
            let mut stats = self.stats.write().await;
            stats.cache_hits += 1;
            self.update_access_latency(&mut stats, access_start.elapsed().as_millis() as f64);
            return Ok(Some(edge));
        }

        // Check cold tier
        if let Some(edge) = self.cold_tier.get_edge(id).await? {
            let mut stats = self.stats.write().await;
            stats.cache_misses += 1;
            self.update_access_latency(&mut stats, access_start.elapsed().as_millis() as f64);
            return Ok(Some(edge));
        }

        Ok(None)
    }

    /// Insert edge into hot tier
    async fn insert_edge_to_hot(&self, edge: Edge) -> Result<Arc<Edge>> {
        let edge_arc = self.hot_tier.insert_edge(edge)?;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.hot_tier_edges += 1;
        }

        Ok(edge_arc)
    }

    /// Update access latency statistics
    fn update_access_latency(&self, stats: &mut QuasarStats, latency_ms: f64) {
        let total_accesses = stats.cache_hits + stats.cache_misses;
        if total_accesses == 1 {
            stats.average_access_latency_ms = latency_ms;
        } else {
            stats.average_access_latency_ms =
                (stats.average_access_latency_ms * (total_accesses - 1) as f64 + latency_ms)
                    / total_accesses as f64;
        }
    }

    /// Calculate storage cost savings
    async fn calculate_cost_savings(&self) -> f64 {
        let stats = self.stats.read().await;
        let total_nodes = stats.hot_tier_nodes + stats.cold_tier_nodes;

        if total_nodes == 0 {
            return 0.0;
        }

        // Assume cold storage costs 10% of hot storage
        let cold_ratio = stats.cold_tier_nodes as f64 / total_nodes as f64;
        cold_ratio * 0.9 // 90% savings on cold data
    }

    /// Get engine statistics
    pub async fn get_stats(&self) -> QuasarStats {
        let mut stats = {
            let guard = self.stats.read().await;
            guard.clone()
        };
        stats.storage_cost_savings_ratio = self.calculate_cost_savings().await;
        stats
    }

    /// Force migration cycle (for testing/maintenance)
    pub async fn force_migration(&self) -> Result<()> {
        self.tiering_manager.perform_migration_cycle().await
    }

    /// Get hot tier statistics
    pub async fn get_hot_tier_stats(&self) -> Result<crate::graph::engines::orion::EngineStats> {
        Ok(self.hot_tier.get_stats().await)
    }

    /// Get access pattern statistics
    pub async fn get_access_stats(&self) -> cache::AccessStats {
        self.access_cache.get_stats().await
    }
}

impl Drop for QuasarGraphEngine {
    fn drop(&mut self) {
        // Cancel background migration task
        if let Some(task) = self.migration_task.take() {
            task.abort();
        }
    }
}

impl GraphEngine for QuasarGraphEngine {
    fn insert_node(&self, node: Node) -> Result<Arc<Node>> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.insert_node_to_hot(node))
    }

    fn get_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.get_node_from_tiers(id))
    }

    fn update_node(&self, node: Node) -> Result<Arc<Node>> {
        let node_id = node.id.clone();

        // Try to update in hot tier first
        if self.hot_tier.get_node(&node_id)?.is_some() {
            return self.hot_tier.update_node(node);
        }

        // If not in hot tier, it might be in cold tier
        // For simplicity, insert into hot tier (it will be the "updated" version)
        let rt = tokio::runtime::Handle::current();
        let result = rt.block_on(self.insert_node_to_hot(node))?;

        // Remove from cold tier asynchronously
        tokio::spawn({
            let cold_tier = Arc::clone(&self.cold_tier);
            let node_id = node_id.clone();
            async move {
                if let Err(e) = cold_tier.delete_node(&node_id).await {
                    tracing::warn!("Failed to remove updated node from cold tier: {:?}", e);
                }
            }
        });

        Ok(result)
    }

    fn delete_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        // Try deleting from hot tier first
        if let Ok(Some(node)) = self.hot_tier.delete_node(id) {
            // Update stats
            tokio::spawn({
                let stats = Arc::clone(&self.stats);
                async move {
                    let mut stats = stats.write().await;
                    stats.hot_tier_nodes = stats.hot_tier_nodes.saturating_sub(1);
                }
            });

            return Ok(Some(node));
        }

        // Try cold tier
        let rt = tokio::runtime::Handle::current();
        let result = rt.block_on(async {
            if let Ok(Some(node)) = self.cold_tier.delete_node(id).await {
                // Update stats
                let mut stats = self.stats.write().await;
                stats.cold_tier_nodes = stats.cold_tier_nodes.saturating_sub(1);
                Ok(Some(node))
            } else {
                Ok(None)
            }
        });

        result
    }

    fn insert_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.insert_edge_to_hot(edge))
    }

    fn get_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.get_edge_from_tiers(id))
    }

    fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        let edge_id = edge.id.clone();

        // Similar logic to update_node
        if self.hot_tier.get_edge(&edge_id)?.is_some() {
            return self.hot_tier.update_edge(edge);
        }

        let rt = tokio::runtime::Handle::current();
        let result = rt.block_on(self.insert_edge_to_hot(edge))?;

        // Remove from cold tier asynchronously
        tokio::spawn({
            let cold_tier = Arc::clone(&self.cold_tier);
            let edge_id = edge_id.clone();
            async move {
                if let Err(e) = cold_tier.delete_edge(&edge_id).await {
                    tracing::warn!("Failed to remove updated edge from cold tier: {:?}", e);
                }
            }
        });

        Ok(result)
    }

    fn delete_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        // Try hot tier first
        if let Ok(Some(edge)) = self.hot_tier.delete_edge(id) {
            tokio::spawn({
                let stats = Arc::clone(&self.stats);
                async move {
                    let mut stats = stats.write().await;
                    stats.hot_tier_edges = stats.hot_tier_edges.saturating_sub(1);
                }
            });

            return Ok(Some(edge));
        }

        // Try cold tier
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            if let Ok(Some(edge)) = self.cold_tier.delete_edge(id).await {
                let mut stats = self.stats.write().await;
                stats.cold_tier_edges = stats.cold_tier_edges.saturating_sub(1);
                Ok(Some(edge))
            } else {
                Ok(None)
            }
        })
    }

    fn get_outgoing_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        // Check both tiers and combine results
        let mut edges = self.hot_tier.get_outgoing_edges(node_id, edge_type)?;

        let rt = tokio::runtime::Handle::current();
        if let Ok(cold_edges) = rt.block_on(self.cold_tier.get_outgoing_edges(node_id, edge_type)) {
            edges.extend(cold_edges);
        }

        Ok(edges)
    }

    fn get_incoming_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        // Check both tiers and combine results
        let mut edges = self.hot_tier.get_incoming_edges(node_id, edge_type)?;

        let rt = tokio::runtime::Handle::current();
        if let Ok(cold_edges) = rt.block_on(self.cold_tier.get_incoming_edges(node_id, edge_type)) {
            edges.extend(cold_edges);
        }

        Ok(edges)
    }

    fn get_neighbors(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Node>>> {
        // Get edges from both tiers
        let outgoing_edges = self.get_outgoing_edges(node_id, edge_type)?;
        let mut neighbors = Vec::new();

        for edge in outgoing_edges {
            if let Ok(Some(neighbor)) = self.get_node(&edge.to_node_id) {
                neighbors.push(neighbor);
            }
        }

        Ok(neighbors)
    }

    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
        // Check both tiers and combine results
        let mut nodes = self.hot_tier.get_nodes_by_label(label)?;

        let rt = tokio::runtime::Handle::current();
        if let Ok(cold_nodes) = rt.block_on(self.cold_tier.get_nodes_by_label(label)) {
            nodes.extend(cold_nodes);
        }

        Ok(nodes)
    }

    fn node_count(&self) -> Result<usize> {
        let hot_count = self.hot_tier.node_count()?;

        let rt = tokio::runtime::Handle::current();
        let cold_count = rt.block_on(async { self.cold_tier.node_count().await.unwrap_or(0) });

        Ok(hot_count + cold_count)
    }

    fn edge_count(&self) -> Result<usize> {
        let hot_count = self.hot_tier.edge_count()?;

        let rt = tokio::runtime::Handle::current();
        let cold_count = rt.block_on(async { self.cold_tier.edge_count().await.unwrap_or(0) });

        Ok(hot_count + cold_count)
    }

    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
        let mut all_nodes = self.hot_tier.get_all_nodes()?;

        let rt = tokio::runtime::Handle::current();
        let mut cold_nodes =
            rt.block_on(async { self.cold_tier.get_all_nodes().await.unwrap_or_default() });

        all_nodes.append(&mut cold_nodes);
        Ok(all_nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::PropertyValue;
    use crate::proto::proximadb_v1::property_value::Value;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_quasar_engine_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = QuasarConfig {
            cold_tier_path: temp_dir.path().to_path_buf(),
            ..QuasarConfig::default()
        };

        let engine = QuasarGraphEngine::new(config).await.unwrap();

        assert_eq!(engine.node_count().unwrap(), 0);
        assert_eq!(engine.edge_count().unwrap(), 0);

        let stats = engine.get_stats().await;
        assert_eq!(stats.hot_tier_nodes, 0);
        assert_eq!(stats.cold_tier_nodes, 0);
    }

    #[tokio::test]
    async fn test_basic_node_operations() {
        let temp_dir = TempDir::new().unwrap();
        let config = QuasarConfig {
            cold_tier_path: temp_dir.path().to_path_buf(),
            hot_tier_max_nodes: 5, // Small limit for testing
            ..QuasarConfig::default()
        };

        let engine = QuasarGraphEngine::new(config).await.unwrap();

        // Create test node
        let node = Node {
            id: "test_node".to_string(),
            labels: vec!["TestLabel".to_string()],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at: None,
            updated_at: None,
        };

        // Insert node
        let inserted = engine.insert_node(node).unwrap();
        assert_eq!(inserted.id, "test_node");

        // Give some time for async operations
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Get node
        let retrieved = engine.get_node("test_node").unwrap().unwrap();
        assert_eq!(retrieved.id, "test_node");

        // Verify stats
        let stats = engine.get_stats().await;
        assert_eq!(stats.hot_tier_nodes, 1);
        assert!(stats.cache_hits > 0);
    }

    #[tokio::test]
    async fn test_tiering_configuration() {
        let temp_dir = TempDir::new().unwrap();
        let config = QuasarConfig {
            cold_tier_path: temp_dir.path().to_path_buf(),
            hot_tier_max_nodes: 100,
            cold_migration_threshold: Duration::from_secs(10),
            hot_promotion_threshold: Duration::from_secs(5),
            ..QuasarConfig::default()
        };

        assert_eq!(config.hot_tier_max_nodes, 100);
        assert_eq!(config.cold_migration_threshold, Duration::from_secs(10));
        assert_eq!(config.hot_promotion_threshold, Duration::from_secs(5));
    }
}

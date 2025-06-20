// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Standard Collection Strategy
//!
//! Implements the standard strategy: HNSW + LSM + Standard Search
//! This is the default strategy for most collections.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use super::{
    CollectionStrategy, CollectionStrategyConfig, CompactionResult, FlushResult,
    OptimizationResult, SearchFilter, SearchResult, StrategyStats, StrategyType,
};
use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::lsm::LsmTree;
use crate::storage::{CollectionMetadata, WalEntry};

/// Standard strategy implementation
pub struct StandardStrategy {
    /// Strategy configuration
    config: CollectionStrategyConfig,
    /// LSM trees per collection
    lsm_trees: HashMap<CollectionId, Arc<LsmTree>>,
    /// HNSW indexes per collection
    hnsw_indexes: HashMap<CollectionId, Arc<HnswIndex>>,
    /// Search engines per collection
    search_engines: HashMap<CollectionId, Arc<StandardSearchEngine>>,
}

impl StandardStrategy {
    /// Create new standard strategy
    pub fn new(metadata: &CollectionMetadata) -> Result<Self> {
        let config = Self::create_config_from_metadata(metadata)?;

        Ok(Self {
            config,
            lsm_trees: HashMap::new(),
            hnsw_indexes: HashMap::new(),
            search_engines: HashMap::new(),
        })
    }

    /// Create strategy configuration from collection metadata
    fn create_config_from_metadata(
        metadata: &CollectionMetadata,
    ) -> Result<CollectionStrategyConfig> {
        let mut config = CollectionStrategyConfig::default();
        config.strategy_type = StrategyType::Standard;

        // Configure based on collection metadata
        if let Some(indexing_algorithm) = metadata.config.get("indexing_algorithm") {
            if let Some(algorithm_str) = indexing_algorithm.as_str() {
                match algorithm_str {
                    "hnsw" => {
                        config.indexing_config.algorithm = super::IndexingAlgorithm::HNSW {
                            m: metadata
                                .config
                                .get("hnsw_m")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(16) as usize,
                            ef_construction: metadata
                                .config
                                .get("hnsw_ef_construction")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(200)
                                as usize,
                            ef_search: metadata
                                .config
                                .get("hnsw_ef_search")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(50) as usize,
                        };
                    }
                    "ivf" => {
                        config.indexing_config.algorithm = super::IndexingAlgorithm::IVF {
                            nlist: metadata
                                .config
                                .get("ivf_nlist")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(1024) as usize,
                            nprobe: metadata
                                .config
                                .get("ivf_nprobe")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(16) as usize,
                        };
                    }
                    "flat" => {
                        config.indexing_config.algorithm = super::IndexingAlgorithm::Flat;
                    }
                    _ => {} // Keep default HNSW
                }
            }
        }

        // Configure distance metric
        if let Some(distance_metric) = metadata.config.get("distance_metric") {
            if let Some(metric_str) = distance_metric.as_str() {
                config.search_config.distance_metric = match metric_str {
                    "cosine" => super::DistanceMetric::Cosine,
                    "euclidean" => super::DistanceMetric::Euclidean,
                    "dot_product" => super::DistanceMetric::DotProduct,
                    "manhattan" => super::DistanceMetric::Manhattan,
                    _ => super::DistanceMetric::Cosine, // Default
                };
            }
        }

        Ok(config)
    }
}

#[async_trait]
impl CollectionStrategy for StandardStrategy {
    fn strategy_name(&self) -> &'static str {
        "standard"
    }

    fn get_config(&self) -> CollectionStrategyConfig {
        self.config.clone()
    }

    async fn initialize(
        &mut self,
        collection_id: CollectionId,
        metadata: &CollectionMetadata,
    ) -> Result<()> {
        tracing::info!(
            "🚀 Initializing standard strategy for collection: {}",
            collection_id
        );

        // Create LSM tree for storage
        let lsm_tree = Arc::new(LsmTree::new_for_strategy(
            &collection_id,
            &self.config.storage_config,
        )?);
        self.lsm_trees.insert(collection_id.clone(), lsm_tree);

        // Create HNSW index
        let hnsw_index = Arc::new(HnswIndex::new(
            metadata.dimension,
            &self.config.indexing_config,
        )?);
        self.hnsw_indexes.insert(collection_id.clone(), hnsw_index);

        // Create search engine
        let search_engine = Arc::new(StandardSearchEngine::new(&self.config.search_config)?);
        self.search_engines
            .insert(collection_id.clone(), search_engine);

        tracing::info!(
            "✅ Standard strategy initialized for collection: {}",
            collection_id
        );
        Ok(())
    }

    async fn flush_entries(
        &self,
        collection_id: &CollectionId,
        entries: Vec<WalEntry>,
    ) -> Result<FlushResult> {
        let start_time = std::time::Instant::now();
        tracing::debug!(
            "🔄 Standard strategy flushing {} entries for collection: {}",
            entries.len(),
            collection_id
        );

        let lsm_tree = self.lsm_trees.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("LSM tree not found for collection: {}", collection_id)
        })?;

        let hnsw_index = self.hnsw_indexes.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("HNSW index not found for collection: {}", collection_id)
        })?;

        let mut bytes_written = 0u64;
        let mut entries_processed = 0usize;

        // Process each WAL entry according to its operation type
        for entry in entries {
            match &entry.operation {
                crate::storage::WalOperation::Insert {
                    vector_id, record, ..
                } => {
                    // Write to LSM tree (persistent storage)
                    lsm_tree
                        .put(vector_id.clone(), record.clone())
                        .await
                        .context("Failed to write to LSM tree")?;

                    // Add to HNSW index (for fast search)
                    hnsw_index
                        .add_vector(vector_id.clone(), &record.vector)
                        .await
                        .context("Failed to add to HNSW index")?;

                    bytes_written += record.vector.len() as u64 * 4; // f32 = 4 bytes
                    entries_processed += 1;
                }
                crate::storage::WalOperation::Update {
                    vector_id, record, ..
                } => {
                    // Update in LSM tree
                    lsm_tree
                        .put(vector_id.clone(), record.clone())
                        .await
                        .context("Failed to update in LSM tree")?;

                    // Update in HNSW index
                    hnsw_index
                        .update_vector(vector_id.clone(), &record.vector)
                        .await
                        .context("Failed to update in HNSW index")?;

                    bytes_written += record.vector.len() as u64 * 4;
                    entries_processed += 1;
                }
                crate::storage::WalOperation::Delete { vector_id, .. } => {
                    // Mark as deleted in LSM tree (tombstone)
                    lsm_tree
                        .delete(vector_id.clone())
                        .await
                        .context("Failed to delete from LSM tree")?;

                    // Remove from HNSW index
                    hnsw_index
                        .remove_vector(vector_id.clone())
                        .await
                        .context("Failed to remove from HNSW index")?;

                    entries_processed += 1;
                }
                _ => {
                    // Skip non-vector operations in standard strategy
                }
            }
        }

        let flush_time_ms = start_time.elapsed().as_millis() as u64;

        let mut strategy_metadata = HashMap::new();
        strategy_metadata.insert(
            "strategy_type".to_string(),
            serde_json::Value::String("standard".to_string()),
        );
        strategy_metadata.insert(
            "lsm_flush_count".to_string(),
            serde_json::Value::Number(entries_processed.into()),
        );
        strategy_metadata.insert(
            "hnsw_updates".to_string(),
            serde_json::Value::Number(entries_processed.into()),
        );

        tracing::debug!(
            "✅ Standard strategy flush completed for collection: {} in {}ms",
            collection_id,
            flush_time_ms
        );

        Ok(FlushResult {
            entries_flushed: entries_processed,
            bytes_written,
            flush_time_ms,
            strategy_metadata,
        })
    }

    async fn compact_collection(&self, collection_id: &CollectionId) -> Result<CompactionResult> {
        let start_time = std::time::Instant::now();
        tracing::debug!(
            "🔄 Standard strategy compacting collection: {}",
            collection_id
        );

        let lsm_tree = self.lsm_trees.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("LSM tree not found for collection: {}", collection_id)
        })?;

        // Trigger LSM compaction
        let compaction_stats = lsm_tree.compact().await?;

        let compaction_time_ms = start_time.elapsed().as_millis() as u64;

        let mut efficiency_metrics = HashMap::new();
        efficiency_metrics.insert(
            "compression_ratio".to_string(),
            compaction_stats.compression_ratio,
        );
        efficiency_metrics.insert(
            "read_amplification".to_string(),
            compaction_stats.read_amplification,
        );
        efficiency_metrics.insert(
            "write_amplification".to_string(),
            compaction_stats.write_amplification,
        );

        tracing::debug!(
            "✅ Standard strategy compaction completed for collection: {} in {}ms",
            collection_id,
            compaction_time_ms
        );

        Ok(CompactionResult {
            entries_compacted: compaction_stats.entries_compacted,
            space_reclaimed: compaction_stats.space_reclaimed,
            compaction_time_ms,
            efficiency_metrics,
        })
    }

    async fn search_vectors(
        &self,
        collection_id: &CollectionId,
        query: Vec<f32>,
        k: usize,
        filter: Option<SearchFilter>,
    ) -> Result<Vec<SearchResult>> {
        tracing::debug!(
            "🔍 Standard strategy searching collection: {} with k={}",
            collection_id,
            k
        );

        let hnsw_index = self.hnsw_indexes.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("HNSW index not found for collection: {}", collection_id)
        })?;

        let search_engine = self.search_engines.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("Search engine not found for collection: {}", collection_id)
        })?;

        // Use HNSW for approximate nearest neighbor search
        let candidates = hnsw_index.search(&query, k * 2).await?; // Get more candidates for filtering

        // Apply filters and get final results
        let results = search_engine.filter_and_rank(candidates, filter).await?;

        // Limit to requested k results
        Ok(results.into_iter().take(k).collect())
    }

    async fn add_vector(
        &self,
        collection_id: &CollectionId,
        vector_record: &VectorRecord,
    ) -> Result<()> {
        let hnsw_index = self.hnsw_indexes.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("HNSW index not found for collection: {}", collection_id)
        })?;

        hnsw_index
            .add_vector(vector_record.id.clone(), &vector_record.vector)
            .await
    }

    async fn update_vector(
        &self,
        collection_id: &CollectionId,
        vector_record: &VectorRecord,
    ) -> Result<()> {
        let hnsw_index = self.hnsw_indexes.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("HNSW index not found for collection: {}", collection_id)
        })?;

        hnsw_index
            .update_vector(vector_record.id.clone(), &vector_record.vector)
            .await
    }

    async fn remove_vector(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<bool> {
        let hnsw_index = self.hnsw_indexes.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("HNSW index not found for collection: {}", collection_id)
        })?;

        hnsw_index.remove_vector(vector_id.clone()).await
    }

    async fn get_stats(&self, collection_id: &CollectionId) -> Result<StrategyStats> {
        let lsm_tree = self.lsm_trees.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("LSM tree not found for collection: {}", collection_id)
        })?;

        let hnsw_index = self.hnsw_indexes.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("HNSW index not found for collection: {}", collection_id)
        })?;

        let lsm_stats = lsm_tree.get_stats().await?;
        let hnsw_stats = hnsw_index.get_stats().await?;

        Ok(StrategyStats {
            strategy_type: StrategyType::Standard,
            total_operations: lsm_stats.total_operations + hnsw_stats.total_operations,
            avg_latency_ms: (lsm_stats.avg_latency_ms + hnsw_stats.avg_latency_ms) / 2.0,
            index_build_time_ms: hnsw_stats.build_time_ms,
            search_accuracy: hnsw_stats.search_accuracy,
            memory_usage_bytes: lsm_stats.memory_usage + hnsw_stats.memory_usage,
            storage_efficiency: lsm_stats.compression_ratio,
        })
    }

    async fn optimize_collection(
        &self,
        collection_id: &CollectionId,
    ) -> Result<OptimizationResult> {
        let start_time = std::time::Instant::now();
        tracing::debug!(
            "🔧 Standard strategy optimizing collection: {}",
            collection_id
        );

        // Get current metrics
        let before_stats = self.get_stats(collection_id).await?;
        let mut before_metrics = HashMap::new();
        before_metrics.insert("avg_latency_ms".to_string(), before_stats.avg_latency_ms);
        before_metrics.insert("search_accuracy".to_string(), before_stats.search_accuracy);
        before_metrics.insert(
            "storage_efficiency".to_string(),
            before_stats.storage_efficiency,
        );

        // Optimize HNSW index
        let hnsw_index = self.hnsw_indexes.get(collection_id).ok_or_else(|| {
            anyhow::anyhow!("HNSW index not found for collection: {}", collection_id)
        })?;

        hnsw_index.optimize().await?;

        // Trigger LSM compaction
        self.compact_collection(collection_id).await?;

        // Get optimized metrics
        let after_stats = self.get_stats(collection_id).await?;
        let mut after_metrics = HashMap::new();
        after_metrics.insert("avg_latency_ms".to_string(), after_stats.avg_latency_ms);
        after_metrics.insert("search_accuracy".to_string(), after_stats.search_accuracy);
        after_metrics.insert(
            "storage_efficiency".to_string(),
            after_stats.storage_efficiency,
        );

        let optimization_time_ms = start_time.elapsed().as_millis() as u64;
        let improvement_ratio = before_stats.avg_latency_ms / after_stats.avg_latency_ms;

        tracing::debug!("✅ Standard strategy optimization completed for collection: {} in {}ms, improvement: {:.2}x", 
                       collection_id, optimization_time_ms, improvement_ratio);

        Ok(OptimizationResult {
            improvement_ratio,
            optimization_time_ms,
            optimization_type: "hnsw_rebuild_and_lsm_compaction".to_string(),
            before_metrics,
            after_metrics,
        })
    }
}

// Placeholder types - these would be implemented as separate modules
struct HnswIndex;
struct StandardSearchEngine;

impl HnswIndex {
    fn new(_dimension: usize, _config: &super::IndexingConfig) -> Result<Self> {
        // TODO: Implement HNSW index
        Ok(Self)
    }

    async fn add_vector(&self, _id: VectorId, _vector: &[f32]) -> Result<()> {
        // TODO: Implement vector addition
        Ok(())
    }

    async fn update_vector(&self, _id: VectorId, _vector: &[f32]) -> Result<()> {
        // TODO: Implement vector update
        Ok(())
    }

    async fn remove_vector(&self, _id: VectorId) -> Result<bool> {
        // TODO: Implement vector removal
        Ok(true)
    }

    async fn search(&self, _query: &[f32], _k: usize) -> Result<Vec<(VectorId, f32)>> {
        // TODO: Implement HNSW search
        Ok(vec![])
    }

    async fn get_stats(&self) -> Result<HnswStats> {
        // TODO: Implement stats collection
        Ok(HnswStats::default())
    }

    async fn optimize(&self) -> Result<()> {
        // TODO: Implement index optimization
        Ok(())
    }
}

impl StandardSearchEngine {
    fn new(_config: &super::SearchConfig) -> Result<Self> {
        // TODO: Implement search engine
        Ok(Self)
    }

    async fn filter_and_rank(
        &self,
        _candidates: Vec<(VectorId, f32)>,
        _filter: Option<SearchFilter>,
    ) -> Result<Vec<SearchResult>> {
        // TODO: Implement filtering and ranking
        Ok(vec![])
    }
}

#[derive(Default)]
struct HnswStats {
    total_operations: u64,
    avg_latency_ms: f64,
    build_time_ms: u64,
    search_accuracy: f64,
    memory_usage: u64,
}

#[derive(Default)]
struct LsmStats {
    total_operations: u64,
    avg_latency_ms: f64,
    memory_usage: u64,
    compression_ratio: f64,
}

struct CompactionStats {
    entries_compacted: usize,
    space_reclaimed: u64,
    compression_ratio: f64,
    read_amplification: f64,
    write_amplification: f64,
}

// Extension trait for LsmTree to support strategy pattern
trait LsmTreeStrategy {
    fn new_for_strategy(
        collection_id: &CollectionId,
        config: &super::StorageConfig,
    ) -> Result<LsmTree>;
    async fn get_stats(&self) -> Result<LsmStats>;
    async fn compact(&self) -> Result<CompactionStats>;
}

impl LsmTreeStrategy for LsmTree {
    fn new_for_strategy(
        _collection_id: &CollectionId,
        _config: &super::StorageConfig,
    ) -> Result<LsmTree> {
        // TODO: Create LSM tree with strategy-specific configuration
        todo!("Implement LSM tree creation for strategy")
    }

    async fn get_stats(&self) -> Result<LsmStats> {
        // TODO: Implement LSM tree stats
        Ok(LsmStats::default())
    }

    async fn compact(&self) -> Result<CompactionStats> {
        // TODO: Implement LSM tree compaction
        Ok(CompactionStats {
            entries_compacted: 0,
            space_reclaimed: 0,
            compression_ratio: 1.0,
            read_amplification: 1.0,
            write_amplification: 1.0,
        })
    }
}

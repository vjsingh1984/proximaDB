// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Collection Strategy Migration Service
//! 
//! Since data only exists in WAL and no actual storage/indexes exist yet,
//! migration is simply updating collection metadata to change how future
//! operations (flush, indexing, search) will behave.

use anyhow::{Result, Context};
use chrono::Utc;
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::core::CollectionId;
use crate::storage::{
    MetadataStore,
    metadata::{StrategyChangeStatus, StrategyChangeType},
    strategy::{CollectionStrategyConfig, StorageEngineType, IndexingAlgorithm, DistanceMetric},
};

/// Migration service for updating collection strategies
#[derive(Clone)]
pub struct MigrationService {
    metadata_store: Arc<MetadataStore>,
}

/// Strategy migration request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyMigrationRequest {
    /// Collection to migrate
    pub collection_id: CollectionId,
    
    /// New storage engine (optional - keep current if None)
    pub storage_engine: Option<StorageEngineType>,
    
    /// New indexing algorithm (optional - keep current if None)
    pub indexing_algorithm: Option<IndexingAlgorithm>,
    
    /// New search/distance metric (optional - keep current if None)
    pub distance_metric: Option<DistanceMetric>,
    
    /// Additional indexing parameters
    pub indexing_parameters: Option<std::collections::HashMap<String, serde_json::Value>>,
    
    /// Additional storage parameters
    pub storage_parameters: Option<std::collections::HashMap<String, serde_json::Value>>,
    
    /// Additional search parameters
    pub search_parameters: Option<std::collections::HashMap<String, serde_json::Value>>,
    
    /// User who initiated the migration
    pub initiated_by: Option<String>,
    
    /// Reason for migration
    pub reason: Option<String>,
}

/// Migration response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyMigrationResponse {
    /// Migration was successful
    pub success: bool,
    
    /// Change ID for tracking
    pub change_id: String,
    
    /// Previous strategy configuration
    pub previous_strategy: CollectionStrategyConfig,
    
    /// New strategy configuration  
    pub new_strategy: CollectionStrategyConfig,
    
    /// What was changed
    pub change_type: StrategyChangeType,
    
    /// Timestamp of change
    pub changed_at: chrono::DateTime<Utc>,
    
    /// Error message if migration failed
    pub error_message: Option<String>,
}

impl MigrationService {
    /// Create new migration service
    pub fn new(metadata_store: Arc<MetadataStore>) -> Self {
        Self { metadata_store }
    }
    
    /// Migrate collection strategy (immediate metadata update)
    pub async fn migrate_collection_strategy(&self, request: StrategyMigrationRequest) -> Result<StrategyMigrationResponse> {
        tracing::info!("🔄 Starting strategy migration for collection: {}", request.collection_id);
        
        // Get current collection metadata
        let mut metadata = self.metadata_store.get_collection(&request.collection_id).await?
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", request.collection_id))?;
        
        let previous_strategy = metadata.strategy_config.clone();
        let mut new_strategy = previous_strategy.clone();
        
        // Determine what's changing
        let mut changes = Vec::new();
        
        // Update storage engine if requested
        if let Some(storage_engine) = request.storage_engine {
            if new_strategy.storage_config.engine_type != storage_engine {
                new_strategy.storage_config.engine_type = storage_engine;
                changes.push("storage");
            }
            
            // Update storage parameters if provided
            if let Some(params) = request.storage_parameters {
                new_strategy.storage_config.parameters.extend(params);
            }
        }
        
        // Update indexing algorithm if requested
        if let Some(indexing_algorithm) = request.indexing_algorithm {
            if new_strategy.indexing_config.algorithm != indexing_algorithm {
                new_strategy.indexing_config.algorithm = indexing_algorithm;
                changes.push("indexing");
            }
            
            // Update indexing parameters if provided
            if let Some(params) = request.indexing_parameters {
                new_strategy.indexing_config.parameters.extend(params);
            }
        }
        
        // Update distance metric if requested
        if let Some(distance_metric) = request.distance_metric {
            if new_strategy.search_config.distance_metric != distance_metric {
                new_strategy.search_config.distance_metric = distance_metric;
                changes.push("search");
            }
            
            // Update search parameters if provided
            if let Some(params) = request.search_parameters {
                new_strategy.search_config.parameters.extend(params);
            }
        }
        
        // Determine change type
        let change_type = self.determine_change_type(&changes);
        
        if changes.is_empty() {
            return Ok(StrategyMigrationResponse {
                success: true,
                change_id: "no-change".to_string(),
                previous_strategy,
                new_strategy,
                change_type: StrategyChangeType::SearchOnly, // Placeholder
                changed_at: Utc::now(),
                error_message: Some("No changes requested".to_string()),
            });
        }
        
        // Update strategy configuration
        metadata.strategy_config = new_strategy.clone();
        metadata.updated_at = Utc::now();
        
        // Create change tracking record
        let change_id = Uuid::new_v4().to_string();
        let change_status = StrategyChangeStatus {
            change_id: change_id.clone(),
            changed_at: Utc::now(),
            previous_strategy: previous_strategy.clone(),
            current_strategy: new_strategy.clone(),
            change_type: change_type.clone(),
            changed_by: request.initiated_by,
            change_reason: request.reason,
        };
        
        // Add to change history
        metadata.strategy_change_history.push(change_status);
        
        // Update metadata store
        self.metadata_store.update_collection(&request.collection_id, metadata).await
            .context("Failed to update collection metadata")?;
        
        tracing::info!("✅ Strategy migration completed for collection: {} (change_id: {})", 
                      request.collection_id, change_id);
        
        Ok(StrategyMigrationResponse {
            success: true,
            change_id,
            previous_strategy,
            new_strategy,
            change_type,
            changed_at: Utc::now(),
            error_message: None,
        })
    }
    
    /// Get migration history for a collection
    pub async fn get_migration_history(&self, collection_id: &CollectionId) -> Result<Vec<StrategyChangeStatus>> {
        let metadata = self.metadata_store.get_collection(collection_id).await?
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;
        
        Ok(metadata.strategy_change_history)
    }
    
    /// Get current strategy for a collection
    pub async fn get_current_strategy(&self, collection_id: &CollectionId) -> Result<CollectionStrategyConfig> {
        let metadata = self.metadata_store.get_collection(collection_id).await?
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;
        
        Ok(metadata.strategy_config)
    }
    
    /// Determine what type of change is being made
    fn determine_change_type(&self, changes: &[&str]) -> StrategyChangeType {
        let has_storage = changes.contains(&"storage");
        let has_indexing = changes.contains(&"indexing");
        let has_search = changes.contains(&"search");
        
        match (has_storage, has_indexing, has_search) {
            (true, true, true) => StrategyChangeType::Complete,
            (true, true, false) => StrategyChangeType::StorageAndIndexing,
            (true, false, true) => StrategyChangeType::StorageAndSearch,
            (false, true, true) => StrategyChangeType::IndexingAndSearch,
            (true, false, false) => StrategyChangeType::StorageOnly,
            (false, true, false) => StrategyChangeType::IndexingOnly,
            (false, false, true) => StrategyChangeType::SearchOnly,
            (false, false, false) => StrategyChangeType::SearchOnly, // Default fallback
        }
    }
}

/// Predefined strategy templates for common use cases
pub struct StrategyTemplates;

impl StrategyTemplates {
    /// Standard performance strategy
    pub fn standard_performance() -> CollectionStrategyConfig {
        let mut config = CollectionStrategyConfig::default();
        config.indexing_config.algorithm = IndexingAlgorithm::HNSW {
            m: 16,
            ef_construction: 200,
            ef_search: 50,
        };
        config.storage_config.engine_type = StorageEngineType::LSM;
        config.search_config.distance_metric = DistanceMetric::Cosine;
        config
    }
    
    /// High-performance strategy for large-scale workloads
    pub fn high_performance() -> CollectionStrategyConfig {
        let mut config = CollectionStrategyConfig::default();
        config.indexing_config.algorithm = IndexingAlgorithm::IVF {
            nlist: 4096,
            nprobe: 32,
        };
        config.storage_config.engine_type = StorageEngineType::LSM;
        config.search_config.distance_metric = DistanceMetric::DotProduct;
        config.performance_config.enable_simd = true;
        config.performance_config.enable_gpu = true;
        config
    }
    
    /// VIPER strategy for ML-optimized workloads
    pub fn viper_optimized() -> CollectionStrategyConfig {
        let mut config = CollectionStrategyConfig::default();
        config.indexing_config.algorithm = IndexingAlgorithm::PQ {
            m: 16,
            nbits: 8,
        };
        config.storage_config.engine_type = StorageEngineType::VIPER;
        config.search_config.distance_metric = DistanceMetric::Cosine;
        config.search_config.enable_optimization = true;
        config
    }
    
    /// Memory-optimized strategy for resource-constrained environments
    pub fn memory_optimized() -> CollectionStrategyConfig {
        let mut config = CollectionStrategyConfig::default();
        config.indexing_config.algorithm = IndexingAlgorithm::PQ {
            m: 8,
            nbits: 4,
        };
        config.storage_config.engine_type = StorageEngineType::LSM;
        config.performance_config.memory_limit_mb = 256;
        config.performance_config.batch_config.batch_size = 100;
        config
    }
}
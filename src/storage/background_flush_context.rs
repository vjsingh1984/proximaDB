// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Background Flush Context - Pre-computed Collection Metadata
//!
//! Eliminates redundant collection service calls by pre-computing all needed metadata
//! for background flush and compaction operations.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::compute::distance_computation::DistanceMetric;
use crate::services::collection_service::CollectionService;
use crate::proto::proximadb::{Collection, FilterableColumnSpec};

/// Storage engine types supported by ProximaDB
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageEngineType {
    /// VIPER - Columnar Parquet-based engine for analytics workloads
    Viper,
    /// SST - Row-based SSTable engine for OLTP workloads  
    Sst,
}

impl TryFrom<i32> for StorageEngineType {
    type Error = anyhow::Error;
    
    fn try_from(value: i32) -> Result<Self> {
        match value {
            x if x == crate::proto::proximadb::StorageEngine::Viper as i32 => Ok(StorageEngineType::Viper),
            x if x == crate::proto::proximadb::StorageEngine::Sst as i32 => Ok(StorageEngineType::Sst),
            _ => Err(anyhow!("Unknown storage engine type: {}", value)),
        }
    }
}

/// Compression configuration for storage engines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    pub enabled: bool,
    pub compression_type: String, // "zstd", "snappy", "lz4", "gzip"
    pub level: i32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            compression_type: "zstd".to_string(),
            level: 3, // Balanced CPU/IO performance
        }
    }
}

/// Quantization configuration for vector compression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationConfig {
    pub enabled: bool,
    pub quantization_type: String, // "product", "scalar", "binary"
    pub bits_per_component: u8,
    pub subspaces: Option<u8>,
}

/// Operation priority for background tasks
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationPriority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for OperationPriority {
    fn default() -> Self {
        OperationPriority::Normal
    }
}

/// Pre-computed context containing ALL metadata needed for background flush/compaction
/// 
/// This eliminates the need for background threads to make collection service calls,
/// improving performance and reliability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundFlushContext {
    // === Core Identification ===
    /// Collection ID being processed
    pub collection_id: String,
    
    // === Engine Configuration ===
    /// Storage engine type (VIPER vs SST)
    pub storage_engine: StorageEngineType,
    /// Base storage location path (e.g., "file:///data/disk1")
    pub base_location: String,
    
    // === Vector Configuration ===
    /// Vector dimension size
    pub dimension: usize,
    /// Distance metric for similarity calculations
    pub distance_metric: DistanceMetric,
    
    // === Storage Configuration ===
    /// Compression settings for storage engine
    pub compression_config: CompressionConfig,
    
    // === Schema Configuration ===
    /// Filterable metadata columns with their types
    pub filterable_columns: Vec<FilterableColumnSpec>,
    
    // === Performance Configuration ===
    /// Vector quantization settings (if enabled)
    pub quantization_config: Option<QuantizationConfig>,
    /// Suggested batch size for operations
    pub batch_size_hint: Option<usize>,
    
    // === Operational Configuration ===
    /// Priority level for background operations
    pub priority: OperationPriority,
    /// Timeout for operations in milliseconds
    pub timeout_ms: Option<u64>,
    /// Additional metadata for future extensions
    pub extra_metadata: HashMap<String, String>,
}

impl BackgroundFlushContext {
    /// Create context from collection service (eliminates future service calls)
    pub async fn from_collection_service(
        service: &CollectionService,
        collection_id: &str,
    ) -> Result<Self> {
        // Single collection service call - all subsequent operations use this context
        let collection = service
            .get_proto_collection(collection_id)
            .await
            .context("Failed to fetch collection from service")?
            .ok_or_else(|| anyhow!("Collection '{}' not found", collection_id))?;
        
        let config = collection
            .config
            .as_ref()
            .ok_or_else(|| anyhow!("Collection '{}' has no configuration", collection_id))?;
        
        let storage_assignment = collection
            .storage_assignment
            .as_ref()
            .ok_or_else(|| anyhow!("Collection '{}' has no storage assignment", collection_id))?;
        
        // Parse storage engine type
        let storage_engine = StorageEngineType::try_from(config.storage_engine)
            .context("Failed to parse storage engine type")?;
        
        // Parse distance metric
        let distance_metric = DistanceMetric::try_from(config.distance_metric)
            .context("Failed to parse distance metric")?;
        
        // Create compression config based on storage engine defaults
        let compression_config = match storage_engine {
            StorageEngineType::Viper => CompressionConfig {
                enabled: true,
                compression_type: "zstd".to_string(),
                level: 3, // Balanced for Parquet
            },
            StorageEngineType::Sst => CompressionConfig {
                enabled: true,
                compression_type: "snappy".to_string(),
                level: 1, // Fast compression for OLTP
            },
        };
        
        // Parse quantization config if present
        let quantization_config = config.quantization_config.as_ref().map(|qc| {
            // Extract quantization type from the QuantizationLevel oneof field
            let quantization_type = if let Some(ref storage_quant) = qc.storage_quantization {
                if let Some(ref level) = storage_quant.level {
                    match level.level_type {
                        Some(crate::proto::proximadb::quantization_level::LevelType::Pq(_)) => "product",
                        Some(crate::proto::proximadb::quantization_level::LevelType::Scalar(_)) => "scalar", 
                        Some(crate::proto::proximadb::quantization_level::LevelType::Binary(_)) => "binary",
                        Some(crate::proto::proximadb::quantization_level::LevelType::Uniform(_)) => "uniform",
                        _ => "product", // Default fallback
                    }
                } else {
                    "product" // Default if no level specified
                }
            } else {
                "product" // Default if no storage quantization
            };
            
            QuantizationConfig {
                enabled: qc.enabled,
                quantization_type: quantization_type.to_string(),
                bits_per_component: 8, // Default - could be extracted from specific quantization types
                subspaces: Some(8), // Default - could be extracted from ProductQuantization config
            }
        });
        
        // Determine batch size hint based on dimension and engine
        let batch_size_hint = match storage_engine {
            StorageEngineType::Viper => Some(1000.min(10000 / (config.dimension as usize / 100).max(1))),
            StorageEngineType::Sst => Some(500.min(5000 / (config.dimension as usize / 100).max(1))),
        };
        
        Ok(Self {
            collection_id: collection_id.to_string(),
            storage_engine,
            base_location: storage_assignment.base_location.clone(),
            dimension: config.dimension as usize,
            distance_metric,
            compression_config,
            filterable_columns: config.filterable_columns.clone(),
            quantization_config,
            batch_size_hint,
            priority: OperationPriority::Normal,
            timeout_ms: Some(300_000), // 5 minutes default
            extra_metadata: HashMap::new(),
        })
    }
    
    /// Create context for testing with minimal required fields
    pub fn for_testing(collection_id: &str, storage_engine: StorageEngineType) -> Self {
        Self {
            collection_id: collection_id.to_string(),
            storage_engine,
            base_location: format!("file:///tmp/test_data"),
            dimension: 384,
            distance_metric: DistanceMetric::Cosine,
            compression_config: CompressionConfig::default(),
            filterable_columns: Vec::new(),
            quantization_config: None,
            batch_size_hint: Some(1000),
            priority: OperationPriority::Normal,
            timeout_ms: Some(60_000),
            extra_metadata: HashMap::new(),
        }
    }
    
    /// Get engine name as string (for compatibility with existing code)
    pub fn engine_name(&self) -> &str {
        match self.storage_engine {
            StorageEngineType::Viper => "viper",
            StorageEngineType::Sst => "sst",
        }
    }
    
    /// Get suggested row group size for Parquet (VIPER engine)
    pub fn row_group_size(&self) -> usize {
        match self.storage_engine {
            StorageEngineType::Viper => {
                // Balance between compression efficiency and memory usage
                let base_size = 10_000;
                let dimension_factor = (self.dimension / 100).max(1);
                (base_size / dimension_factor).max(1_000).min(50_000)
            },
            StorageEngineType::Sst => 1_000, // Smaller for OLTP workloads
        }
    }
    
    /// Get suggested flush threshold in number of vectors
    pub fn flush_threshold(&self) -> usize {
        match self.storage_engine {
            StorageEngineType::Viper => {
                // VIPER benefits from larger batches for columnar compression
                let base_threshold = 50_000;
                let dimension_factor = (self.dimension / 100).max(1);
                (base_threshold / dimension_factor).max(10_000).min(100_000)
            },
            StorageEngineType::Sst => {
                // SST optimized for smaller, frequent flushes
                let base_threshold = 10_000;
                let dimension_factor = (self.dimension / 100).max(1);
                (base_threshold / dimension_factor).max(1_000).min(25_000)
            },
        }
    }
}
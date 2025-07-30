// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Collection Strategy Configuration
//!
//! Stores essential configuration for collection lifecycle operations including
//! inserting, flushing, compacting, and generating indexes. This configuration
//! is persisted with the collection metadata and used throughout its lifetime.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Use proto enums as base types for consistency
use crate::proto::proximadb::{StorageEngine as ProtoStorageEngine, IndexingAlgorithm as ProtoIndexingAlgorithm, DistanceMetric as ProtoDistanceMetric};

/// Collection strategy configuration for persistence and lifecycle operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionStrategyConfig {
    /// Indexing algorithm configuration
    pub indexing_config: IndexingConfig,
    
    /// Storage engine configuration
    pub storage_config: StorageConfig,
    
    /// Search engine configuration
    pub search_config: SearchConfig,
    
    /// Performance tuning parameters
    pub performance_config: PerformanceConfig,
}

/// Indexing algorithm configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingConfig {
    /// Primary algorithm
    pub algorithm: IndexingAlgorithm,
    /// Algorithm-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
}

/// Available indexing algorithms with configuration parameters
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IndexingAlgorithm {
    /// Hierarchical Navigable Small World
    HNSW {
        m: u32,
        ef_construction: u32,
        ef_search: u32,
    },
    /// Inverted File Index
    IVF {
        nlist: u32,
        nprobe: u32,
    },
    /// Product Quantization
    PQ {
        m: u32,
        nbits: u32,
    },
    /// Flat (brute force) search
    Flat,
}

impl IndexingAlgorithm {
    /// Convert to proto enum (loses parameters)
    pub fn to_proto_type(&self) -> ProtoIndexingAlgorithm {
        match self {
            IndexingAlgorithm::HNSW { .. } => ProtoIndexingAlgorithm::Hnsw,
            IndexingAlgorithm::IVF { .. } => ProtoIndexingAlgorithm::Ivf,
            IndexingAlgorithm::PQ { .. } => ProtoIndexingAlgorithm::Pq,
            IndexingAlgorithm::Flat => ProtoIndexingAlgorithm::Flat,
        }
    }
    
    /// Create from proto enum with default parameters
    pub fn from_proto_type(proto: ProtoIndexingAlgorithm) -> Self {
        match proto {
            ProtoIndexingAlgorithm::Hnsw => IndexingAlgorithm::HNSW {
                m: 16,
                ef_construction: 200,
                ef_search: 50,
            },
            ProtoIndexingAlgorithm::Ivf => IndexingAlgorithm::IVF {
                nlist: 100,
                nprobe: 1,
            },
            ProtoIndexingAlgorithm::Pq => IndexingAlgorithm::PQ {
                m: 8,
                nbits: 8,
            },
            ProtoIndexingAlgorithm::Flat => IndexingAlgorithm::Flat,
            _ => IndexingAlgorithm::HNSW {
                m: 16,
                ef_construction: 200,
                ef_search: 50,
            },
        }
    }
}

/// Storage engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Storage engine type
    pub engine_type: StorageEngineType,
    /// Engine-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
}

/// Storage engine types - use proto enum directly for consistency
pub type StorageEngineType = ProtoStorageEngine;

/// Search engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Distance metric for similarity
    pub distance_metric: DistanceMetric,
    /// Search-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Enable search optimizations
    pub enable_optimization: bool,
}

/// Distance metrics - use proto enum directly for consistency
pub type DistanceMetric = ProtoDistanceMetric;

/// Performance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Memory limit in MB
    pub memory_limit_mb: u32,
    /// Enable SIMD optimizations
    pub enable_simd: bool,
    /// Enable GPU acceleration
    pub enable_gpu: bool,
    /// Batch configuration
    pub batch_config: BatchConfig,
}

/// Batch processing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Batch size for operations
    pub batch_size: usize,
    /// Batch timeout in milliseconds
    pub batch_timeout_ms: u64,
}

impl Default for CollectionStrategyConfig {
    fn default() -> Self {
        Self {
            indexing_config: IndexingConfig {
                algorithm: IndexingAlgorithm::HNSW {
                    m: 16,
                    ef_construction: 200,
                    ef_search: 50,
                },
                parameters: HashMap::new(),
            },
            storage_config: StorageConfig {
                engine_type: ProtoStorageEngine::Sst,
                parameters: HashMap::new(),
            },
            search_config: SearchConfig {
                distance_metric: ProtoDistanceMetric::Cosine,
                parameters: HashMap::new(),
                enable_optimization: true,
            },
            performance_config: PerformanceConfig {
                memory_limit_mb: 1024,
                enable_simd: true,
                enable_gpu: false,
                batch_config: BatchConfig {
                    batch_size: 1000,
                    batch_timeout_ms: 100,
                },
            },
        }
    }
}
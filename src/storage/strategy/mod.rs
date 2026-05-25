// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Collection Strategy Configuration
//!
//! Stores essential configuration for collection lifecycle operations including
//! inserting, flushing, compacting, and generating indexes. This configuration
//! is persisted with the collection metadata and used throughout its lifetime.

use std::collections::HashMap;

// Use proto enums as base types for consistency
use crate::proto::proximadb_v1::{
    DistanceMetric as ProtoDistanceMetric, IndexingAlgorithm as ProtoIndexingAlgorithm,
    StorageEngine as ProtoStorageEngine,
};

/// Collection strategy configuration for persistence and lifecycle operations
#[derive(Debug, Clone)]
pub struct CollectionStrategyConfig {
    /// Indexing algorithm configuration
    pub indexing_config: IndexingConfig,

    /// Storage engine configuration
    pub storage_config: StrategyStorageConfig,

    /// Search engine configuration
    pub search_config: StrategySearchConfig,

    /// Performance tuning parameters
    pub performance_config: StrategyPerformanceConfig,
}

/// Indexing algorithm configuration
#[derive(Debug, Clone)]
pub struct IndexingConfig {
    /// Primary algorithm
    pub algorithm: IndexingAlgorithm,
    /// Algorithm-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
}

/// Available indexing algorithms with configuration parameters
#[derive(Debug, Clone)]
pub enum IndexingAlgorithm {
    /// Hierarchical Navigable Small World
    ///
    /// A graph-based approximate nearest neighbor algorithm that provides
    /// excellent recall with fast query times.
    HNSW {
        /// Number of bidirectional links for each node (higher = better recall, slower indexing)
        m: u32,
        /// Size of dynamic candidate list for construction (higher = better recall, slower indexing)
        ef_construction: u32,
        /// Size of dynamic candidate list for search (higher = better recall, slower search)
        ef_search: u32,
    },
    /// Inverted File Index
    ///
    /// Partitions the vector space into Voronoi cells and searches only
    /// the nearest cells for approximate search.
    IVF {
        /// Number of centroid clusters (higher = better precision, more memory)
        nlist: u32,
        /// Number of clusters to probe during search (higher = better recall, slower search)
        nprobe: u32,
    },
    /// Product Quantization
    ///
    /// Compresses vectors into compact codes for memory-efficient storage
    /// and fast distance computation.
    PQ {
        /// Number of sub-quantizers (must divide dimensionality)
        m: u32,
        /// Number of bits per sub-quantizer (typically 8)
        nbits: u32,
    },
    /// Flat (brute force) search
    ///
    /// Exact nearest neighbor search with no approximation.
    /// Provides perfect recall but slower query performance.
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
            ProtoIndexingAlgorithm::Pq => IndexingAlgorithm::PQ { m: 8, nbits: 8 },
            ProtoIndexingAlgorithm::Flat => IndexingAlgorithm::Flat,
            _ => IndexingAlgorithm::HNSW {
                m: 16,
                ef_construction: 200,
                ef_search: 50,
            },
        }
    }
}

/// Backwards-compat alias for [`StrategyStorageConfig`].
pub type StorageConfig = StrategyStorageConfig;

/// Storage engine configuration
#[derive(Debug, Clone)]
pub struct StrategyStorageConfig {
    /// Storage engine type
    pub engine_type: StorageEngineType,
    /// Engine-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
}

/// Storage engine types - use proto enum directly for consistency
pub type StorageEngineType = ProtoStorageEngine;

/// Backwards-compat alias for [`StrategySearchConfig`].
pub type SearchConfig = StrategySearchConfig;

/// Search engine configuration
#[derive(Debug, Clone)]
pub struct StrategySearchConfig {
    /// Distance metric for similarity
    pub distance_metric: DistanceMetric,
    /// Search-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Enable search optimizations
    pub enable_optimization: bool,
}

/// Distance metrics - use proto enum directly for consistency
pub type DistanceMetric = ProtoDistanceMetric;

/// Backwards-compat alias for [`StrategyPerformanceConfig`].
pub type PerformanceConfig = StrategyPerformanceConfig;

/// Performance configuration
#[derive(Debug, Clone)]
pub struct StrategyPerformanceConfig {
    /// Memory limit in MB
    pub memory_limit_mb: u32,
    /// Enable SIMD optimizations
    pub enable_simd: bool,
    /// Enable GPU acceleration
    pub enable_gpu: bool,
    /// Batch configuration
    pub batch_config: StrategyBatchConfig,
}

/// Backwards-compat alias for [`StrategyBatchConfig`].
pub type BatchConfig = StrategyBatchConfig;

/// Batch processing configuration
#[derive(Debug, Clone)]
pub struct StrategyBatchConfig {
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
            storage_config: StrategyStorageConfig {
                engine_type: ProtoStorageEngine::Sst,
                parameters: HashMap::new(),
            },
            search_config: StrategySearchConfig {
                distance_metric: ProtoDistanceMetric::Cosine,
                parameters: HashMap::new(),
                enable_optimization: true,
            },
            performance_config: StrategyPerformanceConfig {
                memory_limit_mb: 1024,
                enable_simd: true,
                enable_gpu: false,
                batch_config: StrategyBatchConfig {
                    batch_size: 1000,
                    batch_timeout_ms: 100,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexing_algorithm_round_trips_to_proto_family() {
        assert_eq!(
            IndexingAlgorithm::HNSW {
                m: 32,
                ef_construction: 400,
                ef_search: 100,
            }
            .to_proto_type(),
            ProtoIndexingAlgorithm::Hnsw
        );
        assert_eq!(
            IndexingAlgorithm::IVF {
                nlist: 256,
                nprobe: 8,
            }
            .to_proto_type(),
            ProtoIndexingAlgorithm::Ivf
        );
        assert_eq!(
            IndexingAlgorithm::PQ { m: 16, nbits: 6 }.to_proto_type(),
            ProtoIndexingAlgorithm::Pq
        );
        assert_eq!(
            IndexingAlgorithm::Flat.to_proto_type(),
            ProtoIndexingAlgorithm::Flat
        );
    }

    #[test]
    fn indexing_algorithm_from_proto_uses_stable_defaults() {
        match IndexingAlgorithm::from_proto_type(ProtoIndexingAlgorithm::Hnsw) {
            IndexingAlgorithm::HNSW {
                m,
                ef_construction,
                ef_search,
            } => {
                assert_eq!(m, 16);
                assert_eq!(ef_construction, 200);
                assert_eq!(ef_search, 50);
            }
            other => panic!("expected HNSW defaults, got {other:?}"),
        }

        match IndexingAlgorithm::from_proto_type(ProtoIndexingAlgorithm::Ivf) {
            IndexingAlgorithm::IVF { nlist, nprobe } => {
                assert_eq!(nlist, 100);
                assert_eq!(nprobe, 1);
            }
            other => panic!("expected IVF defaults, got {other:?}"),
        }

        match IndexingAlgorithm::from_proto_type(ProtoIndexingAlgorithm::Pq) {
            IndexingAlgorithm::PQ { m, nbits } => {
                assert_eq!(m, 8);
                assert_eq!(nbits, 8);
            }
            other => panic!("expected PQ defaults, got {other:?}"),
        }

        assert!(matches!(
            IndexingAlgorithm::from_proto_type(ProtoIndexingAlgorithm::Flat),
            IndexingAlgorithm::Flat
        ));
    }

    #[test]
    fn collection_strategy_default_matches_realtime_sst_profile() {
        let config = CollectionStrategyConfig::default();

        assert!(matches!(
            config.indexing_config.algorithm,
            IndexingAlgorithm::HNSW {
                m: 16,
                ef_construction: 200,
                ef_search: 50,
            }
        ));
        assert!(config.indexing_config.parameters.is_empty());

        assert_eq!(config.storage_config.engine_type, ProtoStorageEngine::Sst);
        assert!(config.storage_config.parameters.is_empty());

        assert_eq!(
            config.search_config.distance_metric,
            ProtoDistanceMetric::Cosine
        );
        assert!(config.search_config.enable_optimization);
        assert!(config.search_config.parameters.is_empty());

        assert_eq!(config.performance_config.memory_limit_mb, 1024);
        assert!(config.performance_config.enable_simd);
        assert!(!config.performance_config.enable_gpu);
        assert_eq!(config.performance_config.batch_config.batch_size, 1000);
        assert_eq!(config.performance_config.batch_config.batch_timeout_ms, 100);
    }
}

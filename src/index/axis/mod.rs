// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! AXIS - Adaptive eXtensible Indexing System
//!
//! A sophisticated indexing system that automatically adapts to collection
//! characteristics and query patterns, providing zero-downtime migration
//! between indexing strategies as data evolves.

pub mod adaptive_engine;
pub mod analyzer;
pub mod manager;
pub mod migration_engine;
pub mod monitor;
pub mod strategy;

// Unit tests
#[cfg(test)]
pub mod manager_tests;
#[cfg(test)]
pub mod adaptive_engine_tests;
#[cfg(test)]
pub mod analyzer_tests;

pub use adaptive_engine::{
    AccessFrequencyMetrics, AdaptiveIndexEngine, CollectionCharacteristics, MetadataComplexity,
    PerformanceMetrics, QueryDistribution, QueryPatternAnalysis, QueryPatternType, TemporalPattern,
};
pub use analyzer::CollectionAnalyzer;
pub use manager::{
    AxisIndexManager, FilterOperator, HybridQuery, MetadataFilter, MigrationStatus, QueryResult,
    ScoredResult, VectorQuery,
};
pub use migration_engine::{IndexMigrationEngine, MigrationPlan, MigrationResult};
pub use monitor::PerformanceMonitor;
pub use strategy::{IndexStrategy, IndexType, OptimizationConfig};

use serde::{Deserialize, Serialize};

/// AXIS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisConfig {
    /// Migration settings
    pub migration_config: MigrationConfig,

    /// Performance monitoring settings
    pub monitoring_config: MonitoringConfig,

    /// Strategy selection parameters
    pub strategy_config: StrategyConfig,

    /// Resource limits
    pub resource_limits: ResourceLimits,
}

impl Default for AxisConfig {
    fn default() -> Self {
        Self {
            migration_config: MigrationConfig::default(),
            monitoring_config: MonitoringConfig::default(),
            strategy_config: StrategyConfig::default(),
            resource_limits: ResourceLimits::default(),
        }
    }
}

/// Migration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConfig {
    /// Minimum improvement threshold to trigger migration (0.0-1.0)
    pub improvement_threshold: f64,

    /// Maximum concurrent migrations
    pub max_concurrent_migrations: usize,

    /// Migration batch size for incremental migration
    pub migration_batch_size: usize,

    /// Rollback timeout in seconds
    pub rollback_timeout_seconds: u64,

    /// Enable/disable automatic migrations
    pub auto_migration_enabled: bool,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            improvement_threshold: 0.2, // 20% improvement required
            max_concurrent_migrations: 2,
            migration_batch_size: 10000,
            rollback_timeout_seconds: 300, // 5 minutes
            auto_migration_enabled: true,
        }
    }
}

/// Monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    /// Metrics collection interval in seconds
    pub metrics_interval_seconds: u64,

    /// Performance alert thresholds
    pub alert_thresholds: AlertThresholds,

    /// Enable detailed performance logging
    pub detailed_logging: bool,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            metrics_interval_seconds: 60, // 1 minute
            alert_thresholds: AlertThresholds::default(),
            detailed_logging: false,
        }
    }
}

/// Alert thresholds for performance monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertThresholds {
    /// Query latency threshold in milliseconds
    pub max_query_latency_ms: u64,

    /// Minimum query throughput (QPS)
    pub min_query_throughput: f64,

    /// Maximum error rate (0.0-1.0)
    pub max_error_rate: f64,
}

impl Default for AlertThresholds {
    fn default() -> Self {
        Self {
            max_query_latency_ms: 100,
            min_query_throughput: 100.0,
            max_error_rate: 0.01, // 1%
        }
    }
}

/// Strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    /// Enable ML-based strategy selection
    pub use_ml_models: bool,

    /// Strategy evaluation interval in seconds
    pub evaluation_interval_seconds: u64,

    /// Minimum data size for ML model training
    pub min_training_size: usize,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            use_ml_models: true,
            evaluation_interval_seconds: 3600, // 1 hour
            min_training_size: 10000,
        }
    }
}

/// Resource limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum memory for indexing operations
    pub max_index_memory_gb: f64,

    /// Maximum concurrent index operations
    pub max_concurrent_operations: usize,

    /// Maximum index rebuild time in seconds
    pub max_rebuild_time_seconds: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_index_memory_gb: 16.0,
            max_concurrent_operations: 4,
            max_rebuild_time_seconds: 3600, // 1 hour
        }
    }
}

/// Migration decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationDecision {
    /// Migrate to new strategy
    Migrate {
        from: IndexStrategy,
        to: IndexStrategy,
        estimated_improvement: f64,
        migration_complexity: f64,
        estimated_duration: std::time::Duration,
    },
    /// Stay with current strategy
    Stay { reason: String },
}

/// Migration priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MigrationPriority {
    Low,
    Medium,
    High,
    Critical,
}

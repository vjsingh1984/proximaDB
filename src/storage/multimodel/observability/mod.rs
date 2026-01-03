//! # Observability Optimization Module
//!
//! Provides optimization strategies for logs, metrics, and traces:
//! - Time-based partitioning for efficient range queries
//! - Rollup materialized views for fast aggregations
//! - High-cardinality label optimization
//!
//! ## Architecture
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────┐
//! │                Observability Optimization                     │
//! │  ┌─────────────────────────────────────────────────────────┐ │
//! │  │              Time Partitioner                            │ │
//! │  │  - Hourly/daily partitions for logs                     │ │
//! │  │  - Automatic partition creation and retention           │ │
//! │  │  - Partition pruning for range queries                  │ │
//! │  └─────────────────────────────────────────────────────────┘ │
//! │                                                               │
//! │  ┌─────────────────────────────────────────────────────────┐ │
//! │  │              Rollup Manager                              │ │
//! │  │  - Pre-computed aggregations (1min, 5min, 1hr, 1day)   │ │
//! │  │  - Automatic rollup computation                         │ │
//! │  │  - Query-time rollup selection                          │ │
//! │  └─────────────────────────────────────────────────────────┘ │
//! │                                                               │
//! │  ┌─────────────────────────────────────────────────────────┐ │
//! │  │              Cardinality Limiter                         │ │
//! │  │  - High-cardinality label detection                     │ │
//! │  │  - Label value limiting with bloom filters              │ │
//! │  │  - Dynamic cardinality thresholds                       │ │
//! │  └─────────────────────────────────────────────────────────┘ │
//! └───────────────────────────────────────────────────────────────┘
//! ```

pub mod cardinality;
pub mod partitioning;
pub mod rollups;

// Re-exports
pub use cardinality::{
    CardinalityConfig, CardinalityLimiter, CheckResult, LabelStats, LimitAction,
};
pub use partitioning::{
    Partition, PartitionConfig, PartitionGranularity, PartitionRange, TimePartitioner,
};
pub use rollups::{AggregationFunction, RollupConfig, RollupInterval, RollupManager, RollupView};

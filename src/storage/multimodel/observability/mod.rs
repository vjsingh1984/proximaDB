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

pub mod partitioning;
pub mod rollups;
pub mod cardinality;

// Re-exports
pub use partitioning::{TimePartitioner, PartitionConfig, Partition, PartitionRange, PartitionGranularity};
pub use rollups::{RollupManager, RollupConfig, RollupInterval, RollupView, AggregationFunction};
pub use cardinality::{CardinalityLimiter, CardinalityConfig, LabelStats, CheckResult, LimitAction};

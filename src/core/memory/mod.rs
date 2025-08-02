//! Memory management utilities for ProximaDB
//! 
//! This module provides efficient memory management tools including:
//! - Buffer pooling for reduced allocation overhead
//! - Workload-aware memory sizing
//! - Statistics and monitoring for memory usage optimization

pub mod pool;

pub use pool::{
    Pool, PoolConfig, PoolStats, PooledItem,
    VectorMemoryPool, VectorPoolStats,
};
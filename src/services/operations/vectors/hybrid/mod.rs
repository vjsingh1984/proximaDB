//! Hybrid query construction for vector search.
//!
//! This module provides utilities for building hybrid queries that combine
//! vector similarity with metadata filtering, particularly for the AXIS index.

#[cfg(feature = "axis")]
pub mod axis_builder;

#[cfg(feature = "axis")]
pub use axis_builder::{
    build_axis_hybrid_query, build_axis_hybrid_query_with_mode, build_axis_hybrid_query_with_policy,
};

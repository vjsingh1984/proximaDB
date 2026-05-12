//! Hybrid query construction for vector search.
//!
//! This module provides utilities for building hybrid queries that combine
//! vector similarity with metadata filtering, particularly for the AXIS index.

pub mod axis_builder;

pub use axis_builder::build_axis_hybrid_query;

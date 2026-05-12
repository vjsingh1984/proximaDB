//! Search operations for vector similarity and hybrid queries.
//!
//! This module provides utilities for executing vector searches across
//! different storage engines with support for progressive quantization
//! and metadata filtering.

pub mod executor;
pub mod pipeline;

pub use executor::SearchResult;
pub use pipeline::{ProgressiveSearchPipeline, StageResult, default_progressive_stages};

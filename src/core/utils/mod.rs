//! # Core Utilities Module
//!
//! This module contains common utility functions and helpers that are used across
//! multiple components of ProximaDB. It consolidates previously duplicated code
//! into reusable, well-documented, and tested utilities.
//!
//! ## Submodules
//!
//! - **metadata_conversions**: Conversions between different metadata representations
//! - **vector_ops**: Common vector operations and transformations
//! - **validation**: Input validation and sanitization utilities
//! - **config_utils**: Configuration parsing and validation helpers
//!
//! ## Design Principles
//!
//! 1. **Zero-cost abstractions**: Utilities should have minimal runtime overhead
//! 2. **Type safety**: Strong typing to catch errors at compile time
//! 3. **Documentation**: Every public function must be thoroughly documented
//! 4. **Testing**: Comprehensive unit tests for all utility functions
//! 5. **Performance**: SIMD and other optimizations where applicable
//!
//! ## Migration Guide
//!
//! When refactoring existing code to use these utilities:
//! 
//! 1. Search for duplicate implementations of the same logic
//! 2. Replace with calls to the appropriate utility function
//! 3. Update imports to use `crate::core::utils::{module}::{function}`
//! 4. Run tests to ensure functionality is preserved
//! 5. Remove the old duplicate implementation

pub mod metadata_conversions;
pub mod vector_ops;
pub mod validation;

// Re-export commonly used functions for convenience
pub use metadata_conversions::{
    proto_metadata_to_json,
    json_to_proto_metadata,
    json_to_metadata_item,
    merge_metadata,
    filter_metadata,
};

pub use vector_ops::{
    normalize_l2,
    validate_vector,
    resize_vector,
    dot_product,
    cosine_similarity,
    mean,
    standard_deviation,
};

pub use validation::{
    validate_vector_id,
    validate_collection_name,
    validate_field_name,
    validate_dimension,
    validate_batch_size,
    validate_top_k,
    validate_distance_metric,
    validate_storage_engine,
};
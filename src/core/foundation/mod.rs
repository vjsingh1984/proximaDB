//! Foundation Types for ProximaDB Unified Schema
//!
//! This module provides the base traits and generic implementations that serve as the
//! foundation for all other schema modules. It has no dependencies on other modules.

pub mod base_traits;
pub mod conversion;
pub mod generic_types;

// Re-export all foundation types
pub use base_traits::*;
pub use conversion::*;
pub use generic_types::*;

#[cfg(test)]
include!("../foundation_tests.rs");

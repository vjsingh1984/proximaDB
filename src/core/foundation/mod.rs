//! Foundation Types for ProximaDB Unified Schema
//!
//! This module provides the base traits and generic implementations that serve as the
//! foundation for all other schema modules. It has no dependencies on other modules.

pub mod base_traits;
pub mod generic_types;
pub mod conversion;

// Re-export all foundation types
pub use base_traits::*;
pub use generic_types::*;
pub use conversion::*;
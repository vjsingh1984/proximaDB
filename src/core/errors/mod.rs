//! Error Types for ProximaDB Unified Schema
//!
//! This module provides all error types used throughout the ProximaDB system.
//! It builds on the foundation module for common functionality.

pub mod config_error;
pub mod metadata_error;
pub mod service_error;
pub mod core_error;

// Re-export all error types
pub use config_error::*;
pub use metadata_error::*;
pub use service_error::*;
pub use core_error::*;
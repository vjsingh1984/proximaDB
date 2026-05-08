//! Compatibility shim for the extracted foundation contracts.
//!
//! The shared generic foundation traits and helper types now live in the
//! `proximadb-kernel` workspace crate. This module preserves existing imports
//! like `crate::core::foundation::BaseConfig` during the workspace migration.

pub use proximadb_kernel::foundation::base_traits;
pub use proximadb_kernel::foundation::conversion;
pub use proximadb_kernel::foundation::generic_types;
pub use proximadb_kernel::foundation::*;

#[cfg(test)]
include!("../foundation_tests.rs");

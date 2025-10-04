//! Storage Engine Tests - Consolidated Test Suite
//!
//! This module contains all consolidated tests for ProximaDB storage engines.
//!
//! ## Migration Status
//!
//! ### SST Engine
//! -  Helpers: 35 functions (22 duplicates eliminated)
//! -  Flush Tests: 11 tests migrated
//! -  Search Tests: 69 tests migrated
//! - � Core Tests: 228 tests identified
//!
//! ### Other Engines (Pending)
//! - VIPER: 146 tests identified
//! - NOVA: 65 tests identified
//! - HELIX: 72 tests identified
//! - RAPTOR: 51 tests identified
//! - SWIFT: 21 tests identified
//!
//! ## Module Structure
//!
//! - `sst` - SST engine tests (80 tests consolidated + 228 identified)
//! - `viper_tests` - VIPER engine tests (placeholder)
//! - `nova_tests` - NOVA engine tests (placeholder)
//! - `helix_tests` - HELIX engine tests (placeholder)
//! - `raptor_tests` - RAPTOR engine tests (placeholder)
//! - `swift_tests` - SWIFT engine tests (placeholder)
//! - `common_tests` - Cross-engine tests (placeholder)

#[cfg(test)]
pub mod sst;

#[cfg(test)]
pub mod viper;

#[cfg(test)]
pub mod nova;

#[cfg(test)]
pub mod helix;

#[cfg(test)]
mod raptor_tests;

#[cfg(test)]
mod swift_tests;

#[cfg(test)]
mod common_tests;

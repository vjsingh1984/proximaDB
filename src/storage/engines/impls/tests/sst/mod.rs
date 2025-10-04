//! SST Engine Test Module - Consolidated Test Suite
//!
//! This module contains all consolidated tests for the SST (Sorted String Table) engine.
//! Tests have been migrated from 41 original source files into organized sub-modules.
//!
//! ## Migration Status
//!
//! - ✅ **Helpers**: 35 functions consolidated (22 duplicates eliminated)
//! - ✅ **Flush Tests**: 11 tests migrated from 9 source files
//! - ✅ **Search Tests**: 69 tests migrated from 7+ source files (9 duplicates removed)
//! - ⏸️ **Core Tests**: 228 tests identified, phased migration recommended
//!
//! ## Module Structure
//!
//! - `helpers` - Shared test utilities (35 helper functions)
//! - `flush_tests` - Flush operation tests (11 tests)
//! - `search_tests` - Search and reader tests (69 tests)
//! - `core_tests` - Core engine tests (228 tests, skeleton created)
//!
//! ## Original Sources
//!
//! Tests consolidated from:
//! - 21 inline `#[cfg(test)]` modules
//! - 13 dedicated test files
//! - 7 reader test files
//!
//! Total reduction: 41 files → 4 organized modules (90% reduction)

pub mod helpers;

#[cfg(test)]
mod flush_tests;

#[cfg(test)]
mod search_tests;

#[cfg(test)]
mod core_tests;

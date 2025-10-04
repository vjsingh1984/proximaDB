//! VIPER Engine Test Module - Consolidated Test Suite
//!
//! This module contains all consolidated tests for the VIPER (Columnar Parquet) engine.
//! Tests have been migrated from 20 original source files into organized sub-modules.
//!
//! ## Migration Status
//!
//! - ✅ **Helpers**: 32 functions consolidated (~13 duplicates eliminated)
//! - ✅ **Reader Tests**: 50 tests migrated (29 active, 21 commented - missing types)
//! - ✅ **Flush Tests**: 4 tests migrated
//! - ✅ **Metadata Tests**: 5 tests migrated
//! - ✅ **Core Tests**: 5 tests migrated
//! - ⏸️ **Engine Tests**: 26 tests identified (partial migration)
//! - ⏸️ **Compaction Tests**: 10 tests identified (pending)
//! - ⏸️ **Test Data Generator**: 14 tests identified (pending)
//!
//! ## Module Structure
//!
//! - `helpers` - Shared test utilities (32 helper functions)
//! - `reader_tests` - Parquet reader tests (50 tests)
//! - `flush_tests` - Atomic flush tests (4 tests)
//! - `metadata_tests` - Metadata serialization tests (5 tests)
//! - `core_tests` - Core functionality tests (5 tests)
//! - `engine_tests` - Engine integration tests (26 tests, in progress)
//! - `compaction_tests` - Compaction tests (10 tests, pending)
//!
//! ## Original Sources
//!
//! Tests consolidated from:
//! - 8 dedicated test files in tests/
//! - 4 reader test files
//! - 4 inline test modules
//! - 4 other source files
//!
//! Total reduction: 20 files → 7 organized modules (65% reduction)

pub mod helpers;

#[cfg(test)]
mod reader_tests;

#[cfg(test)]
mod flush_tests;

#[cfg(test)]
mod metadata_tests;

#[cfg(test)]
mod core_tests;

#[cfg(test)]
mod engine_tests;

#[cfg(test)]
mod compaction_tests;

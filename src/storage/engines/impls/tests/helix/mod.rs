//! HELIX Engine Test Module - Consolidated Test Suite
//!
//! This module contains all consolidated tests for the HELIX (Locality-Optimized) engine.
//! Tests have been migrated from 15 original source files into organized sub-modules.
//!
//! ## Migration Status
//!
//! - ✅ **Helpers**: 28 functions consolidated
//! - ✅ **Integration Tests**: 24 tests migrated (from tests/integration_tests.rs + tests.rs)
//! - ✅ **Hilbert Tests**: 6 tests migrated (Hilbert curve encoding)
//! - ✅ **Clustering Tests**: 5 tests migrated (PCA, liquid clustering)
//! - ✅ **Zone Map Tests**: 3 tests migrated (dimension-level pruning)
//! - ✅ **Core Tests**: 22 tests migrated (benchmarks, readers, metadata, optimization, etc.)
//! - **Total**: 60 tests consolidated
//!
//! ## Module Structure
//!
//! - `helpers` - Shared test utilities (28 helper functions)
//! - `integration_tests` - Integration tests (24 tests)
//! - `hilbert_tests` - Hilbert curve tests (6 tests)
//! - `clustering_tests` - PCA and clustering tests (5 tests)
//! - `zone_map_tests` - Zone map pruning tests (3 tests)
//! - `core_tests` - Core functionality tests (22 tests)
//!
//! ## Original Sources
//!
//! Tests consolidated from:
//! - 2 dedicated test files (tests/integration_tests.rs, tests.rs)
//! - 13 inline test modules
//!
//! Total reduction: 15 files → 6 organized modules (60% reduction)

pub mod helpers;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod hilbert_tests;

#[cfg(test)]
mod clustering_tests;

#[cfg(test)]
mod zone_map_tests;

#[cfg(test)]
mod core_tests;

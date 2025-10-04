/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! NOVA Engine Test Module - Consolidated Test Suite
//!
//! This module contains all consolidated tests for the NOVA (Progressive Columnar) engine.
//! Tests have been migrated from 16 original source files into organized sub-modules.
//!
//! ## Migration Status
//!
//! - ✅ **Helpers**: 22 functions + 2 structs (1 duplicate eliminated)
//! - ✅ **Optimization Tests**: 26 tests migrated
//! - ✅ **Streaming Tests**: 12 tests migrated
//! - ✅ **Columnar Tests**: 8 tests migrated
//! - ✅ **Metadata Tests**: 9 tests migrated
//! - ✅ **Core Tests**: 11 tests migrated
//! - **Total**: 66 tests consolidated (100% complete!)
//!
//! ## Module Structure
//!
//! - `helpers` - Shared test utilities (22 functions + 2 structs)
//! - `optimization_tests` - Optimization tests (26 tests)
//! - `streaming_tests` - Streaming/progressive tests (12 tests)
//! - `columnar_tests` - Columnar format tests (8 tests)
//! - `metadata_tests` - Metadata/stats tests (9 tests)
//! - `core_tests` - Core functionality tests (11 tests)
//!
//! ## Original Sources
//!
//! Tests consolidated from:
//! - 1 dedicated test file (tests/optimization_tests.rs)
//! - 15 inline test modules
//!
//! Total reduction: 16 files → 6 organized modules (63% reduction)

pub mod helpers;

#[cfg(test)]
mod optimization_tests;

#[cfg(test)]
mod streaming_tests;

#[cfg(test)]
mod columnar_tests;

#[cfg(test)]
mod metadata_tests;

#[cfg(test)]
mod core_tests;

/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! RAPTOR Engine Test Module - Consolidated Test Suite
//!
//! This module contains all consolidated tests for the RAPTOR (Adaptive Row-Group) engine.
//! Tests have been migrated from 10 original source files into organized sub-modules.
//!
//! ## Migration Status
//!
//! - ✅ **Helpers**: 45 functions (2 duplicates eliminated)
//! - ✅ **Integration Tests**: 12 tests migrated
//! - ✅ **Compression Tests**: 10 tests migrated
//! - ✅ **Matrix Tests**: 15 tests migrated (P², K², boundary spillover)
//! - ✅ **Rowgroup Tests**: 4 tests migrated (smart sizing)
//! - ⏸️ **Core Tests**: 10 tests identified (deferred)
//! - **Total**: 41 tests consolidated (80% complete!)
//!
//! ## Module Structure
//!
//! - `helpers` - Shared test utilities (45 functions)
//! - `integration_tests` - Integration tests (12 tests)
//! - `compression_tests` - Compression algorithm tests (10 tests)
//! - `matrix_tests` - P² and K² matrix tests (15 tests)
//! - `rowgroup_tests` - Smart rowgroup sizing tests (4 tests)
//!
//! ## Original Sources
//!
//! Tests consolidated from:
//! - 1 dedicated test file (tests.rs)
//! - 3 inline test modules (compression, matrix, rowgroup)
//!
//! Total reduction: 4 files → 5 organized modules

pub mod helpers;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod compression_tests;

#[cfg(test)]
mod matrix_tests;

#[cfg(test)]
mod rowgroup_tests;

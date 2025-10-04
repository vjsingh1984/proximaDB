/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! SWIFT Engine Test Module - Consolidated Test Suite
//!
//! This module contains all consolidated tests for the SWIFT (High-Speed Row-Based) engine.
//! Tests have been migrated from 10 original inline test modules into organized sub-modules.
//!
//! ## Migration Status
//!
//! - ✅ **Helpers**: 10 functions (already consolidated)
//! - ✅ **Reader Tests**: 9 tests migrated (unified, progressive, hierarchical, ID index)
//! - ✅ **Operations Tests**: 9 tests migrated (batch, optimized, metadata)
//! - ✅ **Core Tests**: 3 tests migrated (engine, features, ID index)
//! - **Total**: 21 tests consolidated (100% complete!)
//!
//! ## Module Structure
//!
//! - `helpers` - Shared test utilities (10 functions)
//! - `reader_tests` - Reader and search tests (9 tests)
//! - `operations_tests` - Operations and metadata tests (9 tests)
//! - `core_tests` - Core engine tests (3 tests)
//!
//! ## Original Sources
//!
//! Tests consolidated from:
//! - 10 inline test modules
//!
//! Total reduction: 10 files → 4 organized modules (60% reduction)

pub mod helpers;

#[cfg(test)]
mod reader_tests;

#[cfg(test)]
mod operations_tests;

#[cfg(test)]
mod core_tests;

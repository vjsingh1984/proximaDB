/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! ProximaDB Unit Tests
//!
//! **IMPORTANT:** This directory structure has been DEPRECATED.
//!
//! ## Migration Complete (2026-04-07):
//! All unit tests have been successfully inlined into their source modules following Rust best practices.
//! Unit tests are now located as `#[cfg(test)] mod tests` blocks within their respective source files.
//!
//! ## Test Organization (Current):
//! - **Unit Tests:** Inline in source files → `cargo test --lib`
//! - **Integration Tests:** Standalone files → `cargo test --test integration`
//!
//! ## Historical Migration:
//! Tests that were previously here have been moved:
//! - mvcc_logic_tests → tests/integration/mvcc_logic_test
//! - sst_optimization_tests → tests/integration/sst_optimization_test
//! - write_buffer_recovery_stress_tests → tests/integration/write_buffer_recovery_stress_test
//! - services tests → tests/integration/services_*
//! - storage tests → tests/integration/storage/*
//! - All other unit tests → Inlined into respective src/**/*.rs files
//!
//! ## Directory Status:
//! This directory and its subdirectories are now deprecated and will be removed in future cleanup.
//! Please use `cargo test --lib` for unit tests and `cargo test --test integration` for integration tests.
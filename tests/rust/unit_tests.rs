//! Unit tests module inclusion
//!
//! **DEPRECATED**: This module structure is no longer used.
//!
//! ## Migration Complete (2026-04-07):
//! All unit tests have been successfully inlined into their source modules.
//!
//! ## Current Test Organization:
//! - **Unit Tests:** Now located as `#[cfg(test)]` modules in source files
//!   - Use `cargo test --lib` to run unit tests
//! - **Integration Tests:** Located in `tests/integration/` directory
//!   - Use `cargo test --test integration` to run integration tests
//!
//! ## Historical Context:
//! This file previously included unit tests from `tests/unit/storage/mod.rs`,
//! but those tests have been migrated to inline test modules following Rust best practices.
//!
//! ## See Also:
//! - `src/storage/**/*.rs` - Contains inline unit tests for storage components
//! - `tests/integration/` - Contains integration tests
//! - `tests/unit/mod.rs` - Contains migration documentation

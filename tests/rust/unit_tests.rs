//! Unit tests module inclusion

// Include all unit tests
// Note: Most unit tests are now inline in source files (src/**/*.rs)
// This file mainly includes legacy unit test structure

#[path = "../unit/storage/mod.rs"]
pub mod storage;

// Write buffer recovery stress tests moved to integration tests
// See: tests/integration/write_buffer_recovery_stress_test.rs

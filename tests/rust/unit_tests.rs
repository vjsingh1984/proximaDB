//! Unit tests module inclusion

// Include all unit tests
#[path = "../unit/storage/mod.rs"]
pub mod storage;

// Include write buffer recovery stress tests
#[path = "../unit/write_ahead_log_recovery_stress_tests.rs"]
pub mod write_ahead_log_recovery_stress_tests;
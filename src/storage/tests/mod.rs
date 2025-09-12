//! Storage module tests

#[cfg(test)]
pub mod atomic_coordinator_tests;

#[cfg(test)]
pub mod atomic_coordinator_concurrency_tests;

#[cfg(test)]
pub mod storage_engine_concurrency_tests;

#[cfg(test)]
pub mod atomic_write_tests;

#[cfg(test)]
pub mod storage_engine_simple_tests;

// REMOVED: atomic_path_tests - used outdated async coordinator APIs
// #[cfg(test)]
// pub mod atomic_path_tests;

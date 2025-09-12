//! Performance benchmarks for cache subsystem

#[cfg(all(test, not(target_env = "msvc")))]
pub mod migration_benchmarks;
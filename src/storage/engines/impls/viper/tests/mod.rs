//! VIPER Engine Test Module

pub mod atomic_flush_tests;
pub mod test_data_generator;
pub mod unified_storage_tests;

#[cfg(test)]
pub mod engine_tests;

#[cfg(test)]
pub mod compaction_tests;

#[cfg(test)]
pub mod debug_compaction_test;

// Include pipeline tests in the module structure
// Note: pipeline_tests.rs doesn't exist - commented out
// #[cfg(test)]
// pub mod pipeline_tests;


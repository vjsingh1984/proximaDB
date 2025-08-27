//! VIPER Engine Test Module

pub mod atomic_flush_tests;
pub mod unified_storage_tests;
pub mod test_data_generator;

#[cfg(test)]
pub mod engine_tests;

#[cfg(test)]
pub mod compaction_tests;

#[cfg(test)]
pub mod debug_compaction_test;

// Include pipeline tests in the module structure
#[cfg(test)]
pub mod pipeline_tests {
    pub use crate::storage::engines::impls::viper::pipeline_tests::viper_pipeline_tests::*;
}

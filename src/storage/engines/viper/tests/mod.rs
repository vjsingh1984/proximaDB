//! VIPER Engine Test Module

// Core test utilities
pub mod test_data_generator;

// Main engine tests
#[cfg(test)]
pub mod engine_tests;

// Compaction tests
#[cfg(test)]
pub mod compaction_tests;

#[cfg(test)]
pub mod debug_compaction_test;

// Storage tests
pub mod unified_storage_tests;

// Flattened from readers/tests/ in Phase 2 - Reader-specific tests
pub mod coverage_tests;
pub mod strategy_tests;
pub mod unified_parquet_reader_edge_tests;
pub mod unified_parquet_reader_tests;

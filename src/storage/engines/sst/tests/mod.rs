//! SST Engine Test Module

// Main engine tests
pub mod arrow_block_end_to_end_test;
pub mod arrow_vs_proximablocks_benchmark;
pub mod arrowblock_compaction_test;
pub mod arrowblock_full_lifecycle_test;
pub mod compaction_coverage_tests;
pub mod compaction_vector_tracking_tests;
pub mod cross_format_interop_benchmark;
pub mod end_to_end_test;
pub mod flush_recovery_tdd_test;
pub mod hierarchical_tests;
pub mod modular_integration_test;
pub mod sst_compactor_tests;
pub mod sst1_format_tests;
pub mod strategy_tests;

// Reader-specific tests - flattened from readers/tests/ in Phase 2
pub mod test_metadata_filtering;
pub mod test_metadata_filtering_fixed;
pub mod test_sst1_validation;
pub mod unified_sstable_reader_edge_tests;
pub mod unified_sstable_reader_tests;

//! Quantization Tests Module
//!
//! Comprehensive test suite for ProximaDB's quantization system covering:
//! - Basic quantization functionality
//! - PQ codebook training across all engines
//! - Storage engine integration
//! - Performance and compatibility testing

pub mod quantization_basic_tests;
pub mod quantization_integration_tests;
pub mod pq_codebook_training_tests;
pub mod storage_engine_pq_tests;
pub mod storage_vs_stateless_quantization_tests;
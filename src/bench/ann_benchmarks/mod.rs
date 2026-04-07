// ANN-Benchmarks adapter for ProximaDB
//
// Provides standardized interface for ANN-benchmarks competition.

/// Adapter bridging ProximaDB to the ANN-benchmarks evaluation harness.
pub mod adapter;

// Re-export all types
pub use adapter::*;

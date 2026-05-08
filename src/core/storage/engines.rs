//! Storage engines and distance metrics

// Use the canonical DistanceMetric from compute distance module
pub use crate::compute::distance_computation::DistanceMetric;

// Use the canonical StorageEngine from the extracted proto crate.
pub use crate::proto::proximadb_v1::StorageEngine;

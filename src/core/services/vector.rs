//! Vector service types - Clean API layer over Avro unified types
//! 
//! This module provides stable type aliases for the underlying Avro types,
//! ensuring a clean API while maintaining zero-cost abstractions.

/// Primary vector record type - zero-cost alias to Avro unified type
/// 
/// This provides a stable API over the underlying Avro implementation
/// and ensures all vector operations use the same serializable type.
pub type VectorRecord = crate::core::avro_unified::VectorRecord;

// Additional vector-related type aliases can be added here
// All underlying implementations should remain in avro_unified.rs
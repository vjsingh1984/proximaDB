//! ⚠️  OBSOLETE - DO NOT USE! ⚠️
//! 
//! This file contains DEPRECATED VectorRecord definitions that cause import confusion.
//! 
//! USE INSTEAD: crate::core::avro_unified::VectorRecord
//! 
//! The avro_unified module is the single source of truth for all VectorRecord types.

/// This type alias redirects to the correct implementation to help with migration
pub type VectorRecord = crate::core::avro_unified::VectorRecord;

// No additional types or implementations should be added here.
// All VectorRecord functionality should be in avro_unified.rs
//! I/O infrastructure for storage engines
//!
//! Provides zero-copy I/O, filesystem abstractions, and bandwidth optimization

// Zero-copy I/O system is now internal - functionality integrated into UnifiedCachingFilesystem
pub(crate) mod zero_copy;

// These types are now internal - access through UnifiedCachingFilesystem instead

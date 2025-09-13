//! Memtable Implementation Modules
//!
//! Production data structure implementations optimized for specific workload characteristics.

// Production implementations
pub mod btree;                // B-tree implementation for ordered data
pub mod global_partitioned;   // ✅ ACTIVE - Primary write buffer implementation
pub mod skiplist;            // Skip list implementation for concurrent access

// Unit tests
#[cfg(test)]
pub mod tests;

//! Memtable Implementation Modules
//!
//! Production data structure implementations optimized for specific workload characteristics.

// Production implementations
pub mod btree; // B-tree implementation for ordered data
pub mod global_partitioned; // ✅ ACTIVE - Primary write buffer implementation
pub mod graph_memtable;
pub mod skiplist; // Skip list implementation for concurrent access // Graph-specific memtable with CSR optimization

// Note: Test modules have been inlined into their respective implementations

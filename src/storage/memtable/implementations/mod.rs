//! Memtable Implementation Modules
//!
//! Pure data structure implementations without specialized behaviors.
//! Each implementation is optimized for specific workload characteristics.

// 🔴 UNUSED MEMTABLE IMPLEMENTATIONS - COMMENTED OUT FOR REMOVAL
// Only global_partitioned is actually used in production
// ~1,200 lines of unused implementations - Safe to remove
// pub mod bplustree;  // UNUSED - Complex B+ tree never used
// pub mod btree;      // UNUSED - Standard B-tree never used  
// pub mod dashmap;    // UNUSED - Concurrent HashMap wrapper never used
pub mod global_partitioned; // ✅ ACTIVE - Used in write buffer
// pub mod hashmap;    // UNUSED - HashMap wrapper never used
// pub mod skiplist;   // UNUSED - Skip list implementation never used
// pub mod artmap;     // Already commented out due to type inference issues

// Unit tests
#[cfg(test)]
pub mod tests;

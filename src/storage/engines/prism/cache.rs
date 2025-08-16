//! PRISM Cache - Caching structures for PRISM engine

use std::time::Instant;

/// Cached tree node with memory optimization
pub struct CachedNode {
    pub level: u8,
    pub data: Vec<u8>,
    pub last_accessed: Instant,
    pub access_count: u64,
    /// Pin in memory (for L3+L4)
    pub pinned: bool,
}

impl CachedNode {
    /// Create a new cached node
    pub fn new(level: u8, data: Vec<u8>, pinned: bool) -> Self {
        Self {
            level,
            data,
            last_accessed: Instant::now(),
            access_count: 0,
            pinned,
        }
    }
    
    /// Access this node (update statistics)
    pub fn access(&mut self) {
        self.last_accessed = Instant::now();
        self.access_count += 1;
    }
}
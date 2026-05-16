//! Shared resources for all storage engines
//! 
//! This module provides shared resources that should be reused across all storage engines
//! to maximize efficiency and minimize resource duplication.

use std::sync::Arc;
use once_cell::sync::Lazy;
use proximadb_runtime_common::pool::VectorMemoryPool;

/// Global shared memory pool for all storage engines
/// This ensures memory buffers are reused across engines for maximum efficiency
pub static SHARED_MEMORY_POOL: Lazy<Arc<VectorMemoryPool>> = Lazy::new(|| {
    Arc::new(VectorMemoryPool::new())
});

/// Get the shared memory pool instance
/// 
/// All storage engines should use this instead of creating their own pools
/// to enable cross-engine memory buffer reuse.
pub fn get_shared_memory_pool() -> Arc<VectorMemoryPool> {
    SHARED_MEMORY_POOL.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_memory_pool_singleton() {
        let pool1 = get_shared_memory_pool();
        let pool2 = get_shared_memory_pool();
        assert!(Arc::ptr_eq(&pool1, &pool2));
    }
}
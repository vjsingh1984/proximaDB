pub mod adaptive_replacement;
pub mod least_frequently_used;
pub mod least_recently_used;

use std::hash::Hash;

pub use adaptive_replacement::ARCStrategy;
pub use least_frequently_used::LFUStrategy;
pub use least_recently_used::LRUStrategy;

/// Cache state information for eviction decisions
pub struct CacheState {
    pub total_capacity: usize,
    pub current_size: usize,
    pub entry_count: usize,
}

/// Trait for cache eviction strategies
pub trait EvictionStrategy: Send + Sync {
    type Key: Hash + Eq + Clone;

    /// Select a victim for eviction based on the strategy
    fn select_victim(&self, cache_state: &CacheState) -> Option<Self::Key>;

    /// Update strategy state when a key is accessed
    fn update_on_access(&mut self, key: &Self::Key);

    /// Update strategy state when a new key is inserted
    fn update_on_insert(&mut self, key: &Self::Key, size: usize);

    /// Update strategy state when a key is removed
    fn update_on_evict(&mut self, key: &Self::Key);

    /// Get the current strategy statistics
    fn stats(&self) -> EvictionStats;
}

/// Statistics for eviction strategies
#[derive(Debug, Clone, Default)]
pub struct EvictionStats {
    pub total_evictions: u64,
    pub total_accesses: u64,
    pub hit_rate: f64,
}

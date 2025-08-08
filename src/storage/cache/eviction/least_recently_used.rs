use super::{CacheState, EvictionStats, EvictionStrategy};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::RwLock;

/// Least Recently Used (LRU) eviction strategy
pub struct LRUStrategy<K: Hash + Eq + Clone> {
    access_order: RwLock<VecDeque<K>>,
    key_positions: RwLock<HashMap<K, usize>>,
    stats: RwLock<EvictionStats>,
}

impl<K: Hash + Eq + Clone> LRUStrategy<K> {
    pub fn new() -> Self {
        Self {
            access_order: RwLock::new(VecDeque::new()),
            key_positions: RwLock::new(HashMap::new()),
            stats: RwLock::new(EvictionStats::default()),
        }
    }
    
    fn move_to_front(&self, key: &K) {
        let mut order = self.access_order.write().unwrap();
        let mut positions = self.key_positions.write().unwrap();
        
        // Remove from current position if exists
        if let Some(&pos) = positions.get(key) {
            order.remove(pos);
            
            // Update positions for all keys after the removed one
            for (k, p) in positions.iter_mut() {
                if *p > pos {
                    *p -= 1;
                }
            }
        }
        
        // Add to front
        order.push_front(key.clone());
        positions.insert(key.clone(), 0);
        
        // Update positions for all other keys
        for (k, p) in positions.iter_mut() {
            if k != key {
                *p += 1;
            }
        }
    }
}

impl<K: Hash + Eq + Clone + Send + Sync> EvictionStrategy for LRUStrategy<K> {
    type Key = K;
    
    fn select_victim(&self, _cache_state: &CacheState) -> Option<Self::Key> {
        let order = self.access_order.read().unwrap();
        order.back().cloned()
    }
    
    fn update_on_access(&mut self, key: &Self::Key) {
        self.move_to_front(key);
        
        let mut stats = self.stats.write().unwrap();
        stats.total_accesses += 1;
    }
    
    fn update_on_insert(&mut self, key: &Self::Key, _size: usize) {
        self.move_to_front(key);
    }
    
    fn update_on_evict(&mut self, key: &Self::Key) {
        let mut order = self.access_order.write().unwrap();
        let mut positions = self.key_positions.write().unwrap();
        
        if let Some(&pos) = positions.get(key) {
            order.remove(pos);
            positions.remove(key);
            
            // Update positions for remaining keys
            for (_, p) in positions.iter_mut() {
                if *p > pos {
                    *p -= 1;
                }
            }
        }
        
        let mut stats = self.stats.write().unwrap();
        stats.total_evictions += 1;
    }
    
    fn stats(&self) -> EvictionStats {
        self.stats.read().unwrap().clone()
    }
}

impl<K: Hash + Eq + Clone> Default for LRUStrategy<K> {
    fn default() -> Self {
        Self::new()
    }
}
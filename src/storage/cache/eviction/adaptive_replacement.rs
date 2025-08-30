use super::{CacheState, EvictionStats, EvictionStrategy};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::RwLock;

/// Adaptive Replacement Cache (ARC) eviction strategy
/// Combines recency and frequency to adapt to workload patterns
pub struct ARCStrategy<K: Hash + Eq + Clone> {
    // T1: Recent cache entries (LRU)
    t1: RwLock<VecDeque<K>>,
    // T2: Frequent cache entries (LFU-like)
    t2: RwLock<VecDeque<K>>,
    // B1: Ghost entries recently evicted from T1
    b1: RwLock<VecDeque<K>>,
    // B2: Ghost entries recently evicted from T2
    b2: RwLock<VecDeque<K>>,
    
    // Adaptive parameter (target size for T1)
    p: RwLock<f64>,
    
    // Quick lookup maps
    location_map: RwLock<HashMap<K, CacheLocation>>,
    
    // Statistics
    stats: RwLock<EvictionStats>,
    
    // Maximum cache size
    cache_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CacheLocation {
    T1,
    T2,
    B1,
    B2,
    NotInCache,
}

impl<K: Hash + Eq + Clone> ARCStrategy<K> {
    pub fn new(cache_size: usize) -> Self {
        Self {
            t1: RwLock::new(VecDeque::new()),
            t2: RwLock::new(VecDeque::new()),
            b1: RwLock::new(VecDeque::new()),
            b2: RwLock::new(VecDeque::new()),
            p: RwLock::new(cache_size as f64 / 2.0),
            location_map: RwLock::new(HashMap::new()),
            stats: RwLock::new(EvictionStats::default()),
            cache_size,
        }
    }
    
    fn adapt(&self, delta: f64) {
        let mut p = self.p.write().unwrap();
        *p = (*p + delta).max(0.0).min(self.cache_size as f64);
    }
    
    fn move_to_t2(&self, key: &K) {
        let mut t1 = self.t1.write().unwrap();
        let mut t2 = self.t2.write().unwrap();
        let mut locations = self.location_map.write().unwrap();
        
        // Remove from T1
        if let Some(pos) = t1.iter().position(|k| k == key) {
            t1.remove(pos);
        }
        
        // Add to T2 front (MRU position)
        t2.push_front(key.clone());
        locations.insert(key.clone(), CacheLocation::T2);
    }
}

impl<K: Hash + Eq + Clone + Send + Sync> EvictionStrategy for ARCStrategy<K> {
    type Key = K;
    
    fn select_victim(&self, _cache_state: &CacheState) -> Option<Self::Key> {
        let t1 = self.t1.read().unwrap();
        let t2 = self.t2.read().unwrap();
        let p = *self.p.read().unwrap();
        
        let t1_target_size = p.round() as usize;
        
        // Prefer evicting from T1 if it's above target size
        if t1.len() > t1_target_size {
            t1.back().cloned()
        } else {
            // Otherwise evict from T2
            t2.back().cloned()
        }
    }
    
    fn update_on_access(&mut self, key: &Self::Key) {
        let locations = self.location_map.read().unwrap();
        let location = locations.get(key).copied();
        drop(locations);
        
        match location {
            Some(CacheLocation::T1) => {
                // Move from T1 to T2 (promote to frequent)
                self.move_to_t2(key);
            }
            Some(CacheLocation::T2) => {
                // Move to front of T2
                let mut t2 = self.t2.write().unwrap();
                if let Some(pos) = t2.iter().position(|k| k == key) {
                    t2.remove(pos);
                    t2.push_front(key.clone());
                }
            }
            Some(CacheLocation::B1) => {
                // Ghost hit in B1: increase p (favor recency)
                let b2_len = self.b2.read().unwrap().len();
                let b1_len = self.b1.read().unwrap().len();
                let delta = if b2_len > 0 {
                    (b1_len as f64 / b2_len as f64).min(1.0)
                } else {
                    1.0
                };
                self.adapt(delta);
            }
            Some(CacheLocation::B2) => {
                // Ghost hit in B2: decrease p (favor frequency)
                let b1_len = self.b1.read().unwrap().len();
                let b2_len = self.b2.read().unwrap().len();
                let delta = if b1_len > 0 {
                    -(b2_len as f64 / b1_len as f64).min(1.0)
                } else {
                    -1.0
                };
                self.adapt(delta);
            }
            Some(CacheLocation::NotInCache) | None => {}
        }
        
        let mut stats = self.stats.write().unwrap();
        stats.total_accesses += 1;
    }
    
    fn update_on_insert(&mut self, key: &Self::Key, _size: usize) {
        let mut t1 = self.t1.write().unwrap();
        let mut locations = self.location_map.write().unwrap();
        
        // New entries go to T1 (recent)
        t1.push_front(key.clone());
        locations.insert(key.clone(), CacheLocation::T1);
    }
    
    fn update_on_evict(&mut self, key: &Self::Key) {
        let mut locations = self.location_map.write().unwrap();
        let location = locations.get(key).copied();
        
        match location {
            Some(CacheLocation::T1) => {
                let mut t1 = self.t1.write().unwrap();
                let mut b1 = self.b1.write().unwrap();
                
                if let Some(pos) = t1.iter().position(|k| k == key) {
                    t1.remove(pos);
                    // Add to ghost list B1
                    b1.push_front(key.clone());
                    locations.insert(key.clone(), CacheLocation::B1);
                }
            }
            Some(CacheLocation::T2) => {
                let mut t2 = self.t2.write().unwrap();
                let mut b2 = self.b2.write().unwrap();
                
                if let Some(pos) = t2.iter().position(|k| k == key) {
                    t2.remove(pos);
                    // Add to ghost list B2
                    b2.push_front(key.clone());
                    locations.insert(key.clone(), CacheLocation::B2);
                }
            }
            _ => {}
        }
        
        let mut stats = self.stats.write().unwrap();
        stats.total_evictions += 1;
    }
    
    fn stats(&self) -> EvictionStats {
        let stats = self.stats.read().unwrap();
        let mut result = stats.clone();
        
        // Calculate hit rate
        if stats.total_accesses > 0 {
            let t1_size = self.t1.read().unwrap().len();
            let t2_size = self.t2.read().unwrap().len();
            let cache_size = t1_size + t2_size;
            result.hit_rate = cache_size as f64 / self.cache_size as f64;
        }
        
        result
    }
}
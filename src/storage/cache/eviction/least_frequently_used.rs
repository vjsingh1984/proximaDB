use super::{CacheState, EvictionStats, EvictionStrategy};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hash;
use std::sync::RwLock;

/// Least Frequently Used (LFU) eviction strategy
pub struct LFUStrategy<K: Hash + Eq + Clone> {
    frequency_map: RwLock<HashMap<K, usize>>,
    frequency_lists: RwLock<BTreeMap<usize, HashSet<K>>>,
    min_frequency: RwLock<usize>,
    stats: RwLock<EvictionStats>,
}

impl<K: Hash + Eq + Clone> LFUStrategy<K> {
    pub fn new() -> Self {
        Self {
            frequency_map: RwLock::new(HashMap::new()),
            frequency_lists: RwLock::new(BTreeMap::new()),
            min_frequency: RwLock::new(0),
            stats: RwLock::new(EvictionStats::default()),
        }
    }
    
    fn update_frequency(&self, key: &K, increment: bool) {
        let mut freq_map = self.frequency_map.write().unwrap();
        let mut freq_lists = self.frequency_lists.write().unwrap();
        let mut min_freq = self.min_frequency.write().unwrap();
        
        let old_freq = freq_map.get(key).copied().unwrap_or(0);
        let new_freq = if increment { old_freq + 1 } else { 1 };
        
        // Remove from old frequency list
        if old_freq > 0 {
            if let Some(keys) = freq_lists.get_mut(&old_freq) {
                keys.remove(key);
                if keys.is_empty() {
                    freq_lists.remove(&old_freq);
                    if *min_freq == old_freq {
                        *min_freq = freq_lists.keys().next().copied().unwrap_or(0);
                    }
                }
            }
        }
        
        // Add to new frequency list
        freq_lists
            .entry(new_freq)
            .or_insert_with(HashSet::new)
            .insert(key.clone());
        
        freq_map.insert(key.clone(), new_freq);
        
        // Update minimum frequency
        if new_freq < *min_freq || *min_freq == 0 {
            *min_freq = new_freq;
        }
    }
}

impl<K: Hash + Eq + Clone + Send + Sync> EvictionStrategy for LFUStrategy<K> {
    type Key = K;
    
    fn select_victim(&self, _cache_state: &CacheState) -> Option<Self::Key> {
        let freq_lists = self.frequency_lists.read().unwrap();
        
        // Find the list with minimum frequency
        freq_lists
            .iter()
            .next()
            .and_then(|(_, keys)| keys.iter().next().cloned())
    }
    
    fn update_on_access(&mut self, key: &Self::Key) {
        self.update_frequency(key, true);
        
        let mut stats = self.stats.write().unwrap();
        stats.total_accesses += 1;
    }
    
    fn update_on_insert(&mut self, key: &Self::Key, _size: usize) {
        self.update_frequency(key, false);
    }
    
    fn update_on_evict(&mut self, key: &Self::Key) {
        let mut freq_map = self.frequency_map.write().unwrap();
        let mut freq_lists = self.frequency_lists.write().unwrap();
        
        if let Some(&freq) = freq_map.get(key) {
            if let Some(keys) = freq_lists.get_mut(&freq) {
                keys.remove(key);
                if keys.is_empty() {
                    freq_lists.remove(&freq);
                }
            }
            freq_map.remove(key);
        }
        
        let mut stats = self.stats.write().unwrap();
        stats.total_evictions += 1;
    }
    
    fn stats(&self) -> EvictionStats {
        self.stats.read().unwrap().clone()
    }
}

impl<K: Hash + Eq + Clone> Default for LFUStrategy<K> {
    fn default() -> Self {
        Self::new()
    }
}
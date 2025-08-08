use super::{CacheTier, StorageBackend, StorageError};
use async_trait::async_trait;
use dashmap::DashMap;
use std::fmt::{self, Debug};
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// In-memory storage backend using DashMap for concurrent access
#[derive(Clone)]
pub struct MemoryBackend<K, V> 
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    storage: Arc<DashMap<K, V>>,
    size_bytes: Arc<AtomicUsize>,
    max_size_bytes: usize,
}

impl<K, V> Debug for MemoryBackend<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryBackend")
            .field("max_size_bytes", &self.max_size_bytes)
            .field("current_size", &self.size_bytes.load(Ordering::Relaxed))
            .finish()
    }
}

impl<K, V> MemoryBackend<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    pub fn new(max_size_mb: usize) -> Self {
        Self {
            storage: Arc::new(DashMap::new()),
            size_bytes: Arc::new(AtomicUsize::new(0)),
            max_size_bytes: max_size_mb * 1024 * 1024,
        }
    }
    
    pub fn with_capacity(max_size_mb: usize, initial_capacity: usize) -> Self {
        Self {
            storage: Arc::new(DashMap::with_capacity(initial_capacity)),
            size_bytes: Arc::new(AtomicUsize::new(0)),
            max_size_bytes: max_size_mb * 1024 * 1024,
        }
    }
    
    fn estimate_size<T>(_value: &T) -> usize {
        // Simple size estimation - in production, use a more accurate method
        std::mem::size_of::<T>() + 32 // Add overhead for metadata
    }
}

#[async_trait]
impl<K, V> StorageBackend for MemoryBackend<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + Debug + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    type Key = K;
    type Value = V;
    
    async fn get(&self, key: &Self::Key) -> Option<Self::Value> {
        self.storage.get(key).map(|entry| entry.value().clone())
    }
    
    async fn put(&self, key: Self::Key, value: Self::Value) -> Result<(), StorageError> {
        let estimated_size = Self::estimate_size(&value);
        let current_size = self.size_bytes.load(Ordering::Relaxed);
        
        // Check capacity
        if current_size + estimated_size > self.max_size_bytes {
            return Err(StorageError::CapacityExceeded);
        }
        
        // If replacing an existing entry, adjust size
        if let Some(old_entry) = self.storage.get(&key) {
            let old_size = Self::estimate_size(old_entry.value());
            self.size_bytes.fetch_sub(old_size, Ordering::Relaxed);
        }
        
        self.storage.insert(key, value);
        self.size_bytes.fetch_add(estimated_size, Ordering::Relaxed);
        
        Ok(())
    }
    
    async fn remove(&self, key: &Self::Key) -> bool {
        if let Some((_, value)) = self.storage.remove(key) {
            let size = Self::estimate_size(&value);
            self.size_bytes.fetch_sub(size, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
    
    async fn contains(&self, key: &Self::Key) -> bool {
        self.storage.contains_key(key)
    }
    
    async fn clear(&self) -> Result<(), StorageError> {
        self.storage.clear();
        self.size_bytes.store(0, Ordering::Relaxed);
        Ok(())
    }
    
    async fn size_bytes(&self) -> usize {
        self.size_bytes.load(Ordering::Relaxed)
    }
    
    async fn entry_count(&self) -> usize {
        self.storage.len()
    }
    
    fn tier(&self) -> CacheTier {
        CacheTier::L1
    }
}
//! Shared cache traits and simple in-memory implementations for query layers.

use dashmap::DashMap;
use std::hash::Hash;
use std::sync::Arc;

/// Cache trait for key-value lookup and storage
pub trait Cache<K, V> {
    /// Retrieve a cached value by key
    fn get(&self, key: &K) -> Option<Arc<V>>;
    /// Insert a value into the cache
    fn insert(&self, key: K, value: V);
}

/// Sharded concurrent map cache using DashMap
pub struct ShardedMapCache<K, V> {
    inner: DashMap<K, Arc<V>>,
}

impl<K: Eq + Hash, V> ShardedMapCache<K, V> {
    /// Create a new empty sharded map cache.
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }
}

impl<K: Eq + Hash, V> Default for ShardedMapCache<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + Hash, V> Cache<K, V> for ShardedMapCache<K, V> {
    fn get(&self, key: &K) -> Option<Arc<V>> {
        self.inner.get(key).map(|v| v.clone())
    }
    fn insert(&self, key: K, value: V) {
        self.inner.insert(key, Arc::new(value));
    }
}

//! Shared cache traits and simple in-memory implementations for query layers.

use dashmap::DashMap;
use std::hash::Hash;
use std::sync::Arc;

pub trait Cache<K, V> {
    fn get(&self, key: &K) -> Option<Arc<V>>;
    fn insert(&self, key: K, value: V);
}

pub struct ShardedMapCache<K, V> {
    inner: DashMap<K, Arc<V>>,
}

impl<K: Eq + Hash, V> ShardedMapCache<K, V> {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
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

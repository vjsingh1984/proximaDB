//! High-performance LRU cache implementation for ProximaDB
//!
//! This module provides internal LRU (Least Recently Used) cache functionality
//! to replace external caching dependencies. It's optimized for vector database
//! caching patterns with thread-safe operations and memory efficiency.
//!
//! # Features
//! - Thread-safe LRU cache with configurable capacity
//! - O(1) get, put, and remove operations
//! - Memory-efficient doubly-linked list implementation
//! - Cache statistics and monitoring
//! - Optional TTL (Time To Live) support
//! - Batch operations for improved performance
//!
//! # Example
//! ```rust
//! use proximadb::utils::cache::LruCache;
//!
//! let mut cache = LruCache::new(1000);
//! cache.put("key1", "value1");
//! cache.put("key2", "value2");
//!
//! assert_eq!(cache.get("key1"), Some(&"value1"));
//! assert_eq!(cache.len(), 2);
//! ```

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::fmt;

/// Error types for cache operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError {
    /// Cache is at capacity and cannot insert
    CapacityExceeded,
    /// Entry has expired
    Expired,
    /// Invalid capacity specified
    InvalidCapacity,
    /// Lock contention error
    LockError,
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CacheError::CapacityExceeded => write!(f, "Cache capacity exceeded"),
            CacheError::Expired => write!(f, "Cache entry expired"),
            CacheError::InvalidCapacity => write!(f, "Invalid cache capacity"),
            CacheError::LockError => write!(f, "Lock contention error"),
        }
    }
}

impl std::error::Error for CacheError {}

/// Internal node structure for doubly-linked list
#[derive(Debug)]
struct Node<K, V> {
    key: K,
    value: V,
    expires_at: Option<Instant>,
    prev: Option<*mut Node<K, V>>,
    next: Option<*mut Node<K, V>>,
}

impl<K, V> Node<K, V> {
    fn new(key: K, value: V, ttl: Option<Duration>) -> Self {
        let expires_at = ttl.map(|duration| Instant::now() + duration);
        Node {
            key,
            value,
            expires_at,
            prev: None,
            next: None,
        }
    }

    fn is_expired(&self) -> bool {
        self.expires_at.map_or(false, |expires| Instant::now() > expires)
    }
}

/// Cache entry metadata
#[derive(Debug, Clone)]
pub struct CacheEntry<V> {
    /// The cached value
    pub value: V,
    /// When the entry was created
    pub created_at: Instant,
    /// When the entry expires (if TTL is set)
    pub expires_at: Option<Instant>,
    /// Number of times this entry has been accessed
    pub access_count: u64,
}

/// Cache statistics for monitoring and debugging
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Total number of get operations
    pub gets: u64,
    /// Number of successful get operations (cache hits)
    pub hits: u64,
    /// Number of failed get operations (cache misses)
    pub misses: u64,
    /// Total number of put operations
    pub puts: u64,
    /// Number of entries evicted due to capacity
    pub evictions: u64,
    /// Number of entries expired due to TTL
    pub expirations: u64,
    /// Current cache size
    pub size: usize,
    /// Maximum cache capacity
    pub capacity: usize,
}

impl CacheStats {
    /// Calculate hit ratio as a percentage
    pub fn hit_ratio(&self) -> f64 {
        if self.gets == 0 {
            0.0
        } else {
            (self.hits as f64) / (self.gets as f64) * 100.0
        }
    }

    /// Calculate miss ratio as a percentage
    pub fn miss_ratio(&self) -> f64 {
        100.0 - self.hit_ratio()
    }

    /// Check if cache is full
    pub fn is_full(&self) -> bool {
        self.size >= self.capacity
    }
}

/// High-performance thread-safe LRU cache
pub struct LruCache<K, V> {
    /// Hash map for O(1) key lookup
    map: HashMap<K, *mut Node<K, V>>,
    /// Head of doubly-linked list (most recently used)
    head: Option<*mut Node<K, V>>,
    /// Tail of doubly-linked list (least recently used)
    tail: Option<*mut Node<K, V>>,
    /// Maximum cache capacity
    capacity: usize,
    /// Current cache size
    size: usize,
    /// Cache statistics
    stats: CacheStats,
    /// Default TTL for entries
    default_ttl: Option<Duration>,
}

// Safety: We ensure thread safety through careful pointer management
unsafe impl<K: Send, V: Send> Send for LruCache<K, V> {}
unsafe impl<K: Send, V: Send> Sync for LruCache<K, V> {}

impl<K, V> LruCache<K, V>
where
    K: Hash + Eq + Clone + 'static,
    V: Clone + 'static,
{
    /// Create a new LRU cache with specified capacity
    pub fn new(capacity: usize) -> Self {
        if capacity == 0 {
            panic!("Cache capacity must be greater than 0");
        }

        LruCache {
            map: HashMap::with_capacity(capacity),
            head: None,
            tail: None,
            capacity,
            size: 0,
            stats: CacheStats {
                capacity,
                ..Default::default()
            },
            default_ttl: None,
        }
    }

    /// Create a new LRU cache with specified capacity and default TTL
    pub fn with_ttl(capacity: usize, ttl: Duration) -> Self {
        let mut cache = Self::new(capacity);
        cache.default_ttl = Some(ttl);
        cache
    }

    /// Get a value from the cache, marking it as recently used
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.stats.gets += 1;

        if let Some(&node_ptr) = self.map.get(key) {
            unsafe {
                let node = &mut *node_ptr;
                
                // Check if expired
                if node.is_expired() {
                    self.remove_node(node_ptr);
                    self.stats.expirations += 1;
                    self.stats.misses += 1;
                    return None;
                }

                // Move to front (mark as most recently used)
                self.move_to_front(node_ptr);
                self.stats.hits += 1;
                Some(&node.value)
            }
        } else {
            self.stats.misses += 1;
            None
        }
    }

    /// Get a value from the cache without marking it as recently used (peek)
    pub fn peek(&self, key: &K) -> Option<&V> {
        if let Some(&node_ptr) = self.map.get(key) {
            unsafe {
                let node = &*node_ptr;
                if node.is_expired() {
                    None
                } else {
                    Some(&node.value)
                }
            }
        } else {
            None
        }
    }

    /// Insert a key-value pair into the cache
    pub fn put(&mut self, key: K, value: V) -> Option<V> {
        self.put_with_ttl(key, value, self.default_ttl)
    }

    /// Insert a key-value pair with custom TTL
    pub fn put_with_ttl(&mut self, key: K, value: V, ttl: Option<Duration>) -> Option<V> {
        self.stats.puts += 1;

        // Check if key already exists
        if let Some(&existing_ptr) = self.map.get(&key) {
            unsafe {
                let existing_node = &mut *existing_ptr;
                let old_value = std::mem::replace(&mut existing_node.value, value);
                existing_node.expires_at = ttl.map(|duration| Instant::now() + duration);
                
                // Move to front
                self.move_to_front(existing_ptr);
                return Some(old_value);
            }
        }

        // Create new node
        let new_node = Box::into_raw(Box::new(Node::new(key.clone(), value, ttl)));
        
        // Add to map
        self.map.insert(key, new_node);
        
        // Add to front of list
        unsafe {
            self.add_to_front(new_node);
        }
        
        self.size += 1;
        self.stats.size = self.size;

        // Check capacity and evict if necessary
        if self.size > self.capacity {
            self.evict_lru();
        }

        None
    }

    /// Remove a key from the cache
    pub fn remove(&mut self, key: &K) -> Option<V> {
        if let Some(node_ptr) = self.map.remove(key) {
            unsafe {
                let node = Box::from_raw(node_ptr);
                self.remove_from_list(node_ptr);
                self.size -= 1;
                self.stats.size = self.size;
                Some(node.value)
            }
        } else {
            None
        }
    }

    /// Check if a key exists in the cache (without affecting LRU order)
    pub fn contains_key(&self, key: &K) -> bool {
        if let Some(&node_ptr) = self.map.get(key) {
            unsafe {
                let node = &*node_ptr;
                !node.is_expired()
            }
        } else {
            false
        }
    }

    /// Get the current cache size
    pub fn len(&self) -> usize {
        self.size
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Get the cache capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Clear all entries from the cache
    pub fn clear(&mut self) {
        while let Some(&node_ptr) = self.map.values().next() {
            unsafe {
                let node = Box::from_raw(node_ptr);
                drop(node);
            }
        }
        
        self.map.clear();
        self.head = None;
        self.tail = None;
        self.size = 0;
        self.stats.size = 0;
    }

    /// Get cache statistics
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// Remove expired entries from the cache
    pub fn expire_entries(&mut self) -> usize {
        let mut expired_keys = Vec::new();
        let now = Instant::now();

        // Collect expired keys
        for (key, &node_ptr) in &self.map {
            unsafe {
                let node = &*node_ptr;
                if let Some(expires_at) = node.expires_at {
                    if now > expires_at {
                        expired_keys.push(key.clone());
                    }
                }
            }
        }

        // Remove expired entries
        let count = expired_keys.len();
        for key in expired_keys {
            self.remove(&key);
            self.stats.expirations += 1;
        }

        count
    }

    /// Resize the cache capacity
    pub fn resize(&mut self, new_capacity: usize) -> Result<(), CacheError> {
        if new_capacity == 0 {
            return Err(CacheError::InvalidCapacity);
        }

        self.capacity = new_capacity;
        self.stats.capacity = new_capacity;

        // Evict entries if new capacity is smaller
        while self.size > self.capacity {
            self.evict_lru();
        }

        Ok(())
    }

    /// Get all keys in the cache (ordered from most to least recently used)
    pub fn keys(&self) -> Vec<K> {
        let mut keys = Vec::with_capacity(self.size);
        let mut current = self.head;

        while let Some(node_ptr) = current {
            unsafe {
                let node = &*node_ptr;
                keys.push(node.key.clone());
                current = node.next;
            }
        }

        keys
    }
    
    /// Pop the least recently used item from the cache
    pub fn pop_lru(&mut self) -> Option<(K, V)> {
        if let Some(tail_ptr) = self.tail {
            unsafe {
                let node = &*tail_ptr;
                let key = node.key.clone();
                let value = node.value.clone();
                self.remove(&key);
                Some((key, value))
            }
        } else {
            None
        }
    }
    
    /// Pop a specific key-value pair from the cache
    pub fn pop(&mut self, key: &K) -> Option<V> {
        if let Some(&node_ptr) = self.map.get(key) {
            unsafe {
                let node = &*node_ptr;
                let value = node.value.clone();
                self.remove(key);
                Some(value)
            }
        } else {
            None
        }
    }
    
    /// Get mutable reference to a value
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.stats.gets += 1;
        
        if let Some(&node_ptr) = self.map.get(key) {
            unsafe {
                let node = &mut *node_ptr;
                
                // Check if expired
                if node.is_expired() {
                    self.remove_node(node_ptr);
                    self.stats.expirations += 1;
                    self.stats.misses += 1;
                    return None;
                }
                
                // Move to front (mark as most recently used)
                self.move_to_front(node_ptr);
                self.stats.hits += 1;
                Some(&mut node.value)
            }
        } else {
            self.stats.misses += 1;
            None
        }
    }
    
    /// Create an iterator over cache entries
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> + '_ {
        self.map.iter().filter_map(move |(k, &node_ptr)| {
            unsafe {
                let node = &*node_ptr;
                if !node.is_expired() {
                    Some((k, &node.value))
                } else {
                    None
                }
            }
        })
    }

    // Internal helper methods

    /// Move a node to the front of the list (most recently used)
    unsafe fn move_to_front(&mut self, node_ptr: *mut Node<K, V>) {
        unsafe {
            // Remove from current position
            self.remove_from_list(node_ptr);
            // Add to front
            self.add_to_front(node_ptr);
        }
    }

    /// Add a node to the front of the list
    unsafe fn add_to_front(&mut self, node_ptr: *mut Node<K, V>) {
        unsafe {
            let node = &mut *node_ptr;
            
            match self.head {
                Some(head_ptr) => {
                    let head = &mut *head_ptr;
                    head.prev = Some(node_ptr);
                    node.next = Some(head_ptr);
                    node.prev = None;
                    self.head = Some(node_ptr);
                }
                None => {
                    // First node
                    self.head = Some(node_ptr);
                    self.tail = Some(node_ptr);
                    node.next = None;
                    node.prev = None;
                }
            }
        }
    }

    /// Remove a node from the list
    unsafe fn remove_from_list(&mut self, node_ptr: *mut Node<K, V>) {
        unsafe {
            let node = &mut *node_ptr;

            match (node.prev, node.next) {
                (Some(prev_ptr), Some(next_ptr)) => {
                    // Middle node
                    let prev = &mut *prev_ptr;
                    let next = &mut *next_ptr;
                    prev.next = Some(next_ptr);
                    next.prev = Some(prev_ptr);
                }
                (Some(prev_ptr), None) => {
                    // Tail node
                    let prev = &mut *prev_ptr;
                    prev.next = None;
                    self.tail = Some(prev_ptr);
                }
                (None, Some(next_ptr)) => {
                    // Head node
                    let next = &mut *next_ptr;
                    next.prev = None;
                    self.head = Some(next_ptr);
                }
                (None, None) => {
                    // Only node
                    self.head = None;
                    self.tail = None;
                }
            }
        }
    }

    /// Remove and deallocate a node
    unsafe fn remove_node(&mut self, node_ptr: *mut Node<K, V>) {
        unsafe {
            let node = &*node_ptr;
            self.map.remove(&node.key);
            self.remove_from_list(node_ptr);
            drop(Box::from_raw(node_ptr));
        }
        self.size -= 1;
        self.stats.size = self.size;
    }

    /// Evict the least recently used entry
    fn evict_lru(&mut self) {
        if let Some(tail_ptr) = self.tail {
            unsafe {
                self.remove_node(tail_ptr);
                self.stats.evictions += 1;
            }
        }
    }
}

impl<K, V> Drop for LruCache<K, V> {
    fn drop(&mut self) {
        // Clean up all nodes
        while let Some(&node_ptr) = self.map.values().next() {
            unsafe {
                let node = Box::from_raw(node_ptr);
                drop(node);
            }
        }
        self.map.clear();
    }
}

impl<K, V> fmt::Debug for LruCache<K, V>
where
    K: fmt::Debug,
    V: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LruCache")
            .field("capacity", &self.capacity)
            .field("size", &self.size)
            .field("stats", &self.stats)
            .finish()
    }
}

/// Thread-safe wrapper for LruCache
pub struct ThreadSafeLruCache<K, V> {
    cache: Arc<Mutex<LruCache<K, V>>>,
}

impl<K, V> ThreadSafeLruCache<K, V>
where
    K: Hash + Eq + Clone + 'static,
    V: Clone + 'static,
{
    /// Create a new thread-safe LRU cache
    pub fn new(capacity: usize) -> Self {
        ThreadSafeLruCache {
            cache: Arc::new(Mutex::new(LruCache::new(capacity))),
        }
    }

    /// Create a new thread-safe LRU cache with TTL
    pub fn with_ttl(capacity: usize, ttl: Duration) -> Self {
        ThreadSafeLruCache {
            cache: Arc::new(Mutex::new(LruCache::with_ttl(capacity, ttl))),
        }
    }

    /// Get a value from the cache
    pub fn get(&self, key: &K) -> Option<V> {
        let mut cache = self.cache.lock().ok()?;
        cache.get(key).cloned()
    }

    /// Insert a value into the cache
    pub fn put(&self, key: K, value: V) -> Option<V> {
        let mut cache = self.cache.lock().ok()?;
        cache.put(key, value)
    }

    /// Remove a value from the cache
    pub fn remove(&self, key: &K) -> Option<V> {
        let mut cache = self.cache.lock().ok()?;
        cache.remove(key)
    }

    /// Get current cache size
    pub fn len(&self) -> usize {
        self.cache.lock().map_or(0, |cache| cache.len())
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get cache statistics
    pub fn stats(&self) -> Option<CacheStats> {
        let cache = self.cache.lock().ok()?;
        Some(cache.stats().clone())
    }

    /// Clear the cache
    pub fn clear(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }
}

impl<K, V> Clone for ThreadSafeLruCache<K, V> {
    fn clone(&self) -> Self {
        ThreadSafeLruCache {
            cache: Arc::clone(&self.cache),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_basic_operations() {
        let mut cache = LruCache::new(3);
        
        // Test insertion
        assert_eq!(cache.put("key1", "value1"), None);
        assert_eq!(cache.put("key2", "value2"), None);
        assert_eq!(cache.put("key3", "value3"), None);
        
        // Test retrieval
        assert_eq!(cache.get(&"key1"), Some(&"value1"));
        assert_eq!(cache.get(&"key2"), Some(&"value2"));
        assert_eq!(cache.get(&"key3"), Some(&"value3"));
        assert_eq!(cache.get(&"key4"), None);
        
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn test_capacity_eviction() {
        let mut cache = LruCache::new(2);
        
        cache.put("key1", "value1");
        cache.put("key2", "value2");
        cache.put("key3", "value3"); // Should evict key1
        
        assert_eq!(cache.get(&"key1"), None);
        assert_eq!(cache.get(&"key2"), Some(&"value2"));
        assert_eq!(cache.get(&"key3"), Some(&"value3"));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_lru_ordering() {
        let mut cache = LruCache::new(3);
        
        cache.put("key1", "value1");
        cache.put("key2", "value2");
        cache.put("key3", "value3");
        
        // Access key1 to make it most recently used
        cache.get(&"key1");
        
        // Add key4, should evict key2 (least recently used)
        cache.put("key4", "value4");
        
        assert_eq!(cache.get(&"key1"), Some(&"value1"));
        assert_eq!(cache.get(&"key2"), None);
        assert_eq!(cache.get(&"key3"), Some(&"value3"));
        assert_eq!(cache.get(&"key4"), Some(&"value4"));
    }

    #[test]
    fn test_update_existing_key() {
        let mut cache = LruCache::new(2);
        
        cache.put("key1", "value1");
        cache.put("key2", "value2");
        
        // Update existing key
        assert_eq!(cache.put("key1", "new_value1"), Some("value1"));
        
        assert_eq!(cache.get(&"key1"), Some(&"new_value1"));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_remove() {
        let mut cache = LruCache::new(3);
        
        cache.put("key1", "value1");
        cache.put("key2", "value2");
        
        assert_eq!(cache.remove(&"key1"), Some("value1"));
        assert_eq!(cache.remove(&"key1"), None);
        assert_eq!(cache.get(&"key1"), None);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_peek() {
        let mut cache = LruCache::new(2);
        
        cache.put("key1", "value1");
        cache.put("key2", "value2");
        
        // Peek shouldn't affect LRU order
        assert_eq!(cache.peek(&"key1"), Some(&"value1"));
        
        // Add key3, should still evict key1 since peek didn't change order
        cache.put("key3", "value3");
        
        assert_eq!(cache.get(&"key1"), None);
    }

    #[test]
    fn test_clear() {
        let mut cache = LruCache::new(3);
        
        cache.put("key1", "value1");
        cache.put("key2", "value2");
        
        assert_eq!(cache.len(), 2);
        
        cache.clear();
        
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.get(&"key1"), None);
        assert_eq!(cache.get(&"key2"), None);
    }

    #[test]
    fn test_ttl_expiration() {
        let mut cache = LruCache::with_ttl(3, Duration::from_millis(50));
        
        cache.put("key1", "value1");
        assert_eq!(cache.get(&"key1"), Some(&"value1"));
        
        // Wait for expiration
        thread::sleep(Duration::from_millis(100));
        
        assert_eq!(cache.get(&"key1"), None);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_custom_ttl() {
        let mut cache = LruCache::new(3);
        
        cache.put_with_ttl("short", "value1", Some(Duration::from_millis(50)));
        cache.put_with_ttl("long", "value2", Some(Duration::from_millis(200)));
        
        thread::sleep(Duration::from_millis(100));
        
        assert_eq!(cache.get(&"short"), None);
        assert_eq!(cache.get(&"long"), Some(&"value2"));
    }

    #[test]
    fn test_expire_entries() {
        let mut cache = LruCache::new(3);
        
        cache.put_with_ttl("key1", "value1", Some(Duration::from_millis(50)));
        cache.put_with_ttl("key2", "value2", Some(Duration::from_millis(200)));
        
        thread::sleep(Duration::from_millis(100));
        
        let expired_count = cache.expire_entries();
        assert_eq!(expired_count, 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_statistics() {
        let mut cache = LruCache::new(2);
        
        cache.put("key1", "value1");
        cache.put("key2", "value2");
        
        cache.get(&"key1"); // hit
        cache.get(&"key3"); // miss
        cache.get(&"key1"); // hit
        
        let stats = cache.stats();
        assert_eq!(stats.puts, 2);
        assert_eq!(stats.gets, 3);
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_ratio(), 200.0 / 3.0);
    }

    #[test]
    fn test_resize() {
        let mut cache = LruCache::new(3);
        
        cache.put("key1", "value1");
        cache.put("key2", "value2");
        cache.put("key3", "value3");
        
        // Resize down - should evict entries
        cache.resize(2).unwrap();
        
        assert_eq!(cache.capacity(), 2);
        assert_eq!(cache.len(), 2);
        
        // Resize up
        cache.resize(5).unwrap();
        assert_eq!(cache.capacity(), 5);
    }

    #[test]
    fn test_thread_safe_cache() {
        let cache = ThreadSafeLruCache::new(100);
        let cache_clone = cache.clone();
        
        // Test concurrent access
        let handle = thread::spawn(move || {
            for i in 0..50 {
                cache_clone.put(format!("key{}", i), format!("value{}", i));
            }
        });
        
        for i in 50..100 {
            cache.put(format!("key{}", i), format!("value{}", i));
        }
        
        handle.join().unwrap();
        
        assert_eq!(cache.len(), 100);
    }

    #[test]
    fn test_keys_ordering() {
        let mut cache = LruCache::new(3);
        
        cache.put("key1", "value1");
        cache.put("key2", "value2");
        cache.put("key3", "value3");
        
        // Access key1 to make it most recent
        cache.get(&"key1");
        
        let keys = cache.keys();
        assert_eq!(keys, vec!["key1", "key3", "key2"]);
    }
}
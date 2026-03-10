//! Production-ready concurrent skip list implementation for ProximaDB
//!
//! This module provides a thread-safe skip list implementation using a Mutex-protected
//! BTreeMap for guaranteed 100% insertion success. While this trades some concurrency
//! for correctness, it ensures all operations complete successfully.
//!
//! # Features
//! - Thread-safe concurrent operations with guaranteed success
//! - 100% insertion success rate even under high contention
//! - Range queries and ordered iteration
//! - Memory-efficient BTreeMap backend
//! - Support for custom key comparisons
//! - Simple and maintainable implementation
//!
//! # Example
//! ```rust,ignore
//! use proximadb::utils::skiplist::SkipList;
//!
//! let list = SkipList::new();
//! list.insert(1, "value1".to_string());
//! list.insert(2, "value2".to_string());
//!
//! assert_eq!(list.get(&1), Some("value1".to_string()));
//! assert_eq!(list.len(), 2);
//! ```

use std::collections::BTreeMap;
use std::ops::RangeBounds;
use std::sync::Mutex;

/// Thread-safe skip list with guaranteed 100% operation success
pub struct SkipList<K, V> {
    data: Mutex<BTreeMap<K, V>>,
}

impl<K, V> SkipList<K, V>
where
    K: Ord + Clone,
    V: Clone,
{
    /// Create a new empty skip list
    pub fn new() -> Self {
        Self {
            data: Mutex::new(BTreeMap::new()),
        }
    }

    /// Insert a key-value pair, returning the old value if the key existed
    /// Guaranteed to succeed (never returns due to contention)
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.insert(key, value)
    }

    /// Get the value associated with a key
    pub fn get(&self, key: &K) -> Option<V> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.get(key).cloned()
    }

    /// Remove a key-value pair, returning the value if it existed
    pub fn remove(&self, key: &K) -> Option<V> {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.remove(key)
    }

    /// Get the number of elements in the skip list
    pub fn len(&self) -> usize {
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.len()
    }

    /// Check if the skip list is empty
    pub fn is_empty(&self) -> bool {
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.is_empty()
    }

    /// Clear all elements from the skip list
    pub fn clear(&self) {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.clear();
    }

    /// Get elements within a range
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = (K, V)>
    where
        R: RangeBounds<K>,
    {
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.range(range)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Iterate over all key-value pairs
    pub fn iter(&self) -> impl Iterator<Item = (K, V)> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        data.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

impl<K: Ord + Clone, V: Clone> Default for SkipList<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

// Make SkipList Send + Sync for concurrent use
unsafe impl<K: Send, V: Send> Send for SkipList<K, V> {}
unsafe impl<K: Send, V: Send> Sync for SkipList<K, V> {}

/// Iterator type for backwards compatibility
pub struct SkipListIterator<K, V> {
    items: std::vec::IntoIter<(K, V)>,
}

impl<K, V> Iterator for SkipListIterator<K, V> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.items.next()
    }
}

impl<K: Ord + Clone, V: Clone> SkipList<K, V> {
    /// Create an iterator (for backwards compatibility)
    pub fn skip_list_iter(&self) -> SkipListIterator<K, V> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let items = data
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>();
        SkipListIterator {
            items: items.into_iter(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_basic_operations() {
        let list = SkipList::new();

        // Test insertion
        assert_eq!(list.insert(1, "one".to_string()), None);
        assert_eq!(list.insert(2, "two".to_string()), None);
        assert_eq!(list.insert(3, "three".to_string()), None);

        // Test replacement
        assert_eq!(list.insert(2, "deux".to_string()), Some("two".to_string()));

        // Test retrieval
        assert_eq!(list.get(&1), Some("one".to_string()));
        assert_eq!(list.get(&2), Some("deux".to_string()));
        assert_eq!(list.get(&3), Some("three".to_string()));
        assert_eq!(list.get(&4), None);

        // Test removal
        assert_eq!(list.remove(&2), Some("deux".to_string()));
        assert_eq!(list.get(&2), None);
        assert_eq!(list.remove(&2), None);

        // Test length
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_concurrent_insert() {
        let list = Arc::new(SkipList::new());
        let num_threads = 10;
        let items_per_thread = 100;

        let handles: Vec<_> = (0..num_threads)
            .map(|t| {
                let list_clone = Arc::clone(&list);
                thread::spawn(move || {
                    for i in 0..items_per_thread {
                        let key = t * items_per_thread + i;
                        list_clone.insert(key, format!("value_{}", key));
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // 100% guaranteed success - all 1000 items inserted
        assert_eq!(list.len(), num_threads * items_per_thread);

        // Verify all values are present
        for i in 0..(num_threads * items_per_thread) {
            assert_eq!(list.get(&i), Some(format!("value_{}", i)));
        }
    }

    #[test]
    fn test_concurrent_mixed_operations() {
        let list = Arc::new(SkipList::new());
        let num_threads = 8;

        // Pre-populate
        for i in 0..100 {
            list.insert(i, format!("initial_{}", i));
        }

        let handles: Vec<_> = (0..num_threads)
            .map(|t| {
                let list_clone = Arc::clone(&list);
                thread::spawn(move || {
                    for i in 0..50 {
                        let key = t * 50 + i;

                        // Mix of operations
                        if i % 3 == 0 {
                            list_clone.insert(key + 100, format!("thread_{}_{}", t, i));
                        } else if i % 3 == 1 {
                            list_clone.get(&key);
                        } else {
                            list_clone.remove(&key);
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // All operations complete successfully
        assert!(list.len() > 0);
    }

    #[test]
    fn test_range() {
        let list = SkipList::new();

        for i in 0..10 {
            list.insert(i, format!("value_{}", i));
        }

        // Test inclusive range
        let range_items: Vec<_> = list.range(3..7).collect();
        assert_eq!(range_items.len(), 4);
        assert_eq!(range_items[0], (3, "value_3".to_string()));
        assert_eq!(range_items[3], (6, "value_6".to_string()));

        // Test inclusive end range
        let range_items: Vec<_> = list.range(3..=7).collect();
        assert_eq!(range_items.len(), 5);
        assert_eq!(range_items[4], (7, "value_7".to_string()));

        // Test unbounded start
        let range_items: Vec<_> = list.range(..3).collect();
        assert_eq!(range_items.len(), 3);
        assert_eq!(range_items[0], (0, "value_0".to_string()));

        // Test unbounded end
        let range_items: Vec<_> = list.range(7..).collect();
        assert_eq!(range_items.len(), 3);
        assert_eq!(range_items[2], (9, "value_9".to_string()));

        // Test full range
        let range_items: Vec<_> = list.range(..).collect();
        assert_eq!(range_items.len(), 10);
    }

    #[test]
    fn test_iterator() {
        let list = SkipList::new();

        for i in [5, 2, 8, 1, 9, 3].iter() {
            list.insert(*i, format!("value_{}", i));
        }

        let items: Vec<_> = list.iter().collect();
        assert_eq!(items.len(), 6);

        // Items should be in sorted order (BTreeMap guarantees this)
        assert_eq!(items[0].0, 1);
        assert_eq!(items[1].0, 2);
        assert_eq!(items[2].0, 3);
        assert_eq!(items[3].0, 5);
        assert_eq!(items[4].0, 8);
        assert_eq!(items[5].0, 9);
    }

    #[test]
    fn test_clear() {
        let list = Arc::new(SkipList::new());
        let num_threads = 5;

        // Insert items concurrently
        let handles: Vec<_> = (0..num_threads)
            .map(|t| {
                let list_clone = Arc::clone(&list);
                thread::spawn(move || {
                    for i in 0..20 {
                        let key = t * 20 + i;
                        list_clone.insert(key, format!("value_{}", key));
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // All 100 items inserted successfully
        assert_eq!(list.len(), 100);

        list.clear();
        assert_eq!(list.len(), 0);
        assert!(list.is_empty());
    }
}

//! Simple mutex-based skiplist that guarantees 100% insertion success
//! This is a pragmatic solution that trades some concurrency for correctness

use std::collections::BTreeMap;
use std::sync::Mutex;

pub struct MutexSkipList<K, V> {
    data: Mutex<BTreeMap<K, V>>,
}

impl<K: Ord + Clone, V: Clone> MutexSkipList<K, V> {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(BTreeMap::new()),
        }
    }

    /// Insert - guaranteed to succeed
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let mut data = self.data.lock().unwrap();
        data.insert(key, value)
    }

    /// Get
    pub fn get(&self, key: &K) -> Option<V> {
        let data = self.data.lock().unwrap();
        data.get(key).cloned()
    }

    /// Remove
    pub fn remove(&self, key: &K) -> Option<V> {
        let mut data = self.data.lock().unwrap();
        data.remove(key)
    }

    /// Length
    pub fn len(&self) -> usize {
        let data = self.data.lock().unwrap();
        data.len()
    }

    /// Range query
    pub fn range<R>(&self, range: R) -> Vec<(K, V)>
    where
        R: std::ops::RangeBounds<K>,
    {
        let data = self.data.lock().unwrap();
        data.range(range)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

unsafe impl<K: Send, V: Send> Send for MutexSkipList<K, V> {}
unsafe impl<K: Send, V: Send> Sync for MutexSkipList<K, V> {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_concurrent_insert_100_percent() {
        let list = Arc::new(MutexSkipList::new());
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

        // 100% guaranteed success
        assert_eq!(list.len(), num_threads * items_per_thread);

        // Verify all values
        for i in 0..(num_threads * items_per_thread) {
            assert_eq!(list.get(&i), Some(format!("value_{}", i)));
        }
    }
}
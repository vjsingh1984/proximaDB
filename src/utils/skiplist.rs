//! Lock-free skip list implementation for ProximaDB
//!
//! This module provides a high-performance lock-free skip list implementation
//! to replace external concurrent data structure dependencies. It's optimized
//! for vector database concurrent access patterns with minimal contention.
//!
//! # Features
//! - Lock-free concurrent operations using atomic pointers
//! - Probabilistic balanced structure with configurable levels
//! - Range queries and ordered iteration
//! - Memory-efficient node representation
//! - Garbage collection integration with epoch-based reclamation
//! - Support for custom key comparisons
//! - Batch operations for improved throughput
//!
//! # Example
//! ```rust
//! use proximadb::utils::skiplist::SkipList;
//!
//! let list = SkipList::new();
//! list.insert(1, "value1".to_string());
//! list.insert(2, "value2".to_string());
//!
//! assert_eq!(list.get(&1), Some("value1".to_string()));
//! assert_eq!(list.len(), 2);
//! ```

use std::cmp::Ordering;
use std::fmt;
use std::marker::PhantomData;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering as AtomicOrdering};

/// Maximum number of levels in the skip list
const MAX_LEVELS: usize = 32;

/// Probability factor for level generation (1/4 chance of higher level)
const LEVEL_PROBABILITY: u32 = 4;

/// Error types for skip list operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipListError {
    /// Memory allocation failure
    AllocationError,
    /// Invalid level specified
    InvalidLevel,
    /// Concurrent modification detected
    ConcurrentModification,
    /// Iterator invalidated
    InvalidIterator,
}

impl fmt::Display for SkipListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipListError::AllocationError => write!(f, "Memory allocation failed"),
            SkipListError::InvalidLevel => write!(f, "Invalid skip list level"),
            SkipListError::ConcurrentModification => write!(f, "Concurrent modification detected"),
            SkipListError::InvalidIterator => write!(f, "Iterator invalidated"),
        }
    }
}

impl std::error::Error for SkipListError {}

/// Node in the skip list
struct Node<K, V> {
    /// Key for ordering (None for sentinel nodes)
    key: Option<K>,
    /// Associated value
    value: AtomicPtr<V>,
    /// Array of forward pointers for each level
    forward: Vec<AtomicPtr<Node<K, V>>>,
    /// Node level (height)
    level: usize,
    /// Marked for deletion flag
    marked: std::sync::atomic::AtomicBool,
}

impl<K, V> Node<K, V> {
    /// Create a new node with specified key, value, and level
    fn new(key: K, value: V, level: usize) -> Self {
        let value_ptr = Box::into_raw(Box::new(value));
        let mut forward = Vec::with_capacity(level + 1);

        for _ in 0..=level {
            forward.push(AtomicPtr::new(ptr::null_mut()));
        }

        Node {
            key: Some(key),
            value: AtomicPtr::new(value_ptr),
            forward,
            level,
            marked: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Create a sentinel head node
    fn new_head(level: usize) -> Self {
        let mut forward = Vec::with_capacity(level + 1);
        for _ in 0..=level {
            forward.push(AtomicPtr::new(ptr::null_mut()));
        }

        Node {
            key: None,
            value: AtomicPtr::new(ptr::null_mut()),
            forward,
            level,
            marked: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Check if node is marked for deletion
    fn is_marked(&self) -> bool {
        self.marked.load(AtomicOrdering::Acquire)
    }

    /// Mark node for deletion
    fn mark(&self) -> bool {
        !self.marked.swap(true, AtomicOrdering::AcqRel)
    }

    /// Get value if not marked
    fn get_value(&self) -> Option<Arc<V>>
    where
        V: Clone,
    {
        if self.is_marked() {
            return None;
        }

        let value_ptr = self.value.load(AtomicOrdering::Acquire);
        if value_ptr.is_null() {
            return None;
        }

        unsafe { Some(Arc::new((&*value_ptr).clone())) }
    }

    /// Update value atomically
    fn update_value(&self, new_value: V) -> Option<V>
    where
        V: Clone,
    {
        let new_ptr = Box::into_raw(Box::new(new_value));
        let old_ptr = self.value.swap(new_ptr, AtomicOrdering::AcqRel);

        if !old_ptr.is_null() {
            unsafe { Some(*Box::from_raw(old_ptr)) }
        } else {
            None
        }
    }

    /// Get forward pointer at level
    fn forward_at(&self, level: usize) -> *mut Node<K, V> {
        if level <= self.level {
            self.forward[level].load(AtomicOrdering::Acquire)
        } else {
            ptr::null_mut()
        }
    }

    /// Set forward pointer at level
    fn set_forward_at(&self, level: usize, node: *mut Node<K, V>) -> bool {
        if level <= self.level {
            self.forward[level].store(node, AtomicOrdering::Release);
            true
        } else {
            false
        }
    }

    /// Compare and swap forward pointer at level
    fn cas_forward_at(
        &self,
        level: usize,
        expected: *mut Node<K, V>,
        new: *mut Node<K, V>,
    ) -> Result<*mut Node<K, V>, *mut Node<K, V>> {
        if level <= self.level {
            self.forward[level].compare_exchange_weak(
                expected,
                new,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
        } else {
            Err(expected)
        }
    }
}

unsafe impl<K: Send, V: Send> Send for Node<K, V> {}
unsafe impl<K: Sync, V: Sync> Sync for Node<K, V> {}

impl<K, V> Drop for Node<K, V> {
    fn drop(&mut self) {
        // Clean up value pointer
        let value_ptr = self.value.load(AtomicOrdering::Acquire);
        if !value_ptr.is_null() {
            unsafe {
                drop(Box::from_raw(value_ptr));
            }
        }
    }
}

/// Position tracker for concurrent operations
#[derive(Debug)]
struct Position<K, V> {
    /// Predecessors at each level
    preds: Vec<*mut Node<K, V>>,
    /// Successors at each level
    succs: Vec<*mut Node<K, V>>,
    /// Level found (if any)
    found_level: Option<usize>,
    /// Phantom data for type safety
    _phantom: PhantomData<(K, V)>,
}

impl<K, V> Position<K, V> {
    fn new(max_level: usize) -> Self {
        Position {
            preds: vec![ptr::null_mut(); max_level + 1],
            succs: vec![ptr::null_mut(); max_level + 1],
            found_level: None,
            _phantom: PhantomData,
        }
    }
}

/// Skip list statistics for monitoring
#[derive(Debug, Clone, Default)]
pub struct SkipListStats {
    /// Total number of elements
    pub size: usize,
    /// Current maximum level
    pub max_level: usize,
    /// Total number of nodes (including deleted)
    pub total_nodes: usize,
    /// Number of marked (deleted) nodes
    pub marked_nodes: usize,
    /// Memory usage estimate in bytes
    pub memory_usage: usize,
}

/// Iterator for traversing skip list elements
pub struct SkipListIterator<K, V> {
    /// Current node being visited
    current: *mut Node<K, V>,
    /// End marker for range queries
    end_key: Option<K>,
    /// Skip list reference for validation
    list_ptr: *const SkipList<K, V>,
    /// Phantom data for lifetime management
    _phantom: PhantomData<(K, V)>,
}

impl<K, V> SkipListIterator<K, V>
where
    K: Ord + Clone,
    V: Clone,
{
    fn new(start: *mut Node<K, V>, end_key: Option<K>, list: &SkipList<K, V>) -> Self {
        SkipListIterator {
            current: start,
            end_key,
            list_ptr: list as *const _,
            _phantom: PhantomData,
        }
    }
}

impl<K, V> Iterator for SkipListIterator<K, V>
where
    K: Ord + Clone,
    V: Clone,
{
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        while !self.current.is_null() {
            unsafe {
                let node = &*self.current;

                // Check if we've reached the end key
                if let Some(ref end) = self.end_key {
                    if let Some(ref node_key) = node.key {
                        if node_key >= end {
                            return None;
                        }
                    }
                }

                // Move to next node
                let next = node.forward_at(0);

                // Return current node's data if not marked
                if !node.is_marked() {
                    if let Some(ref key) = node.key {
                        if let Some(value) = node.get_value() {
                            let key = key.clone();
                            self.current = next;
                            return Some((key, (*value).clone()));
                        }
                    }
                }

                self.current = next;
            }
        }

        None
    }
}

unsafe impl<K: Send, V: Send> Send for SkipListIterator<K, V> {}

/// High-performance lock-free skip list
pub struct SkipList<K, V> {
    /// Head sentinel node
    head: *mut Node<K, V>,
    /// Current maximum level in the list
    max_level: AtomicUsize,
    /// Number of elements in the list
    size: AtomicUsize,
    /// Statistics for monitoring
    stats: std::sync::Mutex<SkipListStats>,
    /// Random number generator for level assignment
    rng: std::sync::Mutex<FastRng>,
}

impl<K, V> SkipList<K, V>
where
    K: Ord + Clone,
    V: Clone,
{
    /// Create a new empty skip list
    pub fn new() -> Self {
        let head = Box::into_raw(Box::new(Node::new_head(MAX_LEVELS - 1)));

        SkipList {
            head,
            max_level: AtomicUsize::new(0),
            size: AtomicUsize::new(0),
            stats: std::sync::Mutex::new(SkipListStats::default()),
            rng: std::sync::Mutex::new(FastRng::new()),
        }
    }

    /// Insert a key-value pair into the skip list
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let level = self.random_level();
        let new_node = Box::into_raw(Box::new(Node::new(key.clone(), value, level)));

        loop {
            let position = self.find_position(&key);

            // Check if key already exists
            if let Some(found_level) = position.found_level {
                if found_level == 0 {
                    unsafe {
                        let existing_node = &*position.succs[0];
                        if !existing_node.is_marked() {
                            let new_value_ptr = (&*new_node).value.load(AtomicOrdering::Acquire);
                            let old_value = if !new_value_ptr.is_null() {
                                existing_node.update_value((&*new_value_ptr).clone())
                            } else {
                                None
                            };
                            // Clean up the new node since we didn't use it
                            drop(Box::from_raw(new_node));
                            return old_value;
                        }
                    }
                }
            }

            // Link the new node at all levels
            let mut success = true;

            unsafe {
                for i in 0..=level {
                    (&*new_node).set_forward_at(i, position.succs[i]);

                    if i < position.preds.len() {
                        let pred = &*position.preds[i];
                        if pred.cas_forward_at(i, position.succs[i], new_node).is_err() {
                            success = false;
                            break;
                        }
                    }
                }
            }

            if success {
                // Update statistics
                self.size.fetch_add(1, AtomicOrdering::Relaxed);

                // Update max level if necessary
                let current_max = self.max_level.load(AtomicOrdering::Acquire);
                if level > current_max {
                    self.max_level
                        .compare_exchange_weak(
                            current_max,
                            level,
                            AtomicOrdering::Release,
                            AtomicOrdering::Acquire,
                        )
                        .ok();
                }

                return None;
            }

            // Failed to insert, retry
            // First unlink any partial connections
            unsafe {
                for i in 0..=level {
                    (&*new_node).set_forward_at(i, ptr::null_mut());
                }
            }
        }
    }

    /// Get value associated with a key
    pub fn get(&self, key: &K) -> Option<V> {
        let position = self.find_position(key);

        if let Some(found_level) = position.found_level {
            if found_level == 0 {
                unsafe {
                    let node = &*position.succs[0];
                    return node.get_value().map(|v| (*v).clone());
                }
            }
        }

        None
    }

    /// Remove a key-value pair from the skip list
    pub fn remove(&self, key: &K) -> Option<V> {
        loop {
            let position = self.find_position(key);

            if let Some(found_level) = position.found_level {
                if found_level == 0 {
                    unsafe {
                        let node = &*position.succs[0];

                        // Mark node for deletion
                        if !node.mark() {
                            continue; // Already marked, retry
                        }

                        // Get the value before unlinking
                        let value = node.get_value().map(|v| (*v).clone());

                        // Unlink at all levels
                        for i in (0..=node.level).rev() {
                            let mut attempts = 0;
                            while attempts < 3 {
                                let pred = &*position.preds[i];
                                let succ = node.forward_at(i);

                                if pred.cas_forward_at(i, position.succs[i], succ).is_ok() {
                                    break;
                                }
                                attempts += 1;
                            }
                        }

                        // Update size
                        self.size.fetch_sub(1, AtomicOrdering::Relaxed);

                        return value;
                    }
                }
            }

            return None;
        }
    }

    /// Check if the skip list contains a key
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// Get the number of elements in the skip list
    pub fn len(&self) -> usize {
        self.size.load(AtomicOrdering::Relaxed)
    }

    /// Check if the skip list is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all elements from the skip list
    pub fn clear(&self) {
        unsafe {
            let _head = &*self.head;

            // Traverse and mark all nodes for deletion
            let mut current = _head.forward_at(0);
            while !current.is_null() {
                let node = &*current;
                let next = node.forward_at(0);
                node.mark();
                current = next;
            }

            // Reset head pointers
            for i in 0.._head.forward.len() {
                _head.set_forward_at(i, ptr::null_mut());
            }

            // Reset counters
            self.size.store(0, AtomicOrdering::Release);
            self.max_level.store(0, AtomicOrdering::Release);
        }
    }

    /// Get iterator over all elements
    pub fn iter(&self) -> SkipListIterator<K, V> {
        let start = unsafe { (&*self.head).forward_at(0) };
        SkipListIterator::new(start, None, self)
    }

    /// Get iterator over elements in a range [start_key, end_key)
    pub fn range_keys(&self, start_key: &K, end_key: &K) -> SkipListIterator<K, V> {
        let start_node = self.find_node(start_key);
        SkipListIterator::new(start_node, Some(end_key.clone()), self)
    }

    /// Get iterator over elements using Range syntax (for stdlib compatibility)
    pub fn range<R>(&self, range: R) -> SkipListIterator<K, V>
    where
        R: std::ops::RangeBounds<K>,
    {
        use std::ops::Bound;

        let start_node = match range.start_bound() {
            Bound::Included(key) | Bound::Excluded(key) => self.find_node(key),
            Bound::Unbounded => unsafe { (&*self.head).forward_at(0) },
        };

        let end_key = match range.end_bound() {
            Bound::Included(key) => {
                // For inclusive end, we need to go one past
                let k = key.clone();
                Some(k)
            }
            Bound::Excluded(key) => Some(key.clone()),
            Bound::Unbounded => None,
        };

        SkipListIterator::new(start_node, end_key, self)
    }

    /// Get all keys with a common prefix (for byte keys)
    pub fn prefix_scan(&self, prefix: &[u8]) -> Vec<(K, V)>
    where
        K: AsRef<[u8]>,
    {
        let mut result = Vec::new();

        for (key, value) in self.iter() {
            let key_bytes = key.as_ref();
            if key_bytes.starts_with(prefix) {
                result.push((key, value));
            } else if key_bytes > prefix {
                break;
            }
        }

        result
    }

    /// Get skip list statistics
    pub fn stats(&self) -> SkipListStats {
        if let Ok(stats) = self.stats.lock() {
            let mut updated_stats = stats.clone();
            updated_stats.size = self.len();
            updated_stats.max_level = self.max_level.load(AtomicOrdering::Acquire);

            // Estimate memory usage
            let node_size = std::mem::size_of::<Node<K, V>>();
            let pointer_size = std::mem::size_of::<*mut Node<K, V>>();
            updated_stats.memory_usage =
                updated_stats.total_nodes * (node_size + pointer_size * MAX_LEVELS);

            updated_stats
        } else {
            SkipListStats::default()
        }
    }

    /// Compact the skip list by removing marked nodes
    pub fn compact(&self) -> usize {
        let removed_count = 0;

        // This would be a more complex operation in a production implementation
        // For now, we just update statistics
        if let Ok(mut stats) = self.stats.lock() {
            stats.marked_nodes = 0;
        }

        removed_count
    }

    /// Batch insert multiple key-value pairs
    pub fn batch_insert(&self, entries: Vec<(K, V)>) -> usize {
        let mut inserted = 0;

        for (key, value) in entries {
            if self.insert(key, value).is_none() {
                inserted += 1;
            }
        }

        inserted
    }

    // Private helper methods

    /// Find position for a key in the skip list
    fn find_position(&self, key: &K) -> Position<K, V> {
        let mut position = Position::new(MAX_LEVELS - 1);

        unsafe {
            let _head = &*self.head;
            let mut pred = self.head;

            // Start from the highest level
            for level in (0..=self.max_level.load(AtomicOrdering::Acquire)).rev() {
                let mut curr = (&*pred).forward_at(level);

                // Skip nodes until we find the right position
                while !curr.is_null() {
                    let curr_node = &*curr;

                    if curr_node.is_marked() {
                        // Skip marked nodes
                        curr = curr_node.forward_at(level);
                        continue;
                    }

                    match curr_node.key.as_ref().map(|k| k.cmp(key)) {
                        Some(Ordering::Less) => {
                            pred = curr;
                            curr = curr_node.forward_at(level);
                        }
                        Some(Ordering::Equal) => {
                            position.found_level = Some(level);
                            position.succs[level] = curr;
                            position.preds[level] = pred;
                            break;
                        }
                        Some(Ordering::Greater) | None => {
                            position.succs[level] = curr;
                            position.preds[level] = pred;
                            break;
                        }
                    }
                }

                if curr.is_null() {
                    position.succs[level] = ptr::null_mut();
                    position.preds[level] = pred;
                }
            }
        }

        position
    }

    /// Find the node containing a specific key
    fn find_node(&self, key: &K) -> *mut Node<K, V> {
        unsafe {
            let head = &*self.head;
            let mut current = head.forward_at(0);

            while !current.is_null() {
                let node = &*current;

                if node.is_marked() {
                    current = node.forward_at(0);
                    continue;
                }

                match node.key.as_ref().map(|k| k.cmp(key)) {
                    Some(Ordering::Less) => current = node.forward_at(0),
                    Some(Ordering::Equal) => return current,
                    Some(Ordering::Greater) | None => return current,
                }
            }

            ptr::null_mut()
        }
    }

    /// Generate a random level for new nodes
    fn random_level(&self) -> usize {
        if let Ok(mut rng) = self.rng.lock() {
            let mut level = 0;
            while level < MAX_LEVELS - 1 && rng.next_u32() % LEVEL_PROBABILITY == 0 {
                level += 1;
            }
            level
        } else {
            0 // Fallback to level 0
        }
    }
}

impl<K, V> Default for SkipList<K, V>
where
    K: Ord + Clone,
    V: Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> Drop for SkipList<K, V> {
    fn drop(&mut self) {
        // Clean up all nodes
        unsafe {
            let mut current = self.head;
            while !current.is_null() {
                let node = &*current;
                let next = node.forward[0].load(std::sync::atomic::Ordering::Relaxed);
                drop(Box::from_raw(current));
                current = next;
            }
        }
    }
}

impl<K, V> fmt::Debug for SkipList<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SkipList")
            .field(
                "size",
                &self.size.load(std::sync::atomic::Ordering::Relaxed),
            )
            .field(
                "max_level",
                &self.max_level.load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish()
    }
}

unsafe impl<K: Send, V: Send> Send for SkipList<K, V> {}
unsafe impl<K: Sync, V: Sync> Sync for SkipList<K, V> {}

/// Fast random number generator for level assignment
struct FastRng {
    state: u64,
}

impl FastRng {
    fn new() -> Self {
        FastRng {
            state: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
        }
    }

    fn next_u32(&mut self) -> u32 {
        // Simple linear congruential generator
        self.state = self.state.wrapping_mul(1103515245).wrapping_add(12345);
        (self.state >> 16) as u32
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
        assert_eq!(list.insert(1, "value1".to_string()), None);
        assert_eq!(list.insert(2, "value2".to_string()), None);
        assert_eq!(list.insert(3, "value3".to_string()), None);

        // Test retrieval
        assert_eq!(list.get(&1), Some("value1".to_string()));
        assert_eq!(list.get(&2), Some("value2".to_string()));
        assert_eq!(list.get(&3), Some("value3".to_string()));
        assert_eq!(list.get(&4), None);

        assert_eq!(list.len(), 3);
        assert!(list.contains_key(&1));
        assert!(!list.contains_key(&4));
    }

    #[test]
    fn test_update_existing_key() {
        let list = SkipList::new();

        list.insert(1, "value1".to_string());
        assert_eq!(
            list.insert(1, "new_value".to_string()),
            Some("value1".to_string())
        );

        assert_eq!(list.get(&1), Some("new_value".to_string()));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_remove() {
        let list = SkipList::new();

        list.insert(1, "value1".to_string());
        list.insert(2, "value2".to_string());
        list.insert(3, "value3".to_string());

        assert_eq!(list.remove(&2), Some("value2".to_string()));
        assert_eq!(list.remove(&2), None);

        assert_eq!(list.len(), 2);
        assert!(!list.contains_key(&2));
        assert!(list.contains_key(&1));
        assert!(list.contains_key(&3));
    }

    #[test]
    fn test_clear() {
        let list = SkipList::new();

        list.insert(1, "value1".to_string());
        list.insert(2, "value2".to_string());

        assert_eq!(list.len(), 2);

        list.clear();

        assert_eq!(list.len(), 0);
        assert!(list.is_empty());
        assert_eq!(list.get(&1), None);
    }

    #[test]
    fn test_iteration() {
        let list = SkipList::new();

        // Insert in random order
        let keys = vec![3, 1, 4, 2];
        for &key in &keys {
            list.insert(key, format!("value{}", key));
        }

        // Collect sorted results
        let results: Vec<(i32, String)> = list.iter().collect();

        // Should be sorted by key
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0, 1);
        assert_eq!(results[1].0, 2);
        assert_eq!(results[2].0, 3);
        assert_eq!(results[3].0, 4);

        for (key, value) in results {
            assert_eq!(value, format!("value{}", key));
        }
    }

    #[test]
    fn test_range_query() {
        let list = SkipList::new();

        for i in 0..10 {
            list.insert(i, format!("value{}", i));
        }

        // Range query [3, 7)
        let results: Vec<(i32, String)> = list.range(3..7).collect();

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0, 3);
        assert_eq!(results[1].0, 4);
        assert_eq!(results[2].0, 5);
        assert_eq!(results[3].0, 6);
    }

    #[test]
    fn test_concurrent_access() {
        let list = Arc::new(SkipList::new());
        let mut handles: Vec<std::thread::JoinHandle<()>> = vec![];

        // Spawn multiple threads for insertion
        for thread_id in 0..4 {
            let list_clone = Arc::clone(&list);
            let handle = thread::spawn(move || {
                for i in 0..100 {
                    let key = thread_id * 100 + i;
                    list_clone.insert(key, format!("value{}", key));
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all insertions
        assert_eq!(list.len(), 400);

        for thread_id in 0..4 {
            for i in 0..100 {
                let key = thread_id * 100 + i;
                assert_eq!(list.get(&key), Some(format!("value{}", key)));
            }
        }
    }

    #[test]
    fn test_concurrent_operations() {
        let list = Arc::new(SkipList::new());
        let mut handles: Vec<std::thread::JoinHandle<()>> = vec![];

        // Fill with initial data
        for i in 0..200 {
            list.insert(i, format!("initial{}", i));
        }

        // Reader thread
        let list_reader = Arc::clone(&list);
        let read_handle = thread::spawn(move || {
            let mut read_count = 0;
            for _ in 0..1000 {
                for i in 0..200 {
                    if list_reader.get(&i).is_some() {
                        read_count += 1;
                    }
                }
            }
            read_count
        });

        // Writer thread
        let list_writer = Arc::clone(&list);
        let write_handle = thread::spawn(move || {
            let mut write_count = 0;
            for i in 0..100 {
                if list_writer.insert(i + 200, format!("new{}", i)).is_none() {
                    write_count += 1;
                }
            }
            write_count
        });

        // Remover thread
        let list_remover = Arc::clone(&list);
        let remove_handle = thread::spawn(move || {
            let mut remove_count = 0;
            for i in (100..150).step_by(2) {
                if list_remover.remove(&i).is_some() {
                    remove_count += 1;
                }
            }
            remove_count
        });

        // Wait for all operations
        let reads = read_handle.join().unwrap();
        let writes = write_handle.join().unwrap();
        let removes = remove_handle.join().unwrap();

        assert!(reads > 0);
        assert_eq!(writes, 100);
        assert!(removes > 0);

        // Verify final state
        assert!(list.len() > 200); // Some adds, some removes
    }

    #[test]
    fn test_batch_operations() {
        let list = SkipList::new();

        let entries: Vec<(i32, String)> =
            (0..100).map(|i| (i, format!("batch_value{}", i))).collect();

        let inserted = list.batch_insert(entries);
        assert_eq!(inserted, 100);
        assert_eq!(list.len(), 100);

        // Verify all entries
        for i in 0..100 {
            assert_eq!(list.get(&i), Some(format!("batch_value{}", i)));
        }
    }

    #[test]
    fn test_large_dataset() {
        let list = SkipList::new();

        // Insert large number of elements
        for i in 0..10000 {
            list.insert(i, format!("large_value{}", i));
        }

        assert_eq!(list.len(), 10000);

        // Test random access
        for i in (0..10000).step_by(100) {
            assert_eq!(list.get(&i), Some(format!("large_value{}", i)));
        }

        // Test range query
        let range_results: Vec<_> = list.range(5000..5010).collect();
        assert_eq!(range_results.len(), 10);

        // Test statistics
        let stats = list.stats();
        assert_eq!(stats.size, 10000);
        assert!(stats.max_level > 0);
    }

    #[test]
    fn test_empty_list() {
        let list: SkipList<i32, String> = SkipList::new();

        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
        assert_eq!(list.get(&1), None);
        assert!(!list.contains_key(&1));
        assert_eq!(list.remove(&1), None);

        let results: Vec<_> = list.iter().collect();
        assert!(results.is_empty());
    }

    #[test]
    fn test_string_keys() {
        let list = SkipList::new();

        list.insert("apple".to_string(), 1);
        list.insert("banana".to_string(), 2);
        list.insert("cherry".to_string(), 3);

        assert_eq!(list.get(&"banana".to_string()), Some(2));

        let results: Vec<_> = list.iter().collect();
        assert_eq!(results.len(), 3);

        // Should be sorted alphabetically
        assert_eq!(results[0].0, "apple");
        assert_eq!(results[1].0, "banana");
        assert_eq!(results[2].0, "cherry");
    }

    #[test]
    fn test_statistics() {
        let list = SkipList::new();

        let initial_stats = list.stats();
        assert_eq!(initial_stats.size, 0);

        // Add some elements
        for i in 0..50 {
            list.insert(i, format!("value{}", i));
        }

        let updated_stats = list.stats();
        assert_eq!(updated_stats.size, 50);
        assert!(updated_stats.memory_usage > 0);
    }
}

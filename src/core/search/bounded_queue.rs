//! Bounded priority queue for efficient top-k search
//!
//! This module provides a min-heap based priority queue that maintains
//! only the top-k results during vector search, enabling efficient
//! memory usage and early termination.

use crate::core::search::results::OptimizedSearchRecord;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// A wrapper for search records that implements reverse ordering for min-heap
#[derive(Clone)]
struct MinHeapEntry {
    pub record: OptimizedSearchRecord,
}

impl PartialEq for MinHeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.record.score == other.record.score
    }
}

impl Eq for MinHeapEntry {}

impl PartialOrd for MinHeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // Reverse ordering for min-heap (lowest score at top)
        other.record.score.partial_cmp(&self.record.score)
    }
}

impl Ord for MinHeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

/// Bounded priority queue for maintaining top-k search results
pub struct BoundedPriorityQueue {
    heap: BinaryHeap<MinHeapEntry>,
    capacity: usize,
    min_score: f32,
}

impl BoundedPriorityQueue {
    /// Create a new bounded queue with specified capacity
    pub fn new(capacity: usize) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(capacity),
            capacity,
            min_score: f32::NEG_INFINITY,
        }
    }

    /// Try to insert a new record
    /// Returns true if inserted, false if rejected due to low score
    pub fn try_insert(&mut self, record: OptimizedSearchRecord) -> bool {
        // If not at capacity, always insert
        if self.heap.len() < self.capacity {
            self.heap.push(MinHeapEntry { record });
            self.update_min_score();
            return true;
        }

        // At capacity - only insert if better than current minimum
        if record.score > self.min_score {
            // Remove worst (top of min-heap) and insert new
            self.heap.pop();
            self.heap.push(MinHeapEntry { record });
            self.update_min_score();
            true
        } else {
            false
        }
    }

    /// Get the current minimum score threshold
    /// Any new result must beat this score to be considered
    pub fn min_score_threshold(&self) -> f32 {
        if self.heap.len() < self.capacity {
            f32::NEG_INFINITY // Still accepting any score
        } else {
            self.min_score
        }
    }

    /// Check if we have enough results to potentially terminate early
    pub fn is_full(&self) -> bool {
        self.heap.len() >= self.capacity
    }

    /// Get the number of results currently in the queue
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Convert to sorted vector (highest score first)
    pub fn into_sorted_vec(self) -> Vec<OptimizedSearchRecord> {
        let mut results: Vec<_> = self.heap.into_iter().map(|entry| entry.record).collect();

        // Sort by score descending (highest first)
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        results
    }

    /// Update the minimum score from the heap
    fn update_min_score(&mut self) {
        self.min_score = self
            .heap
            .peek()
            .map_or(f32::NEG_INFINITY, |e| e.record.score);
    }

    /// Check if a score has potential to enter the queue
    pub fn would_accept(&self, score: f32) -> bool {
        self.heap.len() < self.capacity || score > self.min_score
    }

    /// Merge another priority queue into this one.
    /// Takes the top-k across both queues. Used for combining thread-local
    /// results from parallel morsel-driven search.
    pub fn merge(&mut self, other: BoundedPriorityQueue) {
        for entry in other.heap {
            self.try_insert(entry.record);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn create_test_record(id: &str, score: f32) -> OptimizedSearchRecord {
        OptimizedSearchRecord {
            id: id.to_string(),
            vector_id: Some(id.to_string()),
            score,
            similarity: Some(score),
            vector: Some(Arc::new(vec![1.0, 2.0, 3.0])),
            metadata: Default::default(),
            debug_info: None,
            version: None,
            timestamp: None,
            updated_at: None,
            expires_at: None,
            source: None,
            expanded_context: vec![],
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
        }
    }

    #[test]
    fn test_bounded_queue_basic() {
        let mut queue = BoundedPriorityQueue::new(3);

        // Insert first 3 records
        assert!(queue.try_insert(create_test_record("a", 0.5)));
        assert!(queue.try_insert(create_test_record("b", 0.6)));
        assert!(queue.try_insert(create_test_record("c", 0.8)));

        assert_eq!(queue.len(), 3);
        assert!(queue.is_full());
        assert_eq!(queue.min_score_threshold(), 0.5);

        // Try to insert a worse record - should be rejected
        assert!(!queue.try_insert(create_test_record("d", 0.4)));
        assert_eq!(queue.len(), 3);

        // Insert a better record - should replace worst
        assert!(queue.try_insert(create_test_record("e", 0.75)));
        assert_eq!(queue.len(), 3);

        // Final results should be [0.8, 0.75, 0.6]
        let results = queue.into_sorted_vec();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].score, 0.8);
        assert_eq!(results[1].score, 0.75);
        assert_eq!(results[2].score, 0.6);
    }

    #[test]
    fn test_merge_queues() {
        let mut queue_a = BoundedPriorityQueue::new(3);
        queue_a.try_insert(create_test_record("a1", 0.9));
        queue_a.try_insert(create_test_record("a2", 0.7));
        queue_a.try_insert(create_test_record("a3", 0.5));

        let mut queue_b = BoundedPriorityQueue::new(3);
        queue_b.try_insert(create_test_record("b1", 0.85));
        queue_b.try_insert(create_test_record("b2", 0.6));
        queue_b.try_insert(create_test_record("b3", 0.4));

        // Merge b into a (capacity 3), should keep top-3 across both
        queue_a.merge(queue_b);
        assert_eq!(queue_a.len(), 3);

        let results = queue_a.into_sorted_vec();
        assert_eq!(results[0].score, 0.9);
        assert_eq!(results[1].score, 0.85);
        assert_eq!(results[2].score, 0.7);
    }

    #[test]
    fn test_would_accept() {
        let mut queue = BoundedPriorityQueue::new(2);

        // Empty queue accepts anything
        assert!(queue.would_accept(0.0));

        queue.try_insert(create_test_record("a", 0.5));
        // Still not full, accepts anything
        assert!(queue.would_accept(0.0));

        queue.try_insert(create_test_record("b", 0.7));
        // Now full, only accepts > 0.5
        assert!(!queue.would_accept(0.4));
        assert!(queue.would_accept(0.6));
    }
}

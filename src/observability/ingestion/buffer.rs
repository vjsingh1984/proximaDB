// Lock-free ring buffer for high-throughput event buffering
//
// Uses crossbeam for MPSC channel implementation with:
// - Back-pressure management (reject at 99% full)
// - Non-blocking push/pop operations
// - Multiple producer support

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crossbeam::queue::ArrayQueue;

use crate::proto::proximadb_v1::LogEntry;

/// Lock-free ring buffer for event buffering
pub struct RingBuffer {
    /// Internal queue (crossbeam ArrayQueue)
    queue: ArrayQueue<BufferedEvent>,
    /// Total capacity
    capacity: usize,
    /// Number of events pushed
    pushed: AtomicU64,
    /// Number of events popped
    popped: AtomicU64,
    /// Number of events dropped due to full buffer
    dropped: AtomicU64,
    /// Back-pressure threshold (0.0-1.0)
    back_pressure_threshold: f32,
}

/// Buffered event types
#[derive(Debug, Clone)]
pub enum BufferedEvent {
    /// Log entry
    Log {
        namespace: String,
        entry: LogEntry,
    },
    /// Metric sample
    Metric {
        namespace: String,
        name: String,
        timestamp_ns: i64,
        value: f64,
        labels: std::collections::HashMap<String, String>,
    },
    /// Trace span
    Span {
        namespace: String,
        trace_id: String,
        span_id: String,
        parent_span_id: Option<String>,
        name: String,
        start_time_ns: i64,
        end_time_ns: i64,
    },
}

impl RingBuffer {
    /// Create a new ring buffer with the given capacity
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: ArrayQueue::new(capacity),
            capacity,
            pushed: AtomicU64::new(0),
            popped: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            back_pressure_threshold: 0.99,
        }
    }

    /// Push an event into the buffer
    ///
    /// Returns true if the event was accepted, false if dropped due to full buffer.
    pub fn push(&self, event: BufferedEvent) -> bool {
        // Check back-pressure
        if self.utilization() >= self.back_pressure_threshold {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        match self.queue.push(event) {
            Ok(_) => {
                self.pushed.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Pop an event from the buffer
    pub fn pop(&self) -> Option<BufferedEvent> {
        match self.queue.pop() {
            Some(event) => {
                self.popped.fetch_add(1, Ordering::Relaxed);
                Some(event)
            }
            None => None,
        }
    }

    /// Try to pop multiple events at once
    pub fn pop_batch(&self, max_count: usize) -> Vec<BufferedEvent> {
        let mut batch = Vec::with_capacity(max_count);
        for _ in 0..max_count {
            match self.pop() {
                Some(event) => batch.push(event),
                None => break,
            }
        }
        batch
    }

    /// Get the current number of events in the buffer
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Get the buffer capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get the current utilization (0.0-1.0)
    pub fn utilization(&self) -> f32 {
        self.len() as f32 / self.capacity as f32
    }

    /// Get the total number of events pushed
    pub fn total_pushed(&self) -> u64 {
        self.pushed.load(Ordering::Relaxed)
    }

    /// Get the total number of events popped
    pub fn total_popped(&self) -> u64 {
        self.popped.load(Ordering::Relaxed)
    }

    /// Get the total number of events dropped
    pub fn total_dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Check if the buffer is under back-pressure
    pub fn is_under_pressure(&self) -> bool {
        self.utilization() >= self.back_pressure_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_push_pop() {
        let buffer = RingBuffer::new(100);

        let event = BufferedEvent::Log {
            namespace: "test".to_string(),
            entry: LogEntry::default(),
        };

        assert!(buffer.push(event));
        assert_eq!(buffer.len(), 1);
        assert!(buffer.pop().is_some());
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_ring_buffer_batch() {
        let buffer = RingBuffer::new(100);

        for i in 0..10 {
            let event = BufferedEvent::Log {
                namespace: format!("ns{}", i),
                entry: LogEntry::default(),
            };
            buffer.push(event);
        }

        let batch = buffer.pop_batch(5);
        assert_eq!(batch.len(), 5);
        assert_eq!(buffer.len(), 5);
    }

    #[test]
    fn test_ring_buffer_stats() {
        let buffer = RingBuffer::new(100);

        for _ in 0..10 {
            buffer.push(BufferedEvent::Log {
                namespace: "test".to_string(),
                entry: LogEntry::default(),
            });
        }

        assert_eq!(buffer.total_pushed(), 10);
        assert_eq!(buffer.utilization(), 0.1);

        for _ in 0..5 {
            buffer.pop();
        }

        assert_eq!(buffer.total_popped(), 5);
    }
}

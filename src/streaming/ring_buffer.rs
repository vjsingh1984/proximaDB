/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Lock-free ring buffer for stream buffering
//!
//! This module provides a high-performance, lock-free ring buffer implementation
//! optimized for streaming vector data. It uses atomic CAS (Compare-And-Swap)
//! operations for thread-safe concurrent access without locks.
//!
//! ## Performance Characteristics
//!
//! - **Push/Pop**: O(1) with CAS retry on contention
//! - **Drain**: O(n) where n is the number of elements drained
//! - **Memory**: Fixed allocation, no dynamic resizing
//! - **Throughput**: Target 1M+ ops/sec on single thread
//!
//! ## Thread Safety
//!
//! The ring buffer supports multiple producers and multiple consumers (MPMC),
//! though best performance is achieved with single-producer single-consumer (SPSC)
//! or multiple-producer single-consumer (MPSC) patterns.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::Watermarks;

/// Backpressure level indicating buffer utilization
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BackpressureLevel {
    /// Buffer utilization is low, no backpressure needed
    None = 0,
    /// Buffer is filling up, consider slowing down
    Low = 1,
    /// Buffer is moderately full, should slow down
    Medium = 2,
    /// Buffer is nearly full, must slow down significantly
    High = 3,
    /// Buffer is at capacity, stop sending immediately
    Critical = 4,
}

impl BackpressureLevel {
    /// Get suggested delay in milliseconds based on backpressure level
    pub fn delay_ms(&self) -> u32 {
        match self {
            BackpressureLevel::None => 0,
            BackpressureLevel::Low => 10,
            BackpressureLevel::Medium => 50,
            BackpressureLevel::High => 200,
            BackpressureLevel::Critical => 1000,
        }
    }

    /// Convert to proto enum value
    pub fn to_proto_level(&self) -> i32 {
        *self as i32
    }

    /// Check if backpressure requires action
    pub fn requires_action(&self) -> bool {
        *self >= BackpressureLevel::Medium
    }

    /// Check if producer should stop sending
    pub fn should_stop(&self) -> bool {
        *self >= BackpressureLevel::Critical
    }
}

impl From<BackpressureLevel> for i32 {
    fn from(level: BackpressureLevel) -> Self {
        level as i32
    }
}

/// Lock-free ring buffer for stream buffering
///
/// Uses atomic CAS operations for thread-safe concurrent access.
/// The capacity must be a power of 2 for efficient modulo operations.
///
/// # Type Parameters
///
/// * `T` - Element type, must be `Send` for cross-thread access
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::streaming::RingBuffer;
///
/// let buffer: RingBuffer<u64> = RingBuffer::new(1024);
///
/// // Push elements
/// let _ = buffer.try_push(42);
///
/// // Check backpressure
/// let level = buffer.backpressure_level();
///
/// // Drain elements
/// let elements = buffer.drain(100);
/// ```
pub struct RingBuffer<T> {
    /// The underlying buffer storage
    buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,

    /// Capacity of the buffer (must be power of 2)
    capacity: usize,

    /// Bitmask for efficient modulo (capacity - 1)
    mask: usize,

    /// Head index (consumer position)
    head: AtomicUsize,

    /// Tail index (producer position)
    tail: AtomicUsize,

    /// Watermarks for backpressure control
    watermarks: Watermarks,

    /// Count of successful push operations
    push_count: AtomicUsize,

    /// Count of successful pop operations
    pop_count: AtomicUsize,

    /// Count of failed push attempts (buffer full)
    push_failures: AtomicUsize,
}

// Safety: T must be Send for cross-thread access
unsafe impl<T: Send> Send for RingBuffer<T> {}
unsafe impl<T: Send> Sync for RingBuffer<T> {}

impl<T> RingBuffer<T> {
    /// Create a new ring buffer with the specified capacity
    ///
    /// # Arguments
    ///
    /// * `capacity` - Must be a power of 2
    ///
    /// # Panics
    ///
    /// Panics if capacity is not a power of 2
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two(),
            "Capacity must be power of 2, got {}",
            capacity
        );
        assert!(capacity > 0, "Capacity must be greater than 0");

        let buffer: Vec<UnsafeCell<MaybeUninit<T>>> = (0..capacity)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();

        Self {
            buffer: buffer.into_boxed_slice(),
            capacity,
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            watermarks: Watermarks::from_capacity(capacity),
            push_count: AtomicUsize::new(0),
            pop_count: AtomicUsize::new(0),
            push_failures: AtomicUsize::new(0),
        }
    }

    /// Create a new ring buffer with custom watermarks
    pub fn with_watermarks(capacity: usize, watermarks: Watermarks) -> Self {
        let mut buffer = Self::new(capacity);
        buffer.watermarks = watermarks;
        buffer
    }

    /// Get the capacity of the buffer
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get the current number of elements in the buffer
    #[inline]
    pub fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Relaxed);
        tail.wrapping_sub(head)
    }

    /// Check if the buffer is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check if the buffer is full
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len() >= self.capacity
    }

    /// Get buffer utilization as a percentage (0-100)
    #[inline]
    pub fn utilization_percent(&self) -> u32 {
        let len = self.len();
        ((len * 100) / self.capacity) as u32
    }

    /// Non-blocking push, returns Err with value if buffer is full
    ///
    /// Uses CAS (Compare-And-Swap) for lock-free operation.
    /// May retry on contention from other producers.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to push
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Value was successfully pushed
    /// * `Err(value)` - Buffer is full, value returned
    pub fn try_push(&self, value: T) -> Result<(), T> {
        loop {
            let tail = self.tail.load(Ordering::Relaxed);
            let next_tail = tail.wrapping_add(1);

            let head = self.head.load(Ordering::Acquire);

            // Check if buffer is full
            if next_tail.wrapping_sub(head) > self.capacity {
                self.push_failures.fetch_add(1, Ordering::Relaxed);
                return Err(value);
            }

            // CAS to claim the slot
            match self.tail.compare_exchange_weak(
                tail,
                next_tail,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let index = tail & self.mask;
                    // Safety: We have exclusive access to this slot after successful CAS
                    unsafe {
                        (*self.buffer[index].get()).write(value);
                    }
                    self.push_count.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                Err(_) => {
                    // CAS failed, another producer won, retry
                    std::hint::spin_loop();
                    continue;
                }
            }
        }
    }

    /// Non-blocking pop, returns None if buffer is empty
    ///
    /// Uses CAS (Compare-And-Swap) for lock-free operation.
    /// May retry on contention from other consumers.
    ///
    /// # Returns
    ///
    /// * `Some(value)` - Successfully popped a value
    /// * `None` - Buffer is empty
    pub fn try_pop(&self) -> Option<T> {
        loop {
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);

            // Check if buffer is empty
            if head == tail {
                return None;
            }

            // CAS to claim the slot
            match self.head.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let index = head & self.mask;
                    // Safety: We have exclusive access to this slot after successful CAS
                    let value = unsafe { (*self.buffer[index].get()).assume_init_read() };
                    self.pop_count.fetch_add(1, Ordering::Relaxed);
                    return Some(value);
                }
                Err(_) => {
                    // CAS failed, another consumer won, retry
                    std::hint::spin_loop();
                    continue;
                }
            }
        }
    }

    /// Drain up to `max` elements from the buffer
    ///
    /// This is more efficient than calling `try_pop` repeatedly
    /// as it batches the operations.
    ///
    /// # Arguments
    ///
    /// * `max` - Maximum number of elements to drain
    ///
    /// # Returns
    ///
    /// Vector of drained elements (may be fewer than `max`)
    pub fn drain(&self, max: usize) -> Vec<T> {
        let mut result = Vec::with_capacity(max.min(self.len()));

        for _ in 0..max {
            match self.try_pop() {
                Some(v) => result.push(v),
                None => break,
            }
        }

        result
    }

    /// Drain all elements from the buffer
    ///
    /// # Returns
    ///
    /// Vector of all elements in the buffer
    pub fn drain_all(&self) -> Vec<T> {
        self.drain(self.capacity)
    }

    /// Get the current backpressure level based on buffer utilization
    ///
    /// This should be used by producers to determine if they need
    /// to slow down or stop sending.
    pub fn backpressure_level(&self) -> BackpressureLevel {
        let len = self.len();
        let critical = self.watermarks.critical();
        let high = self.watermarks.high();
        let medium = self.watermarks.medium();
        let low = self.watermarks.low();

        if len >= critical {
            BackpressureLevel::Critical
        } else if len >= high {
            BackpressureLevel::High
        } else if len >= medium {
            BackpressureLevel::Medium
        } else if len >= low {
            BackpressureLevel::Low
        } else {
            BackpressureLevel::None
        }
    }

    /// Get statistics about the buffer
    pub fn stats(&self) -> RingBufferStats {
        RingBufferStats {
            capacity: self.capacity,
            len: self.len(),
            push_count: self.push_count.load(Ordering::Relaxed),
            pop_count: self.pop_count.load(Ordering::Relaxed),
            push_failures: self.push_failures.load(Ordering::Relaxed),
            utilization_percent: self.utilization_percent(),
            backpressure_level: self.backpressure_level(),
        }
    }

    /// Update watermarks dynamically
    pub fn set_watermarks(&mut self, watermarks: Watermarks) {
        self.watermarks = watermarks;
    }

    /// Get reference to watermarks
    pub fn watermarks(&self) -> &Watermarks {
        &self.watermarks
    }
}

impl<T> Drop for RingBuffer<T> {
    fn drop(&mut self) {
        // Drop any remaining elements
        while self.try_pop().is_some() {}
    }
}

/// Statistics about ring buffer usage
#[derive(Debug, Clone)]
pub struct RingBufferStats {
    /// Total capacity of the buffer
    pub capacity: usize,
    /// Current number of elements
    pub len: usize,
    /// Total successful push operations
    pub push_count: usize,
    /// Total successful pop operations
    pub pop_count: usize,
    /// Total failed push attempts (buffer full)
    pub push_failures: usize,
    /// Current utilization percentage (0-100)
    pub utilization_percent: u32,
    /// Current backpressure level
    pub backpressure_level: BackpressureLevel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_basic_push_pop() {
        let buffer: RingBuffer<u64> = RingBuffer::new(8);

        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);

        // Push some elements
        buffer.try_push(1).expect("push should succeed in test");
        buffer.try_push(2).expect("push should succeed in test");
        buffer.try_push(3).expect("push should succeed in test");

        assert_eq!(buffer.len(), 3);
        assert!(!buffer.is_empty());

        // Pop elements
        assert_eq!(buffer.try_pop(), Some(1));
        assert_eq!(buffer.try_pop(), Some(2));
        assert_eq!(buffer.try_pop(), Some(3));
        assert_eq!(buffer.try_pop(), None);

        assert!(buffer.is_empty());
    }

    #[test]
    fn test_buffer_full() {
        let buffer: RingBuffer<u64> = RingBuffer::new(4);

        // Fill the buffer
        buffer
            .try_push(1)
            .expect("push should succeed when buffer not full");
        buffer
            .try_push(2)
            .expect("push should succeed when buffer not full");
        buffer
            .try_push(3)
            .expect("push should succeed when buffer not full");
        buffer
            .try_push(4)
            .expect("push should succeed when buffer not full");

        // Should fail now
        assert!(buffer.try_push(5).is_err());
        assert!(buffer.is_full());

        // Pop one and push should work
        buffer.try_pop();
        buffer.try_push(5).expect("push should succeed after pop");
    }

    #[test]
    fn test_drain() {
        let buffer: RingBuffer<u64> = RingBuffer::new(16);

        for i in 0..10 {
            buffer
                .try_push(i)
                .expect("push should succeed when buffer not full");
        }

        // Drain first 5
        let drained = buffer.drain(5);
        assert_eq!(drained, vec![0, 1, 2, 3, 4]);
        assert_eq!(buffer.len(), 5);

        // Drain remaining
        let drained = buffer.drain(10);
        assert_eq!(drained, vec![5, 6, 7, 8, 9]);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_backpressure_levels() {
        let buffer: RingBuffer<u64> = RingBuffer::new(1024);

        assert_eq!(buffer.backpressure_level(), BackpressureLevel::None);

        // Fill to low watermark (25%)
        for i in 0..256 {
            buffer
                .try_push(i)
                .expect("push should succeed when buffer not full");
        }
        assert_eq!(buffer.backpressure_level(), BackpressureLevel::Low);

        // Fill to medium (50%)
        for i in 256..512 {
            buffer
                .try_push(i)
                .expect("push should succeed when buffer not full");
        }
        assert_eq!(buffer.backpressure_level(), BackpressureLevel::Medium);

        // Fill to high (75%)
        for i in 512..768 {
            buffer
                .try_push(i)
                .expect("push should succeed when buffer not full");
        }
        assert_eq!(buffer.backpressure_level(), BackpressureLevel::High);

        // Fill to critical (90%)
        for i in 768..922 {
            buffer
                .try_push(i)
                .expect("push should succeed when buffer not full");
        }
        assert_eq!(buffer.backpressure_level(), BackpressureLevel::Critical);
    }

    #[test]
    fn test_concurrent_access() {
        let buffer = Arc::new(RingBuffer::<u64>::new(1024));
        let num_producers = 4;
        let num_consumers = 2;
        let items_per_producer = 1000;
        let producers_done = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];

        // Spawn producers
        for p in 0..num_producers {
            let buffer = Arc::clone(&buffer);
            let producers_done = Arc::clone(&producers_done);
            handles.push(thread::spawn(move || {
                let mut pushed = 0;
                for i in 0..items_per_producer {
                    let value = (p as u64) * items_per_producer as u64 + i as u64;
                    loop {
                        match buffer.try_push(value) {
                            Ok(()) => {
                                pushed += 1;
                                break;
                            }
                            Err(_) => {
                                // Buffer full, yield and retry
                                thread::yield_now();
                            }
                        }
                    }
                }
                producers_done.fetch_add(1, Ordering::Release);
                pushed
            }));
        }

        // Spawn consumers
        for _ in 0..num_consumers {
            let buffer = Arc::clone(&buffer);
            let producers_done = Arc::clone(&producers_done);
            handles.push(thread::spawn(move || {
                let mut popped = 0;
                loop {
                    match buffer.try_pop() {
                        Some(_) => {
                            popped += 1;
                        }
                        None => {
                            // Exit only after all producers finished and buffer is drained.
                            if producers_done.load(Ordering::Acquire) == num_producers
                                && buffer.is_empty()
                            {
                                break;
                            }
                            thread::yield_now();
                        }
                    }
                }
                popped
            }));
        }

        let mut total_pushed = 0;
        let mut total_popped = 0;

        for (i, handle) in handles.into_iter().enumerate() {
            let count = handle.join().expect("thread should not panic");
            if i < num_producers {
                total_pushed += count;
            } else {
                total_popped += count;
            }
        }

        // Account for any remaining items in buffer
        total_popped += buffer.drain_all().len();

        assert_eq!(total_pushed, total_popped);
        assert_eq!(total_pushed, num_producers * items_per_producer);
    }

    #[test]
    fn test_stats() {
        let buffer: RingBuffer<u64> = RingBuffer::new(16);

        buffer.try_push(1).expect("push should succeed in test");
        buffer.try_push(2).expect("push should succeed in test");
        buffer.try_pop();

        let stats = buffer.stats();
        assert_eq!(stats.capacity, 16);
        assert_eq!(stats.len, 1);
        assert_eq!(stats.push_count, 2);
        assert_eq!(stats.pop_count, 1);
        assert_eq!(stats.push_failures, 0);
    }

    #[test]
    #[should_panic(expected = "power of 2")]
    fn test_non_power_of_two_panics() {
        let _buffer: RingBuffer<u64> = RingBuffer::new(100);
    }
}

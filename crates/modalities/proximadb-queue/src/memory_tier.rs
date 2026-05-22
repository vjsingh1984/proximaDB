//! Memory tier — lock-free per-partition ring buffer with backpressure.
//!
//! Each `(topic, partition)` owns one `PartitionMemory`. Producers push
//! messages onto it; consumers pop them off. The disk tier (when wired)
//! writes through transparently — every successful push to the memory tier
//! has already had its bytes appended to the active disk segment.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_queue::ArrayQueue;
use tokio::sync::Notify;

use crate::message::{BackpressureHint, Message, MessageId};
use crate::topic::PartitionId;

const SOFT_PCT: f32 = 0.75;
const HARD_PCT: f32 = 0.95;

pub struct PartitionMemory {
    pub(crate) partition: PartitionId,
    capacity: usize,
    queue: ArrayQueue<MemoryEntry>,
    /// Monotonic offset assigned to each successfully-enqueued message.
    /// Persisted via the disk tier; consumers use it as their commit cursor.
    next_offset: AtomicU64,
    /// Wake up consumers blocked on `poll` when something is enqueued.
    pub(crate) notify: Notify,
}

#[derive(Clone)]
pub struct MemoryEntry {
    pub message: Message,
    pub message_id: MessageId,
    pub offset: u64,
}

impl PartitionMemory {
    pub fn new(partition: PartitionId, capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            partition,
            capacity: cap,
            queue: ArrayQueue::new(cap),
            next_offset: AtomicU64::new(0),
            notify: Notify::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn depth(&self) -> usize {
        self.queue.len()
    }

    pub fn depth_pct(&self) -> f32 {
        self.depth() as f32 / self.capacity as f32
    }

    pub fn pressure(&self) -> Option<PressureLevel> {
        let pct = self.depth_pct();
        if pct >= HARD_PCT {
            Some(PressureLevel::Hard(pct))
        } else if pct >= SOFT_PCT {
            Some(PressureLevel::Soft(pct))
        } else {
            None
        }
    }

    /// Try to enqueue a message. Returns the assigned offset on success or
    /// `Err(QueueFull)` when the ring buffer cannot accept it.
    pub fn try_enqueue(
        self: &Arc<Self>,
        message: Message,
    ) -> std::result::Result<(MessageEntry, Option<BackpressureHint>), Message> {
        let offset = self.next_offset.fetch_add(1, Ordering::Relaxed);
        let id = MessageId::new(self.partition, /* segment_id */ 0, offset);
        let entry = MemoryEntry {
            message: message.clone(),
            message_id: id.clone(),
            offset,
        };
        match self.queue.push(entry) {
            Ok(()) => {
                self.notify.notify_waiters();
                let hint = match self.pressure() {
                    Some(PressureLevel::Soft(_)) => Some(BackpressureHint::Soft),
                    _ => None,
                };
                Ok((
                    MessageEntry {
                        message_id: id,
                        offset,
                    },
                    hint,
                ))
            }
            // ArrayQueue::push returns Err(value) when full — give the
            // caller their message back so they can decide what to do.
            Err(rejected) => Err(rejected.message),
        }
    }

    /// Enqueue a message with a pre-assigned offset (used by recovery and
    /// by the producer path now that `PartitionDiskWriter` owns offset
    /// assignment). Bypasses the memory-tier's internal counter so the
    /// disk frame's offset is the source of truth across restart.
    pub fn enqueue_with_offset(
        self: &Arc<Self>,
        message: Message,
        offset: u64,
    ) -> std::result::Result<(MessageEntry, Option<BackpressureHint>), Message> {
        let id = MessageId::new(self.partition, /* segment_id */ 0, offset);
        let entry = MemoryEntry {
            message: message.clone(),
            message_id: id.clone(),
            offset,
        };
        match self.queue.push(entry) {
            Ok(()) => {
                self.notify.notify_waiters();
                let hint = match self.pressure() {
                    Some(PressureLevel::Soft(_)) => Some(BackpressureHint::Soft),
                    _ => None,
                };
                Ok((
                    MessageEntry {
                        message_id: id,
                        offset,
                    },
                    hint,
                ))
            }
            Err(rejected) => Err(rejected.message),
        }
    }

    /// Drain up to `max` messages from the front of the queue.
    pub fn try_pop_batch(self: &Arc<Self>, max: usize) -> Vec<MemoryEntry> {
        let mut out = Vec::with_capacity(max);
        while out.len() < max {
            match self.queue.pop() {
                Some(e) => out.push(e),
                None => break,
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PressureLevel {
    Soft(f32),
    Hard(f32),
}

impl PressureLevel {
    pub fn pct(self) -> f32 {
        match self {
            Self::Soft(p) | Self::Hard(p) => p,
        }
    }
}

/// Lightweight view returned from `try_enqueue` — the caller turns this into
/// a full `MessageReceipt` once disk fsync is confirmed (Strict mode) or
/// immediately (Lazy mode).
pub struct MessageEntry {
    pub message_id: MessageId,
    pub offset: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(n: u8) -> Message {
        Message::new("topic", "tenant", vec![n])
    }

    #[test]
    fn partition_memory_capacity_is_never_zero() {
        let memory = PartitionMemory::new(3, 0);

        assert_eq!(memory.partition, 3);
        assert_eq!(memory.capacity(), 1);
        assert_eq!(memory.depth(), 0);
        assert_eq!(memory.depth_pct(), 0.0);
        assert!(memory.pressure().is_none());
    }

    #[test]
    fn enqueue_assigns_partition_scoped_ids_and_pop_preserves_fifo_order() {
        let memory = Arc::new(PartitionMemory::new(5, 4));

        let (first, hint) = memory.try_enqueue(message(1)).unwrap();
        let (second, _) = memory.try_enqueue(message(2)).unwrap();

        assert_eq!(first.message_id, MessageId::new(5, 0, 0));
        assert_eq!(first.offset, 0);
        assert_eq!(second.message_id, MessageId::new(5, 0, 1));
        assert!(hint.is_none());
        assert_eq!(memory.depth(), 2);

        let popped = memory.try_pop_batch(10);
        assert_eq!(popped.len(), 2);
        assert_eq!(popped[0].message.payload, vec![1]);
        assert_eq!(popped[1].message.payload, vec![2]);
        assert_eq!(memory.depth(), 0);
    }

    #[test]
    fn pressure_transitions_from_none_to_soft_to_hard() {
        let memory = Arc::new(PartitionMemory::new(1, 4));

        assert!(memory.pressure().is_none());
        memory.try_enqueue(message(1)).unwrap();
        memory.try_enqueue(message(2)).unwrap();
        assert!(memory.pressure().is_none());

        let (_, hint) = memory.try_enqueue(message(3)).unwrap();
        assert!(matches!(hint, Some(BackpressureHint::Soft)));
        let soft = memory.pressure().unwrap();
        assert!(matches!(soft, PressureLevel::Soft(_)));
        assert_eq!(soft.pct(), 0.75);

        memory.try_enqueue(message(4)).unwrap();
        let hard = memory.pressure().unwrap();
        assert!(matches!(hard, PressureLevel::Hard(_)));
        assert_eq!(hard.pct(), 1.0);
    }

    #[test]
    fn full_queue_rejects_message_but_offset_counter_remains_monotonic() {
        let memory = Arc::new(PartitionMemory::new(2, 1));

        let first = memory.try_enqueue(message(1)).unwrap().0;
        let rejected = match memory.try_enqueue(message(2)) {
            Ok(_) => panic!("expected full queue to reject enqueue"),
            Err(message) => message,
        };
        assert_eq!(first.offset, 0);
        assert_eq!(rejected.payload, vec![2]);

        assert_eq!(memory.try_pop_batch(1).len(), 1);
        let next = memory.try_enqueue(message(3)).unwrap().0;
        assert_eq!(next.offset, 2);
        assert_eq!(next.message_id, MessageId::new(2, 0, 2));
    }

    #[test]
    fn pop_batch_respects_max_and_empty_queue_returns_empty_vec() {
        let memory = Arc::new(PartitionMemory::new(0, 3));
        memory.try_enqueue(message(1)).unwrap();
        memory.try_enqueue(message(2)).unwrap();
        memory.try_enqueue(message(3)).unwrap();

        assert_eq!(memory.try_pop_batch(0).len(), 0);
        assert_eq!(memory.try_pop_batch(2).len(), 2);
        assert_eq!(memory.try_pop_batch(2).len(), 1);
        assert!(memory.try_pop_batch(2).is_empty());
    }
}

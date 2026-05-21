//! Memory tier — lock-free per-partition ring buffer with backpressure.
//!
//! Each `(topic, partition)` owns one `PartitionMemory`. Producers push
//! messages onto it; consumers pop them off. The disk tier (when wired)
//! writes through transparently — every successful push to the memory tier
//! has already had its bytes appended to the active disk segment.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

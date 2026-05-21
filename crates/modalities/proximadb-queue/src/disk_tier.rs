//! Disk tier — per-partition segment writer + group-commit fsync.
//!
//! ## Phase 1B scaffold
//!
//! Types and call shape are declared so the rest of the crate compiles
//! against the intended interface. Real segment-write, fsync coordination,
//! and rotation land in a focused follow-up commit. Until then the
//! producer treats every send as memory-only and stamps `fsynced_at` based
//! on the wall clock at enqueue (Strict-mode semantics are approximated,
//! not guaranteed durable on crash).

use crate::config::TopicConfig;
use crate::topic::PartitionId;

/// One segment file. Each `(topic, partition)` has at most one *active*
/// segment at a time; older sealed segments wait for archive + reap.
#[derive(Debug, Clone)]
pub struct Segment {
    pub topic: String,
    pub partition: PartitionId,
    pub segment_id: u64,
    pub size_bytes: u64,
    pub sealed: bool,
}

/// Per-partition disk writer. Wired against `FilesystemFactory` in a
/// follow-up commit so the same code path serves local disk + adls/s3/gcs.
pub struct PartitionDiskWriter {
    _topic: String,
    _partition: PartitionId,
    _config: TopicConfig,
    // active_segment: Mutex<Segment>,        // TODO
    // group_commit:  GroupCommitCoordinator, // TODO
    // filesystem:    Arc<dyn Filesystem>,    // TODO
}

impl PartitionDiskWriter {
    pub fn new(topic: String, partition: PartitionId, config: TopicConfig) -> Self {
        Self {
            _topic: topic,
            _partition: partition,
            _config: config,
        }
    }
}

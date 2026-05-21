//! Queue configuration types.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Durability guarantee for `Producer::send`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// fsync the segment file before returning the receipt. Producers wait
    /// on the per-partition group-commit batch, so concurrent sends share
    /// the fsync cost. Use this for events that cannot be re-derived.
    #[default]
    Strict,
    /// Return after the memory-tier append. Disk fsync happens in the
    /// background. Higher throughput; risks losing the last few unflushed
    /// entries on hard crash. Use for events the producer can re-emit
    /// deterministically (e.g., embedding ingest where the source is
    /// idempotent).
    Lazy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    /// FilesystemFactory URL for queue root, e.g.
    /// `file:///var/lib/proximadb/queue` or `adls://anvaiops/queue`.
    pub root: String,

    /// Optional second-level archive for sealed segments. When set, the
    /// object-tier uploader copies sealed disk segments here, then the
    /// reaper deletes the disk copy after consumers commit past the
    /// segment's last offset. Typically a cheaper / cross-region store.
    pub object_archive: Option<String>,

    /// Default durability for topics that don't override.
    pub default_sync_mode: SyncMode,

    /// Per-topic configuration. Topics not declared here are auto-created
    /// at first use with [`TopicConfig::default`].
    pub topics: HashMap<String, TopicConfig>,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            root: "file:///var/lib/proximadb/queue".to_string(),
            object_archive: None,
            default_sync_mode: SyncMode::Strict,
            topics: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicConfig {
    pub partition_count: u32,
    pub memory_capacity: usize,
    pub disk_rotation_size_mb: u32,
    pub archive_after: Option<Duration>,
    pub max_attempts: u32,
    pub sync_mode_override: Option<SyncMode>,
    pub group_commit_max_wait: Duration,
    pub group_commit_max_batch: usize,
}

impl Default for TopicConfig {
    fn default() -> Self {
        Self {
            partition_count: 16,
            memory_capacity: 4096,
            disk_rotation_size_mb: 16,
            archive_after: None,
            max_attempts: 5,
            sync_mode_override: None,
            group_commit_max_wait: Duration::from_millis(5),
            group_commit_max_batch: 64,
        }
    }
}

impl QueueConfig {
    /// Read env-var overrides. Mirrors the WALConfig pattern in the
    /// existing codebase.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("PROXIMADB_QUEUE_ROOT") {
            cfg.root = v;
        }
        if let Ok(v) = std::env::var("PROXIMADB_QUEUE_OBJECT_ARCHIVE") {
            cfg.object_archive = Some(v);
        }
        if let Ok(v) = std::env::var("PROXIMADB_QUEUE_SYNC_MODE") {
            cfg.default_sync_mode = match v.to_lowercase().as_str() {
                "lazy" => SyncMode::Lazy,
                _ => SyncMode::Strict,
            };
        }
        cfg
    }
}

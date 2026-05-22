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
        // Use the OS temp dir as the default root so cargo test, dev
        // shells, and unit benchmarks work out of the box without root
        // privileges. Production deployments override this via the
        // `PROXIMADB_QUEUE_ROOT` env var (see `from_env`) or by setting
        // `root` explicitly in config — `/var/lib/proximadb/queue` is
        // typical for systemd-managed nodes.
        let temp_root = std::env::temp_dir().join("proximadb-queue");
        Self {
            root: format!("file://{}", temp_root.display()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_queue_config_is_local_strict_and_has_no_declared_topics() {
        let cfg = QueueConfig::default();

        assert!(cfg.root.starts_with("file://"));
        assert!(cfg.root.contains("proximadb-queue"));
        assert_eq!(cfg.object_archive, None);
        assert_eq!(cfg.default_sync_mode, SyncMode::Strict);
        assert!(cfg.topics.is_empty());
    }

    #[test]
    fn default_topic_config_matches_queue_operational_profile() {
        let cfg = TopicConfig::default();

        assert_eq!(cfg.partition_count, 16);
        assert_eq!(cfg.memory_capacity, 4096);
        assert_eq!(cfg.disk_rotation_size_mb, 16);
        assert_eq!(cfg.archive_after, None);
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.sync_mode_override, None);
        assert_eq!(cfg.group_commit_max_wait, Duration::from_millis(5));
        assert_eq!(cfg.group_commit_max_batch, 64);
    }

    #[test]
    fn sync_mode_uses_lowercase_wire_names() {
        assert_eq!(
            serde_json::to_string(&SyncMode::Strict).unwrap(),
            "\"strict\""
        );
        assert_eq!(serde_json::to_string(&SyncMode::Lazy).unwrap(), "\"lazy\"");
        assert_eq!(
            serde_json::from_str::<SyncMode>("\"strict\"").unwrap(),
            SyncMode::Strict
        );
        assert_eq!(
            serde_json::from_str::<SyncMode>("\"lazy\"").unwrap(),
            SyncMode::Lazy
        );
    }

    #[test]
    fn queue_config_round_trips_with_topic_overrides() {
        let mut topics = HashMap::new();
        topics.insert(
            "ingest".to_string(),
            TopicConfig {
                partition_count: 4,
                memory_capacity: 128,
                disk_rotation_size_mb: 8,
                archive_after: Some(Duration::from_secs(60)),
                max_attempts: 3,
                sync_mode_override: Some(SyncMode::Lazy),
                group_commit_max_wait: Duration::from_millis(10),
                group_commit_max_batch: 32,
            },
        );
        let cfg = QueueConfig {
            root: "file:///tmp/proximadb-queue-test".to_string(),
            object_archive: Some("s3://bucket/archive".to_string()),
            default_sync_mode: SyncMode::Strict,
            topics,
        };

        let restored: QueueConfig =
            serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();

        assert_eq!(restored.root, "file:///tmp/proximadb-queue-test");
        assert_eq!(
            restored.object_archive.as_deref(),
            Some("s3://bucket/archive")
        );
        assert_eq!(restored.default_sync_mode, SyncMode::Strict);
        let topic = restored.topics.get("ingest").unwrap();
        assert_eq!(topic.partition_count, 4);
        assert_eq!(topic.sync_mode_override, Some(SyncMode::Lazy));
        assert_eq!(topic.group_commit_max_wait, Duration::from_millis(10));
    }
}

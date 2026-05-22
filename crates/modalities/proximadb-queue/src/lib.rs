//! # ProximaDB Tiered Persistent Queue
//!
//! A queue subsystem designed for write-many-read-once async ingest. Separate
//! from ProximaDB's collection WAL because the access patterns and stability
//! requirements diverge — see the rationale in
//! `~/.claude/plans/jaunty-pondering-cook.md`.
//!
//! ## Three storage tiers
//!
//! - **Disk** (primary for `SyncMode::Strict`): per-partition segment files
//!   under `{queue_root}/{topic}/{partition_id}/{segment_id}.qseg`. Group
//!   commit batches fsync calls so multiple producers amortize one fsync.
//! - **Memory**: lock-free `crossbeam::queue::ArrayQueue<Message>` per
//!   partition. Provides fast consumer reads and backpressure signaling.
//! - **Object store** (recovery + cold archive): sealed disk segments are
//!   uploaded via the existing `FilesystemFactory` (`adls://`, `s3://`,
//!   `gcs://`, `file://`).
//!
//! ## Per-tenant partitioning
//!
//! Each topic has a fixed `partition_count`. `partition_for(tenant_id)`
//! hashes the tenant string into a partition. Same tenant always lands on
//! the same partition → per-tenant FIFO is preserved. Per-partition
//! exclusive consumer leases prevent competing consumers and duplicate work.
//!
//! ## Phase 1B scaffold
//!
//! This commit ships the public API + memory-tier round-trip. Disk tier,
//! object tier, group-commit fsync, partition leases, and crash recovery
//! land as focused follow-up commits (the module files exist with their
//! types but bodies are TODO-marked).

pub mod config;
pub mod consumer;
pub mod disk_tier;
pub mod error;
pub mod fs;
pub mod group_commit;
pub mod memory_tier;
pub mod message;
pub mod metrics;
pub mod object_tier;
pub mod offset_store;
pub mod producer;
pub mod recovery;
pub mod topic;

pub use config::{QueueConfig, SyncMode, TopicConfig};
pub use consumer::Consumer;
pub use error::{QueueError, Result};
pub use message::{Message, MessageId, MessageReceipt};
pub use producer::Producer;
pub use topic::{PartitionId, partition_for};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock, oneshot};
use tokio::task::JoinHandle;
use tracing::info;

use crate::disk_tier::PartitionDiskWriter;
use crate::fs::{LocalFs, QueueFs};
use crate::memory_tier::PartitionMemory;
use crate::object_tier::ObjectTierUploader;

/// Handle to the running queue subsystem. Holds per-topic state and serves
/// `Producer` / `Consumer` handles to callers.
pub struct QueueClient {
    config: QueueConfig,
    /// Resolved filesystem root (URL parsed to a local PathBuf for the
    /// `file://` scheme; full URL preserved in `config.root` for the
    /// adapter case once `proximadb-filesystem` is extracted).
    root_path: PathBuf,
    fs: Arc<dyn QueueFs>,
    topics: RwLock<HashMap<String, Arc<TopicState>>>,
    /// Background-task handles spawned at open. `shutdown()` signals
    /// each one's oneshot and awaits their JoinHandle so a clean exit
    /// drains any in-flight uploads / reaps before returning.
    background_tasks: Mutex<Vec<(JoinHandle<()>, oneshot::Sender<()>)>>,
}

pub(crate) struct TopicState {
    pub(crate) config: TopicConfig,
    pub(crate) memory: Vec<Arc<PartitionMemory>>,
    pub(crate) disk_writers: Vec<Arc<PartitionDiskWriter>>,
}

impl QueueClient {
    /// Open (or initialize) a queue subsystem from the given config.
    ///
    /// Pre-creates topics declared in `config.topics`, opens their disk
    /// writers, and runs `recovery::recover` to replay any segments left
    /// on disk from a previous run. Topics not declared in config get
    /// auto-created lazily on first send/subscribe.
    pub async fn open(config: QueueConfig) -> Result<Arc<Self>> {
        let root_path = resolve_local_root(&config.root)?;
        let fs: Arc<dyn QueueFs> = LocalFs::new_arc();
        fs.create_dir_all(&root_path).await?;

        let client = Arc::new(Self {
            config,
            root_path,
            fs,
            topics: RwLock::new(HashMap::new()),
            background_tasks: Mutex::new(Vec::new()),
        });

        // Snapshot the configured topic names to drop the borrow on
        // client.config before the async ensure_topic_async calls.
        let topic_specs: Vec<(String, TopicConfig)> = client
            .config
            .topics
            .iter()
            .map(|(name, cfg)| (name.clone(), cfg.clone()))
            .collect();
        for (name, topic_cfg) in topic_specs {
            client.ensure_topic_async(&name, topic_cfg).await?;
        }

        // Replay any segments left on disk from a previous run. Recovery
        // is per-topic; topics auto-created later don't have any segments
        // to recover, so the lazy path skips recovery.
        recovery::recover(&client).await?;

        // Object-tier uploader: spawned only when an archive is
        // configured. Mirrors sealed disk segments to the archive root
        // so the queue survives node-loss (PVC-less ECS, k8s pod
        // eviction onto fresh local disk, etc.).
        if let Some(archive_url) = client.config.object_archive.clone() {
            let archive_root = crate::object_tier::resolve_archive_root(&archive_url)?;
            client.fs.create_dir_all(&archive_root).await?;
            let uploader = ObjectTierUploader::new(
                client.fs.clone(),
                client.root_path.clone(),
                archive_root,
            );
            let pair = uploader.start(client.clone());
            client.background_tasks.lock().await.push(pair);
        }

        info!(
            root = %client.config.root,
            topics = client.topics.read().await.len(),
            object_archive = ?client.config.object_archive,
            "proximadb-queue opened"
        );
        Ok(client)
    }

    /// Construct a `Producer` handle. Lightweight — clones an Arc.
    pub fn producer(self: &Arc<Self>) -> Producer {
        Producer::new(self.clone())
    }

    /// Construct a `Consumer` handle within a consumer group. Lightweight.
    pub fn consumer(self: &Arc<Self>, group_id: impl Into<String>) -> Consumer {
        Consumer::new(self.clone(), group_id.into())
    }

    /// Graceful shutdown. Signals every background task (uploader, and
    /// later: reaper, lease renewer) via its oneshot, then awaits the
    /// JoinHandle so any in-flight upload completes before the function
    /// returns. The group-commit drainer is dropped implicitly when
    /// the QueueClient's Arc count hits zero.
    pub async fn shutdown(&self) -> Result<()> {
        let mut tasks = self.background_tasks.lock().await;
        for (handle, tx) in tasks.drain(..) {
            let _ = tx.send(());
            let _ = handle.await;
        }
        info!("proximadb-queue shutdown");
        Ok(())
    }

    pub(crate) fn config(&self) -> &QueueConfig {
        &self.config
    }

    pub(crate) fn fs(&self) -> &Arc<dyn QueueFs> {
        &self.fs
    }

    pub(crate) fn root_path(&self) -> &PathBuf {
        &self.root_path
    }

    pub(crate) async fn topic_state(&self, topic: &str) -> Option<Arc<TopicState>> {
        self.topics.read().await.get(topic).cloned()
    }

    pub(crate) async fn topic_names(&self) -> Vec<String> {
        self.topics.read().await.keys().cloned().collect()
    }

    /// Auto-create a topic at first use with default config if it's not
    /// explicitly declared in `QueueConfig::topics`. Opens disk writers
    /// for every partition.
    pub(crate) async fn ensure_topic_async(
        &self,
        topic: &str,
        cfg: TopicConfig,
    ) -> Result<Arc<TopicState>> {
        if let Some(existing) = self.topics.read().await.get(topic) {
            return Ok(existing.clone());
        }

        let mut disk_writers = Vec::with_capacity(cfg.partition_count as usize);
        for p in 0..cfg.partition_count {
            let writer = PartitionDiskWriter::open(
                topic.to_string(),
                p,
                self.root_path.clone(),
                self.fs.clone(),
                cfg.clone(),
            )
            .await?;
            disk_writers.push(writer);
        }
        let partitions: Vec<Arc<PartitionMemory>> = (0..cfg.partition_count)
            .map(|p| Arc::new(PartitionMemory::new(p, cfg.memory_capacity)))
            .collect();
        let state = Arc::new(TopicState {
            config: cfg,
            memory: partitions,
            disk_writers,
        });

        let mut topics = self.topics.write().await;
        // Double-check after re-acquiring the write lock — another caller
        // may have created the topic in the gap.
        if let Some(existing) = topics.get(topic) {
            return Ok(existing.clone());
        }
        topics.insert(topic.to_string(), state.clone());
        Ok(state)
    }
}

/// Parse `config.root` (a URL like `file:///var/lib/proximadb/queue` or a
/// bare path) into a local PathBuf. Object-store schemes (`adls://`,
/// `s3://`, etc.) are not supported until the proximadb-filesystem
/// extraction lands and the adapter path is wired.
fn resolve_local_root(root: &str) -> Result<PathBuf> {
    if let Some(stripped) = root.strip_prefix("file://") {
        Ok(PathBuf::from(stripped))
    } else if root.contains("://") {
        Err(QueueError::Persistence(format!(
            "queue root scheme not supported by LocalFs: {root}; \
             wait for proximadb-filesystem extraction"
        )))
    } else {
        Ok(PathBuf::from(root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config(root: String) -> QueueConfig {
        let mut config = QueueConfig {
            root,
            default_sync_mode: SyncMode::Lazy,
            ..QueueConfig::default()
        };
        config.topics.insert(
            "ingest".to_string(),
            TopicConfig {
                partition_count: 2,
                memory_capacity: 4,
                sync_mode_override: Some(SyncMode::Lazy),
                ..TopicConfig::default()
            },
        );
        config
    }

    #[test]
    fn resolve_local_root_accepts_file_urls_and_bare_paths() {
        assert_eq!(
            resolve_local_root("file:///tmp/proximadb-queue").unwrap(),
            PathBuf::from("/tmp/proximadb-queue")
        );
        assert_eq!(
            resolve_local_root("/var/lib/proximadb/queue").unwrap(),
            PathBuf::from("/var/lib/proximadb/queue")
        );
    }

    #[test]
    fn resolve_local_root_rejects_non_local_url_schemes() {
        let error = resolve_local_root("s3://bucket/queue").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("queue root scheme not supported")
        );
        assert!(error.to_string().contains("s3://bucket/queue"));
    }

    #[tokio::test]
    async fn open_precreates_declared_topics_and_exposes_lightweight_handles() {
        let dir = tempfile::tempdir().unwrap();
        let client = QueueClient::open(test_config(format!("file://{}", dir.path().display())))
            .await
            .unwrap();

        assert_eq!(client.root_path(), &dir.path().to_path_buf());
        assert_eq!(client.topic_names().await, vec!["ingest".to_string()]);
        let topic = client.topic_state("ingest").await.unwrap();
        assert_eq!(topic.config.partition_count, 2);
        assert_eq!(topic.memory.len(), 2);
        assert_eq!(topic.disk_writers.len(), 2);
        assert_eq!(client.consumer("group-a").group_id(), "group-a");
        client.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn lazy_send_subscribe_poll_and_ack_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let client = QueueClient::open(test_config(format!("file://{}", dir.path().display())))
            .await
            .unwrap();
        let producer = client.producer();
        let message = Message::new("ingest", "tenant-a", b"payload".to_vec());

        let receipt = producer.send(message).await.unwrap();
        assert!(receipt.fsynced_at.is_none());
        assert_eq!(receipt.message_id.partition(), Some(receipt.partition));

        let consumer = client.consumer("group-a");
        consumer
            .subscribe("ingest", &[receipt.partition])
            .await
            .unwrap();
        let polled = consumer.poll(10, Duration::from_millis(1)).await.unwrap();
        assert_eq!(polled.len(), 1);
        assert_eq!(polled[0].payload, b"payload");

        consumer.ack(&[receipt.message_id]).await.unwrap();
        assert!(
            consumer
                .poll(10, Duration::from_millis(1))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn producer_auto_creates_missing_topic_with_default_config() {
        let dir = tempfile::tempdir().unwrap();
        let client = QueueClient::open(QueueConfig {
            root: format!("file://{}", dir.path().display()),
            default_sync_mode: SyncMode::Lazy,
            ..QueueConfig::default()
        })
        .await
        .unwrap();

        let receipt = client
            .producer()
            .send(Message::new("auto", "tenant-a", vec![1]))
            .await
            .unwrap();

        assert!(client.topic_state("auto").await.is_some());
        assert_eq!(receipt.message_id.partition(), Some(receipt.partition));
    }
}

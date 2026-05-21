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

use std::sync::Arc;

use dashmap::DashMap;
use tracing::info;

use crate::memory_tier::PartitionMemory;

/// Handle to the running queue subsystem. Holds per-topic state and serves
/// `Producer` / `Consumer` handles to callers.
pub struct QueueClient {
    config: QueueConfig,
    // topic -> partition_id -> memory tier
    topics: DashMap<String, Arc<TopicState>>,
}

pub(crate) struct TopicState {
    pub(crate) config: TopicConfig,
    pub(crate) memory: Vec<Arc<PartitionMemory>>,
}

impl QueueClient {
    /// Open (or initialize) a queue subsystem from the given config.
    ///
    /// Phase 1B scaffold: initializes the memory tier only. Disk recovery
    /// hook and object-tier upload start are wired in follow-up commits.
    pub async fn open(config: QueueConfig) -> Result<Arc<Self>> {
        let client = Arc::new(Self {
            config,
            topics: DashMap::new(),
        });
        // Pre-create the topic states declared in config so producers and
        // consumers don't race on first use.
        for (name, topic_cfg) in client.config.topics.iter() {
            client.ensure_topic(name, topic_cfg.clone());
        }
        info!(
            root = %client.config.root,
            topics = client.topics.len(),
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

    /// Graceful shutdown. Phase 1B scaffold: no background workers to stop
    /// yet; will drain in-flight fsync batches once the disk tier lands.
    pub async fn shutdown(&self) -> Result<()> {
        info!("proximadb-queue shutdown");
        Ok(())
    }

    pub(crate) fn config(&self) -> &QueueConfig {
        &self.config
    }

    pub(crate) fn topic_state(&self, topic: &str) -> Option<Arc<TopicState>> {
        self.topics.get(topic).map(|e| e.value().clone())
    }

    /// Auto-create a topic at first use with default config if it's not
    /// explicitly declared in `QueueConfig::topics`.
    pub(crate) fn ensure_topic(&self, topic: &str, cfg: TopicConfig) -> Arc<TopicState> {
        if let Some(entry) = self.topics.get(topic) {
            return entry.value().clone();
        }
        let partitions: Vec<Arc<PartitionMemory>> = (0..cfg.partition_count)
            .map(|p| Arc::new(PartitionMemory::new(p, cfg.memory_capacity)))
            .collect();
        let state = Arc::new(TopicState {
            config: cfg,
            memory: partitions,
        });
        self.topics.insert(topic.to_string(), state.clone());
        state
    }
}

//! Two-tier priority scheduler: dedicated sync and async tokio runtimes.
//!
//! Design constraints (from the AnvaiOps ADR):
//!
//! * Sync requests must NEVER preempt async work — they pick up the next
//!   free sync worker. If all sync workers are busy, sync waits in a bounded
//!   queue; if the queue is also full, the request is rejected with 503.
//! * Sync workers reserve their capacity exclusively. Async work CANNOT
//!   spill into the sync pool. The reverse is allowed: idle sync workers
//!   opportunistically steal one batch from `async_queue` after a short
//!   idle interval, so reserved sync capacity is not wasted.
//! * Both pools are sized via `EmbedSchedulerConfig`. Defaults: 4 sync
//!   workers, 8 async workers. Override at startup via env vars in
//!   `EmbedSchedulerConfig::from_env()`.

use std::sync::Arc;
use std::time::Duration;

use crossbeam_queue::ArrayQueue;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tracing::{debug, warn};

use crate::{EmbeddingError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IngestMode {
    Sync,
    Async,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Sync,
    Async,
}

impl From<IngestMode> for Priority {
    fn from(m: IngestMode) -> Self {
        match m {
            IngestMode::Sync => Self::Sync,
            IngestMode::Async => Self::Async,
        }
    }
}

/// Scheduler configuration. Built once at process start and frozen.
#[derive(Debug, Clone)]
pub struct EmbedSchedulerConfig {
    pub sync_workers: usize,
    pub async_workers: usize,
    pub sync_queue_capacity: usize,
    pub async_queue_capacity: usize,
    /// How long a sync worker must be idle before stealing from async_queue.
    pub work_steal_idle: Duration,
}

impl Default for EmbedSchedulerConfig {
    fn default() -> Self {
        Self {
            sync_workers: 4,
            async_workers: 8,
            sync_queue_capacity: 8, // 2× workers; tight cap for hard SLA
            async_queue_capacity: 4096,
            work_steal_idle: Duration::from_millis(5),
        }
    }
}

impl EmbedSchedulerConfig {
    /// Read overrides from environment.
    pub fn from_env() -> Self {
        fn env_usize(key: &str, default: usize) -> usize {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        let d = Self::default();
        Self {
            sync_workers: env_usize("PROXIMADB_EMBED_SYNC_WORKERS", d.sync_workers),
            async_workers: env_usize("PROXIMADB_EMBED_ASYNC_WORKERS", d.async_workers),
            sync_queue_capacity: env_usize("PROXIMADB_EMBED_SYNC_QUEUE_CAP", d.sync_queue_capacity),
            async_queue_capacity: env_usize(
                "PROXIMADB_EMBED_ASYNC_QUEUE_CAP",
                d.async_queue_capacity,
            ),
            work_steal_idle: d.work_steal_idle,
        }
    }
}

/// Boxed task with a completion channel. Runtime dispatches it on the
/// appropriate pool; the inner future encloses the embedding work.
pub struct EmbedTask {
    pub priority: Priority,
    pub work: Box<dyn FnOnce() -> Result<()> + Send + 'static>,
}

/// The dual-pool scheduler. Held inside `EmbeddingService` as a single
/// `Arc<EmbedScheduler>` clone-shared across worker threads.
pub struct EmbedScheduler {
    config: EmbedSchedulerConfig,
    sync_pool: Arc<Runtime>,
    async_pool: Arc<Runtime>,
    sync_queue: Arc<ArrayQueue<EmbedTask>>,
    async_queue: Arc<ArrayQueue<EmbedTask>>,
}

impl EmbedScheduler {
    pub fn new(config: EmbedSchedulerConfig) -> Result<Self> {
        let sync_pool = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(config.sync_workers)
                .thread_name("sync-embed")
                .enable_all()
                .build()
                .map_err(|e| EmbeddingError::Other(e.into()))?,
        );
        let async_pool = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(config.async_workers)
                .thread_name("async-embed")
                .enable_all()
                .build()
                .map_err(|e| EmbeddingError::Other(e.into()))?,
        );

        let sync_queue = Arc::new(ArrayQueue::new(config.sync_queue_capacity));
        let async_queue = Arc::new(ArrayQueue::new(config.async_queue_capacity));

        let scheduler = Self {
            config,
            sync_pool,
            async_pool,
            sync_queue,
            async_queue,
        };
        scheduler.spawn_workers();
        Ok(scheduler)
    }

    fn spawn_workers(&self) {
        // Sync workers — drain sync_queue first, opportunistically steal from
        // async_queue after `work_steal_idle` of no sync work.
        let sync_queue = self.sync_queue.clone();
        let async_queue = self.async_queue.clone();
        let work_steal_idle = self.config.work_steal_idle;
        for i in 0..self.config.sync_workers {
            let sync_queue = sync_queue.clone();
            let async_queue = async_queue.clone();
            self.sync_pool.spawn(async move {
                loop {
                    if let Some(task) = sync_queue.pop() {
                        if let Err(e) = tokio::task::block_in_place(task.work) {
                            warn!(worker = "sync", id = i, error = %e, "embed task failed");
                        }
                        continue;
                    }
                    // Idle: brief sleep, then try stealing one async batch.
                    tokio::time::sleep(work_steal_idle).await;
                    if let Some(task) = async_queue.pop() {
                        debug!(worker = "sync-steal", id = i, "stole async batch");
                        if let Err(e) = tokio::task::block_in_place(task.work) {
                            warn!(worker = "sync-steal", id = i, error = %e,
                                "stolen async task failed");
                        }
                    }
                }
            });
        }

        // Async workers — drain async_queue only. Never touch sync_queue.
        let async_queue = self.async_queue.clone();
        for i in 0..self.config.async_workers {
            let async_queue = async_queue.clone();
            self.async_pool.spawn(async move {
                loop {
                    if let Some(task) = async_queue.pop() {
                        if let Err(e) = tokio::task::block_in_place(task.work) {
                            warn!(worker = "async", id = i, error = %e, "embed task failed");
                        }
                    } else {
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                }
            });
        }
    }

    /// Submit a sync request. Returns immediately with a oneshot receiver
    /// that resolves once the embed work completes (or fails).
    pub fn submit_sync<F, T>(&self, work: F) -> Result<oneshot::Receiver<Result<T>>>
    where
        F: FnOnce() -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel::<Result<T>>();
        let task = EmbedTask {
            priority: Priority::Sync,
            work: Box::new(move || {
                let _ = tx.send(work());
                Ok(())
            }),
        };
        self.sync_queue
            .push(task)
            .map_err(|_| EmbeddingError::QueueFull {
                queue: "sync",
                depth: self.config.sync_queue_capacity,
            })?;
        Ok(rx)
    }

    /// Submit an async request. Fire-and-forget — the drainer is responsible
    /// for tracking completion via the WAL / pending event flag.
    pub fn submit_async<F>(&self, work: F) -> Result<()>
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let task = EmbedTask {
            priority: Priority::Async,
            work: Box::new(work),
        };
        self.async_queue
            .push(task)
            .map_err(|_| EmbeddingError::QueueFull {
                queue: "async",
                depth: self.config.async_queue_capacity,
            })
    }

    pub fn stats(&self) -> SchedulerStats {
        SchedulerStats {
            sync_pending: self.sync_queue.len(),
            async_pending: self.async_queue.len(),
            sync_workers: self.config.sync_workers,
            async_workers: self.config.async_workers,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SchedulerStats {
    pub sync_pending: usize,
    pub async_pending: usize,
    pub sync_workers: usize,
    pub async_workers: usize,
}

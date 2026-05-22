//! Per-partition group-commit coordinator.
//!
//! Mirrors the pattern from
//! `src/storage/persistence/write_ahead_log/batch_sync_coordinator.rs` in
//! the main proximadb crate, scoped to one partition's active segment file.
//!
//! ## Contract
//!
//! Producers call `wait_for_fsync(segment_path)` after appending bytes to
//! the segment. The coordinator collects pending waiters and, after
//! `max_wait` ms OR `max_batch` waiters, runs a single `fsync(segment_path)`
//! call that satisfies all of them. Each waiter receives `Ok(())` (or the
//! batch error) on its oneshot.
//!
//! This is the core of the LSM-style group-commit fsync that lets the
//! queue ack thousands of concurrent producers per fsync call instead of
//! one fsync per producer.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::error::QueueError;
use crate::fs::{QueueFs, Result};

/// One waiter waiting for a fsync of `segment_path`.
struct PendingFsync {
    segment_path: PathBuf,
    reply: oneshot::Sender<Result<()>>,
}

#[derive(Debug, Clone)]
pub struct GroupCommitConfig {
    pub max_wait: Duration,
    pub max_batch: usize,
}

impl Default for GroupCommitConfig {
    fn default() -> Self {
        Self {
            max_wait: Duration::from_millis(5),
            max_batch: 64,
        }
    }
}

pub struct GroupCommitCoordinator {
    pending: Arc<Mutex<Vec<PendingFsync>>>,
    notify: Arc<Notify>,
    config: GroupCommitConfig,
    fs: Arc<dyn QueueFs>,
    _drain_task: JoinHandle<()>,
}

impl GroupCommitCoordinator {
    pub fn new(fs: Arc<dyn QueueFs>, config: GroupCommitConfig) -> Arc<Self> {
        let pending = Arc::new(Mutex::new(Vec::<PendingFsync>::new()));
        let notify = Arc::new(Notify::new());
        let drain_pending = pending.clone();
        let drain_notify = notify.clone();
        let drain_fs = fs.clone();
        let drain_max_wait = config.max_wait;
        let drain_max_batch = config.max_batch;
        let drain_task = tokio::spawn(async move {
            loop {
                // Wait for either a wake notification or the max_wait tick.
                tokio::select! {
                    _ = drain_notify.notified() => {}
                    _ = tokio::time::sleep(drain_max_wait) => {}
                }
                let mut batch = drain_pending.lock().await;
                if batch.is_empty() {
                    continue;
                }
                let take_n = batch.len().min(drain_max_batch);
                let to_flush: Vec<PendingFsync> = batch.drain(..take_n).collect();
                drop(batch);

                // Group by segment_path - one fsync per unique segment.
                let mut by_path: std::collections::HashMap<
                    PathBuf,
                    Vec<oneshot::Sender<Result<()>>>,
                > = std::collections::HashMap::new();
                for item in to_flush {
                    by_path
                        .entry(item.segment_path)
                        .or_default()
                        .push(item.reply);
                }
                for (path, replies) in by_path {
                    let result = drain_fs.fsync(&path).await;
                    // QueueError isn't Clone (anyhow::Error inside Other isn't),
                    // so materialize the error string once and rebuild a fresh
                    // QueueError::Persistence per waiter.
                    let err_str = result.as_ref().err().map(|e| e.to_string());
                    for tx in replies {
                        let waiter_result = match &err_str {
                            Some(s) => Err(QueueError::Persistence(s.clone())),
                            None => Ok(()),
                        };
                        let _ = tx.send(waiter_result);
                    }
                    if let Err(e) = result {
                        warn!(?path, error = %e, "group_commit fsync failed");
                    } else {
                        debug!(?path, "group_commit fsync ok");
                    }
                }
            }
        });

        Arc::new(Self {
            pending,
            notify,
            config,
            fs,
            _drain_task: drain_task,
        })
    }

    /// Register a fsync request for `segment_path`. Returns when the
    /// batch fsync completes (Ok) or fails (Err).
    pub async fn wait_for_fsync(self: &Arc<Self>, segment_path: PathBuf) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.push(PendingFsync {
                segment_path,
                reply: tx,
            });
            // Wake the drain task if we've hit max_batch.
            if pending.len() >= self.config.max_batch {
                self.notify.notify_one();
            }
        }
        rx.await
            .map_err(|_| QueueError::Persistence("group_commit drainer dropped".into()))?
    }

    pub fn config(&self) -> &GroupCommitConfig {
        &self.config
    }

    #[allow(dead_code)]
    pub fn fs(&self) -> &Arc<dyn QueueFs> {
        &self.fs
    }
}

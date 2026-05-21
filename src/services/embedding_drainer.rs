/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! WAL-first async embedding drainer.
//!
//! Reads pending records from the `anvaiops_pending_embed` collection (whose
//! own WAL provides durability), batches them, runs server-side embedding via
//! the `proximadb-embedding` singleton, inserts the embedded records into the
//! target collection, then deletes the pending entries.
//!
//! ## Design
//!
//! The drainer is the WAL-first counterpart of [`ProximaFlightService::
//! embed_text_only_records`] which does inline embedding. When customers
//! send `X-Ingest-Mode: async`, the request handler writes the raw payload
//! into the pending collection (durable via that collection's WAL) and
//! returns 202 immediately. This drainer asynchronously promotes the entries
//! to the target collection with their populated vectors.
//!
//! The pending entries carry these fields in their `props`:
//!
//! - `text`              — raw document text awaiting embedding
//! - `target_collection` — destination collection for the embedded record
//! - `event_id`          — idempotency key (mirrors the OID by default)
//! - `attempt_count`     — incremented on retry; events with count ≥ MAX go
//!                          to `anvaiops_dlq_embed`
//!
//! On startup, the drainer queries the pending collection and resumes work.
//! Process crash mid-drain is safe: pending entries stay in the collection
//! until their canonical-record insert + pending-record delete pair has
//! committed; only fully-promoted entries are deleted.
//!
//! ## Phase 1B status
//!
//! The polling loop, lifecycle, and config are in place. The body of
//! `drain_once` is intentionally a stub that logs the pending depth — wire
//! the actual scan + embed + promote + delete sequence in a follow-up PR
//! once the multi-collection scan client + write-through service surfaces
//! are finalized. The structural skeleton here is the integration point
//! that future work targets.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::api_handlers::request_handlers::UnifiedHandlers;

/// Default name for the pending collection. Override via env so test rigs
/// and the multi-tenant control plane can scope it differently.
pub const PENDING_COLLECTION: &str = "anvaiops_pending_embed";

/// Default name for the dead-letter collection. Entries land here after
/// `EmbeddingDrainerConfig::max_attempts` exhausted retries.
pub const DLQ_COLLECTION: &str = "anvaiops_dlq_embed";

#[derive(Debug, Clone)]
pub struct EmbeddingDrainerConfig {
    pub pending_collection: String,
    pub dlq_collection: String,
    /// How often the drainer polls the pending collection for new work.
    pub poll_interval: Duration,
    /// Records per batch sent to `EmbeddingService::embed_async`.
    pub batch_size: usize,
    /// After this many failed attempts, the pending record is moved to DLQ.
    pub max_attempts: u32,
}

impl Default for EmbeddingDrainerConfig {
    fn default() -> Self {
        Self {
            pending_collection: PENDING_COLLECTION.to_string(),
            dlq_collection: DLQ_COLLECTION.to_string(),
            poll_interval: Duration::from_millis(500),
            batch_size: 64,
            max_attempts: 5,
        }
    }
}

impl EmbeddingDrainerConfig {
    /// Read overrides from environment. Mirrors the EmbedSchedulerConfig
    /// pattern in the proximadb-embedding crate.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("PROXIMADB_EMBED_DRAIN_POLL_MS") {
            if let Ok(ms) = v.parse::<u64>() {
                cfg.poll_interval = Duration::from_millis(ms);
            }
        }
        if let Ok(v) = std::env::var("PROXIMADB_EMBED_DRAIN_BATCH_SIZE") {
            if let Ok(n) = v.parse::<usize>() {
                cfg.batch_size = n.max(1);
            }
        }
        if let Ok(v) = std::env::var("PROXIMADB_EMBED_DRAIN_MAX_ATTEMPTS") {
            if let Ok(n) = v.parse::<u32>() {
                cfg.max_attempts = n.max(1);
            }
        }
        if let Ok(v) = std::env::var("PROXIMADB_EMBED_PENDING_COLLECTION") {
            cfg.pending_collection = v;
        }
        if let Ok(v) = std::env::var("PROXIMADB_EMBED_DLQ_COLLECTION") {
            cfg.dlq_collection = v;
        }
        cfg
    }
}

/// Background task that drains pending text-only records into their target
/// collections with populated vectors.
///
/// `EmbeddingDrainer::start()` spawns the polling loop and returns a handle
/// plus a shutdown sender. Calling `shutdown_tx.send(())` causes the loop
/// to exit cleanly after finishing its current iteration.
pub struct EmbeddingDrainer {
    handlers: Arc<UnifiedHandlers>,
    config: EmbeddingDrainerConfig,
}

impl EmbeddingDrainer {
    pub fn new(handlers: Arc<UnifiedHandlers>, config: EmbeddingDrainerConfig) -> Self {
        Self { handlers, config }
    }

    /// Spawn the drainer onto the current tokio runtime. Returns a join
    /// handle for the spawned task plus a oneshot shutdown sender. Drop the
    /// shutdown sender (or send `()`) to gracefully stop the loop.
    pub fn start(self) -> (JoinHandle<()>, oneshot::Sender<()>) {
        let (tx, mut rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            info!(
                pending = %self.config.pending_collection,
                dlq = %self.config.dlq_collection,
                poll_ms = ?self.config.poll_interval.as_millis(),
                batch_size = %self.config.batch_size,
                "embedding drainer started"
            );

            let mut ticker = tokio::time::interval(self.config.poll_interval);
            // Skip the immediate first tick — the EmbeddingService singleton
            // may not be fully initialized yet at process startup.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await;

            loop {
                tokio::select! {
                    _ = &mut rx => {
                        info!("embedding drainer received shutdown signal");
                        break;
                    }
                    _ = ticker.tick() => {
                        if let Err(e) = self.drain_once().await {
                            warn!(error = %e, "drainer iteration failed");
                        }
                    }
                }
            }

            info!("embedding drainer stopped");
        });
        (handle, tx)
    }

    /// One drainer iteration. Phase 1B scaffold body — to be filled in with
    /// the actual scan + embed + promote + delete sequence in a follow-up
    /// PR once the multi-collection scan client surfaces are ready.
    async fn drain_once(&self) -> anyhow::Result<()> {
        // Probe whether the pending collection exists. If it doesn't, the
        // drainer has nothing to do — the v3 async handler bootstraps it
        // lazily on first write.
        //
        // TODO(phase-1b): add `UnifiedHandlers::scan_collection_for_drainer`
        // that returns up to `batch_size` records and call it here. For each
        // batch:
        //   1. Group by (tenant_id, target_collection, embed_route).
        //   2. Call EmbeddingService::embed_async with the grouped texts.
        //   3. Insert embedded records into `target_collection` via
        //      `handle_record_insert_batch_for_tenant`.
        //   4. Delete the corresponding pending entries from
        //      `self.config.pending_collection`.
        //   5. On embed/insert failure, increment `attempt_count` on the
        //      pending entry; promote to DLQ if `attempt_count >=
        //      self.config.max_attempts`.
        debug!(
            handlers_alive = %Arc::strong_count(&self.handlers),
            pending_collection = %self.config.pending_collection,
            "drain_once tick (stub — embed/promote logic pending)"
        );
        Ok(())
    }
}

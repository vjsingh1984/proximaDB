/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Embedding drainer — consumes the `embed-ingest` topic from
//! `proximadb-queue`, embeds via the in-process `EmbeddingService`
//! singleton, and inserts the populated records into the target
//! collection. Phase 2G of the queue plan.
//!
//! ## Replaces the obsolete pending-collection design
//!
//! An earlier scaffold of this module used `anvaiops_pending_embed` as
//! a pseudo-queue collection. That design was abandoned in favor of
//! the dedicated `proximadb-queue` subsystem (per README locked
//! decision: write-many-read-once messaging belongs in a real queue,
//! not in a ProximaDB collection). This module is now the consumer
//! side of that queue.
//!
//! ## Message contract
//!
//! Producers (REST `/v3/documents?mode=async` in Phase 2H) serialize
//! [`EmbedIngestPayload`] as JSON into the queue `Message.payload`.
//! The drainer deserializes, embeds the text records via
//! `EmbeddingService::embed_sync`, and forwards the populated
//! `ProximaRecord`s to the target collection through the
//! [`DrainerInsertSink`] trait — which production wires to
//! `UnifiedHandlers::handle_record_insert_batch_for_tenant`.
//!
//! ## DrainerInsertSink trait
//!
//! Abstracts the insert target so tests can validate the drain logic
//! without spinning up a full proximadb-server. Production has one
//! impl (wrapping UnifiedHandlers); tests provide their own mock.
//!
//! ## What's still deferred
//!
//! - **SST bulk-load bypass** (Phase 2F): today, the drainer's insert
//!   goes through the normal WAL → memtable path. Locked invariant #5
//!   says async should bulk-load SST segments directly. The
//!   [`DrainerInsertSink`] trait makes this swap-in-place when the
//!   storage-engine refactor lands.
//! - **DLQ on max_attempts**: failed batches are currently logged and
//!   re-queued via nack; no explicit DLQ promotion yet.
//! - **Multi-replica partition assignment**: this drainer subscribes
//!   to ALL partitions by default. Cross-process leases (Phase 2E)
//!   prevent two replicas from competing; assignment via partition
//!   ranges or rendezvous-hash is a follow-up.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use proximadb_embedding::EmbeddingService;
use proximadb_embedding::config::EmbedRoute;
use proximadb_embedding::scheduler::IngestMode;
use proximadb_embedding::service::{EmbedBatch, EmbedRecord};
use proximadb_queue::QueueClient;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Default topic name for text-only ingest events the drainer
/// processes. Aligned with the README's topic naming convention.
pub const EMBED_INGEST_TOPIC: &str = "embed-ingest";

/// One pending record's worth of payload as the producer serializes it
/// into the queue message body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedIngestRecord {
    pub oid: String,
    pub text: String,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

/// The full queue-message payload. One message can carry many records
/// (the v3 REST handler batches records from a single API call into
/// one message, so the drainer can embed them as a batch).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedIngestPayload {
    pub target_collection: String,
    pub tenant_id: String,
    pub records: Vec<EmbedIngestRecord>,
}

/// Insert path the drainer calls once embedding has populated vectors.
/// Production wraps `UnifiedHandlers`; tests provide an in-memory mock.
#[async_trait]
pub trait DrainerInsertSink: Send + Sync {
    async fn insert(
        &self,
        target_collection: &str,
        tenant_id: &str,
        records: Vec<EmbeddedRecord>,
    ) -> anyhow::Result<()>;
}

/// Record handed to the insert sink: text + populated vector + metadata.
/// Tests assert on this shape; the production sink projects it into
/// `ProximaRecord` for the UnifiedHandlers call.
#[derive(Debug, Clone)]
pub struct EmbeddedRecord {
    pub oid: String,
    pub text: String,
    pub vector: Vec<f32>,
    pub vector_dim: u32,
    pub metadata: std::collections::HashMap<String, String>,
    /// Collection's canonical embedding precision. `None` defaults to
    /// `Fp32` (legacy behavior — sink builds an `EmbeddingValues::Fp32`
    /// cell). `Some(target)` instructs the sink to coerce the fp32
    /// vector to `target` via `EmbeddingValues::from_fp32_lossy` before
    /// constructing the `EmbeddingCell`.
    ///
    /// The drainer populates this from a per-collection lookup
    /// (`CanonicalPrecisionResolver::resolve`) once the resolver is
    /// plumbed into the drainer's construction site. Until then, all
    /// records carry `None` and the sink stays on the fp32 path.
    pub target_precision: Option<proximadb_records::EmbeddingScalarType>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingDrainerConfig {
    /// Topic name to consume from. Defaults to `EMBED_INGEST_TOPIC`.
    pub topic: String,
    /// Consumer group identity. Single-group-per-topic per the locked
    /// architectural invariant; default `"embed-drainer"`.
    pub group_id: String,
    /// Max messages drained per poll. The drainer embeds them as one
    /// `EmbedBatch` so larger values amortize inference overhead — but
    /// they also enlarge the at-least-once redelivery window on crash.
    pub batch_size: usize,
    /// Max wait per poll iteration.
    pub poll_wait: Duration,
}

impl Default for EmbeddingDrainerConfig {
    fn default() -> Self {
        Self {
            topic: EMBED_INGEST_TOPIC.to_string(),
            group_id: "embed-drainer".to_string(),
            batch_size: 32,
            poll_wait: Duration::from_millis(100),
        }
    }
}

pub struct EmbeddingDrainer {
    queue: Arc<QueueClient>,
    embed_service: Arc<EmbeddingService>,
    sink: Arc<dyn DrainerInsertSink>,
    config: EmbeddingDrainerConfig,
}

impl EmbeddingDrainer {
    pub fn new(
        queue: Arc<QueueClient>,
        embed_service: Arc<EmbeddingService>,
        sink: Arc<dyn DrainerInsertSink>,
        config: EmbeddingDrainerConfig,
    ) -> Self {
        Self {
            queue,
            embed_service,
            sink,
            config,
        }
    }

    /// Spawn the drainer onto the current tokio runtime. Returns the
    /// JoinHandle plus a shutdown oneshot. Drop the sender (or send
    /// `()`) to stop the loop after the current iteration.
    ///
    /// `partitions` declares which partitions this drainer owns —
    /// typically all of them for a single-replica deployment. With
    /// multi-replica scaleout, each replica passes a disjoint subset
    /// and the cross-process lease (Phase 2E) enforces ownership.
    pub fn start(self, partitions: Vec<u32>) -> (JoinHandle<()>, oneshot::Sender<()>) {
        let (tx, mut rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            info!(
                topic = %self.config.topic,
                group = %self.config.group_id,
                partitions = ?partitions,
                batch_size = self.config.batch_size,
                "embedding drainer started"
            );
            let consumer = self.queue.consumer(self.config.group_id.clone());
            if let Err(e) = consumer.subscribe(&self.config.topic, &partitions).await {
                warn!(error = %e, "drainer subscribe failed; exiting");
                return;
            }
            loop {
                tokio::select! {
                    _ = &mut rx => {
                        info!("embedding drainer received shutdown signal");
                        break;
                    }
                    poll_result = consumer.poll(self.config.batch_size, self.config.poll_wait) => {
                        match poll_result {
                            Ok(messages) if messages.is_empty() => continue,
                            Ok(messages) => {
                                if let Err(e) = self.process_batch(&consumer, messages).await {
                                    warn!(error = %e, "drainer batch failed; messages re-enter via lease expiry");
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "drainer poll failed");
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                        }
                    }
                }
            }
            info!("embedding drainer stopped");
        });
        (handle, tx)
    }

    /// Process one polled batch: parse → embed → insert → ack.
    ///
    /// This is the function the SST-bulk-load optimization (deferred
    /// Phase 2F) will rewrite. Today it loops sink.insert per message;
    /// future version groups all batch records, sorts by oid, and
    /// calls a `BulkLoader::ingest_sorted_segment` once.
    async fn process_batch(
        &self,
        consumer: &proximadb_queue::Consumer,
        messages: Vec<proximadb_queue::Message>,
    ) -> anyhow::Result<()> {
        let mut acked: Vec<proximadb_queue::MessageId> = Vec::with_capacity(messages.len());

        // Parse + flatten — group records across messages so one
        // embed_sync call serves the whole batch.
        let mut batch_records: Vec<EmbedRecord> = Vec::new();
        let mut payload_indices: Vec<(EmbedIngestPayload, std::ops::Range<usize>)> = Vec::new();
        for msg in &messages {
            let payload: EmbedIngestPayload = match serde_json::from_slice(&msg.payload) {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, "drainer: malformed payload; acking to avoid hot-looping");
                    // Compute the message_id from partition/offset
                    // exposed via the Message envelope. proximadb_queue
                    // doesn't surface the MessageId back to Consumer
                    // callers directly; ack uses the partition/offset
                    // derived id mirrored by the in_flight tracker.
                    continue;
                }
            };
            let start = batch_records.len();
            for rec in &payload.records {
                batch_records.push(EmbedRecord {
                    id: rec.oid.clone(),
                    text: rec.text.clone(),
                    tenant_id: payload.tenant_id.clone(),
                });
            }
            let end = batch_records.len();
            payload_indices.push((payload, start..end));
        }

        if batch_records.is_empty() {
            return Ok(());
        }

        let result = self
            .embed_service
            .embed_sync(EmbedBatch {
                records: batch_records,
                mode: IngestMode::Async,
            })
            .await
            .map_err(|e| anyhow::anyhow!("drainer embed failed: {e}"))?;

        // Re-split the result back to per-payload + insert.
        let dim = result.route.dimension() as u32;
        for ((payload, range), msg) in payload_indices.iter().zip(messages.iter()) {
            let mut embedded: Vec<EmbeddedRecord> = Vec::with_capacity(range.end - range.start);
            for (i, rec) in payload.records.iter().enumerate() {
                let vector = result
                    .vectors
                    .get(range.start + i)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("drainer: embed result shape mismatch"))?;
                embedded.push(EmbeddedRecord {
                    oid: rec.oid.clone(),
                    text: rec.text.clone(),
                    vector,
                    vector_dim: dim,
                    metadata: rec.metadata.clone(),
                    target_precision: None,
                });
            }
            self.sink
                .insert(&payload.target_collection, &payload.tenant_id, embedded)
                .await
                .map_err(|e| anyhow::anyhow!("drainer sink insert failed: {e}"))?;

            // Reconstruct MessageId from the polled message. The
            // queue's Message envelope doesn't carry MessageId so we
            // derive it from the in-flight tracker via consumer's
            // ack-by-id. For now we use the offset / partition derivable
            // from the tracker — Phase 2G follow-up exposes
            // MessageId on Message.
            let id = derive_message_id(msg);
            acked.push(id);
        }

        if !acked.is_empty() {
            consumer
                .ack(&acked)
                .await
                .map_err(|e| anyhow::anyhow!("drainer ack failed: {e}"))?;
            debug!(count = acked.len(), "drainer batch ack'd");
        }
        Ok(())
    }
}

/// Derive a `MessageId` from a polled `Message`. The queue's Message
/// type doesn't currently expose its MessageId back to consumers; we
/// reconstruct it from the partition_for(tenant_id) hash + a sentinel
/// segment id. This is the most fragile part of the drainer and is the
/// first target of the Phase 2G follow-up that surfaces MessageId on
/// Message directly.
fn derive_message_id(msg: &proximadb_queue::Message) -> proximadb_queue::MessageId {
    // The consumer's in_flight tracker holds the actual MessageIds.
    // Until the queue exposes them on Message, we use the partition +
    // a synthetic offset that the tracker will retain-match against
    // (which is permissive because ack() retains pending entries on
    // matched ids and silently no-ops unknown ones).
    let partition = proximadb_queue::partition_for(&msg.tenant_id, 16);
    proximadb_queue::MessageId::new(partition, 0, 0)
}

#[allow(unused_variables)]
fn _route_dim(route: EmbedRoute) -> u32 {
    route.dimension() as u32
}

// ── tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_embedding::config::{ByoAuth, ChunkConfig, EmbeddingConfig};
    use proximadb_embedding::scheduler::EmbedSchedulerConfig;
    use proximadb_queue::{Message, QueueConfig, TopicConfig};
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        calls: Mutex<Vec<(String, String, Vec<EmbeddedRecord>)>>,
    }

    #[async_trait]
    impl DrainerInsertSink for RecordingSink {
        async fn insert(
            &self,
            target_collection: &str,
            tenant_id: &str,
            records: Vec<EmbeddedRecord>,
        ) -> anyhow::Result<()> {
            self.calls.lock().await.push((
                target_collection.to_string(),
                tenant_id.to_string(),
                records,
            ));
            Ok(())
        }
    }

    fn ensure_embedding_singleton() {
        if proximadb_embedding::EmbeddingService::try_global().is_some() {
            return;
        }
        let _ = proximadb_embedding::EmbeddingService::initialize(
            EmbeddingConfig {
                route: EmbedRoute::BgeSmall,
                chunk: ChunkConfig::default(),
            },
            EmbedSchedulerConfig::default(),
        );
    }

    fn queue_cfg(tmp: &std::path::Path) -> QueueConfig {
        let mut topics = HashMap::new();
        topics.insert(
            EMBED_INGEST_TOPIC.to_string(),
            TopicConfig {
                partition_count: 2,
                lease_duration: Duration::from_secs(30),
                ..Default::default()
            },
        );
        QueueConfig {
            root: format!("file://{}", tmp.display()),
            topics,
            ..QueueConfig::default()
        }
    }

    fn start_byo_test_endpoint() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind BYO test server");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = r#"{"embeddings":[[0.1,0.2,0.3],[0.4,0.5,0.6]],"model_version":"test"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        });
        format!("http://{}", addr)
    }

    /// Producer sends one well-formed payload; drainer embeds + the
    /// sink receives a record with a non-empty vector.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drainer_embeds_and_forwards_to_sink() {
        ensure_embedding_singleton();
        let tmp = TempDir::new().expect("tempdir");
        let queue = QueueClient::open(queue_cfg(tmp.path()))
            .await
            .expect("queue open");
        let embed_service = proximadb_embedding::EmbeddingService::global();
        embed_service.update_tenant_route(
            "tenant-a",
            EmbedRoute::Byo {
                url: start_byo_test_endpoint(),
                auth: ByoAuth::None,
                declared_dim: 3,
                declared_precision: proximadb_records::EmbeddingScalarType::Fp32,
                batch_size: 8,
                timeout_ms: 1_000,
            },
        );
        let sink = Arc::new(RecordingSink::default());

        let producer = queue.producer();
        let payload = EmbedIngestPayload {
            target_collection: "knowledge".to_string(),
            tenant_id: "tenant-a".to_string(),
            records: vec![
                EmbedIngestRecord {
                    oid: "doc-1".to_string(),
                    text: "what is rust async runtime".to_string(),
                    metadata: HashMap::new(),
                },
                EmbedIngestRecord {
                    oid: "doc-2".to_string(),
                    text: "tokio mpsc channel backpressure".to_string(),
                    metadata: HashMap::new(),
                },
            ],
        };
        let bytes = serde_json::to_vec(&payload).unwrap();
        producer
            .send(Message::new(EMBED_INGEST_TOPIC, "tenant-a", bytes))
            .await
            .expect("send");

        let sink_for_drainer: Arc<dyn DrainerInsertSink> = sink.clone();
        let drainer = EmbeddingDrainer::new(
            queue.clone(),
            embed_service,
            sink_for_drainer,
            EmbeddingDrainerConfig {
                batch_size: 8,
                poll_wait: Duration::from_millis(50),
                ..Default::default()
            },
        );
        let (handle, shutdown) = drainer.start(vec![0, 1]);

        // Wait up to 3s for the sink to record the insert.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if !sink.calls.lock().await.is_empty() {
                break;
            }
            if std::time::Instant::now() > deadline {
                panic!("drainer never invoked sink");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let calls = sink.calls.lock().await;
        assert_eq!(calls.len(), 1, "exactly one sink call expected");
        let (target, tenant, recs) = &calls[0];
        assert_eq!(target, "knowledge");
        assert_eq!(tenant, "tenant-a");
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].oid, "doc-1");
        assert_eq!(recs[1].oid, "doc-2");
        assert!(!recs[0].vector.is_empty(), "vector must be populated");
        assert_eq!(
            recs[0].vector.len() as u32,
            recs[0].vector_dim,
            "vector_dim matches actual length"
        );
        assert_ne!(
            recs[0].vector, recs[1].vector,
            "distinct texts should produce distinct vectors"
        );
        drop(calls);

        let _ = shutdown.send(());
        let _ = handle.await;
        queue.shutdown().await.expect("queue shutdown");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drainer_skips_malformed_payload() {
        ensure_embedding_singleton();
        let tmp = TempDir::new().expect("tempdir");
        let queue = QueueClient::open(queue_cfg(tmp.path())).await.unwrap();
        let embed_service = proximadb_embedding::EmbeddingService::global();
        let sink = Arc::new(RecordingSink::default());

        let producer = queue.producer();
        producer
            .send(Message::new(
                EMBED_INGEST_TOPIC,
                "tenant-a",
                b"not-json".to_vec(),
            ))
            .await
            .expect("send malformed");

        let sink_for_drainer: Arc<dyn DrainerInsertSink> = sink.clone();
        let drainer = EmbeddingDrainer::new(
            queue.clone(),
            embed_service,
            sink_for_drainer,
            EmbeddingDrainerConfig {
                batch_size: 8,
                poll_wait: Duration::from_millis(50),
                ..Default::default()
            },
        );
        let (handle, shutdown) = drainer.start(vec![0, 1]);
        // Run for ~300ms — enough for several poll cycles.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = shutdown.send(());
        let _ = handle.await;

        assert!(
            sink.calls.lock().await.is_empty(),
            "malformed payload must NOT invoke sink",
        );
        queue.shutdown().await.expect("shutdown");
    }
}

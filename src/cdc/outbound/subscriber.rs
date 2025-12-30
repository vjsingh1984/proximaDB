/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! WAL Subscriber for Outbound CDC
//!
//! Provides a streaming interface to subscribe to ProximaDB's Write-Ahead Log
//! with exactly-once delivery guarantees.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock as AsyncRwLock};

// Use std::sync::RwLock for synchronous access
use std::sync::RwLock;

use super::config::{OutboundConfig, StartPosition};
use super::dedup::DeduplicationCache;
use super::exactly_once::{ExactlyOnceConfig, ExactlyOnceManager, IdempotencyKey};
use super::position::{Position, PositionTracker};
use crate::cdc::error::{CdcError, CdcResult};
use crate::cdc::event::{ChangeEvent, Operation, SourceInfo};

/// Status of a subscription
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    /// Subscription is initializing
    Initializing,
    /// Subscription is active and consuming
    Active,
    /// Subscription is paused
    Paused,
    /// Subscription is catching up (behind)
    CatchingUp,
    /// Subscription has encountered an error
    Error,
    /// Subscription is stopped
    Stopped,
}

/// Handle to control a subscription
pub struct SubscriberHandle {
    /// Subscription ID
    pub subscription_id: String,
    /// Control channel
    control_tx: mpsc::Sender<ControlMessage>,
    /// Status
    status: Arc<AsyncRwLock<SubscriptionStatus>>,
    /// Running flag
    running: Arc<AtomicBool>,
}

impl SubscriberHandle {
    /// Pause the subscription
    pub async fn pause(&self) -> CdcResult<()> {
        self.control_tx
            .send(ControlMessage::Pause)
            .await
            .map_err(|e| CdcError::Channel(e.to_string()))?;
        Ok(())
    }

    /// Resume the subscription
    pub async fn resume(&self) -> CdcResult<()> {
        self.control_tx
            .send(ControlMessage::Resume)
            .await
            .map_err(|e| CdcError::Channel(e.to_string()))?;
        Ok(())
    }

    /// Stop the subscription
    pub async fn stop(&self) -> CdcResult<()> {
        self.running.store(false, Ordering::SeqCst);
        let _ = self.control_tx.send(ControlMessage::Stop).await;
        Ok(())
    }

    /// Get current status
    pub async fn status(&self) -> SubscriptionStatus {
        *self.status.read().await
    }

    /// Check if subscription is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Acknowledge an LSN
    pub async fn ack(&self, lsn: u64) -> CdcResult<()> {
        self.control_tx
            .send(ControlMessage::Ack(lsn))
            .await
            .map_err(|e| CdcError::Channel(e.to_string()))?;
        Ok(())
    }
}

/// Control messages for subscriber
enum ControlMessage {
    Pause,
    Resume,
    Stop,
    Ack(u64),
}

/// WAL Subscriber for streaming CDC events
pub struct WalSubscriber {
    /// Subscription ID
    subscription_id: String,
    /// Configuration
    config: OutboundConfig,
    /// Current position tracker
    position_tracker: Arc<PositionTracker>,
    /// Deduplication cache
    dedup_cache: Arc<DeduplicationCache>,
    /// Exactly-once manager (if enabled)
    exactly_once: Option<Arc<ExactlyOnceManager>>,
    /// Current status (async access)
    status: Arc<AsyncRwLock<SubscriptionStatus>>,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Current LSN
    current_lsn: AtomicU64,
    /// Event buffer (sync access)
    event_buffer: RwLock<VecDeque<ChangeEvent>>,
    /// Statistics (sync access)
    stats: RwLock<SubscriberStats>,
    /// Last checkpoint time (sync access)
    last_checkpoint: RwLock<Instant>,
}

/// Statistics for subscriber
#[derive(Debug, Clone, Default)]
pub struct SubscriberStats {
    /// Total events read from WAL
    pub events_read: u64,
    /// Events delivered
    pub events_delivered: u64,
    /// Events acknowledged
    pub events_acknowledged: u64,
    /// Events filtered out
    pub events_filtered: u64,
    /// Duplicate events skipped
    pub events_deduplicated: u64,
    /// Checkpoints made
    pub checkpoints: u64,
    /// Errors encountered
    pub errors: u64,
    /// Bytes processed
    pub bytes_processed: u64,
    /// Current lag (events behind)
    pub current_lag: u64,
}

impl WalSubscriber {
    /// Create a new WAL subscriber
    pub fn new(subscription_id: impl Into<String>, config: OutboundConfig) -> Self {
        let dedup_cache = Arc::new(DeduplicationCache::new(config.dedup_cache_size));

        let exactly_once = if config.exactly_once {
            Some(Arc::new(ExactlyOnceManager::new(ExactlyOnceConfig::default())))
        } else {
            None
        };

        Self {
            subscription_id: subscription_id.into(),
            config,
            position_tracker: Arc::new(PositionTracker::new()),
            dedup_cache,
            exactly_once,
            status: Arc::new(AsyncRwLock::new(SubscriptionStatus::Initializing)),
            running: Arc::new(AtomicBool::new(false)),
            current_lsn: AtomicU64::new(0),
            event_buffer: RwLock::new(VecDeque::new()),
            stats: RwLock::new(SubscriberStats::default()),
            last_checkpoint: RwLock::new(Instant::now()),
        }
    }

    /// Create with position tracker
    pub fn with_position_tracker(mut self, tracker: Arc<PositionTracker>) -> Self {
        self.position_tracker = tracker;
        self
    }

    /// Create with exactly-once manager
    pub fn with_exactly_once(mut self, manager: Arc<ExactlyOnceManager>) -> Self {
        self.exactly_once = Some(manager);
        self
    }

    /// Get subscription ID
    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }

    /// Get current status
    pub async fn status(&self) -> SubscriptionStatus {
        *self.status.read().await
    }

    /// Get current LSN
    pub fn current_lsn(&self) -> u64 {
        self.current_lsn.load(Ordering::SeqCst)
    }

    /// Get statistics
    pub fn stats(&self) -> SubscriberStats {
        self.stats.read().unwrap().clone()
    }

    /// Initialize the subscriber
    pub async fn initialize(&self) -> CdcResult<()> {
        // Determine starting position
        let start_lsn = match &self.config.start_position {
            StartPosition::Beginning => 0,
            StartPosition::Latest => u64::MAX, // Will be resolved to actual latest
            StartPosition::Lsn(lsn) => *lsn,
            StartPosition::Timestamp(_ts) => {
                // Would need to scan WAL to find position by timestamp
                0
            }
            StartPosition::Resume => {
                // Load from position tracker
                self.position_tracker
                    .load(&self.subscription_id)
                    .await?
                    .map(|p| p.lsn)
                    .unwrap_or(0)
            }
        };

        self.current_lsn.store(start_lsn, Ordering::SeqCst);
        self.position_tracker
            .set(&self.subscription_id, Position::from_lsn(start_lsn));

        *self.status.write().await = SubscriptionStatus::Active;
        self.running.store(true, Ordering::SeqCst);

        Ok(())
    }

    /// Start the subscriber and return a handle
    pub async fn start(self: Arc<Self>) -> CdcResult<(SubscriberHandle, mpsc::Receiver<ChangeEvent>)> {
        self.initialize().await?;

        let (control_tx, mut control_rx) = mpsc::channel::<ControlMessage>(32);
        let (event_tx, event_rx) = mpsc::channel::<ChangeEvent>(self.config.batch_size * 2);

        let subscriber = self.clone();

        // Spawn background task
        tokio::spawn(async move {
            let mut paused = false;

            while subscriber.running.load(Ordering::SeqCst) {
                // Check for control messages
                if let Ok(msg) = control_rx.try_recv() {
                    match msg {
                        ControlMessage::Pause => {
                            paused = true;
                            *subscriber.status.write().await = SubscriptionStatus::Paused;
                        }
                        ControlMessage::Resume => {
                            paused = false;
                            *subscriber.status.write().await = SubscriptionStatus::Active;
                        }
                        ControlMessage::Stop => {
                            subscriber.running.store(false, Ordering::SeqCst);
                            break;
                        }
                        ControlMessage::Ack(lsn) => {
                            if let Err(e) = subscriber.acknowledge(lsn).await {
                                tracing::error!("Failed to acknowledge LSN {}: {}", lsn, e);
                            }
                        }
                    }
                }

                if paused {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }

                // Poll for events
                match subscriber.poll_events().await {
                    Ok(events) => {
                        for event in events {
                            if event_tx.send(event).await.is_err() {
                                // Receiver dropped
                                subscriber.running.store(false, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error polling events: {}", e);
                        subscriber.stats.write().unwrap().errors += 1;
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                    }
                }

                // Auto-checkpoint
                if subscriber.config.auto_checkpoint {
                    subscriber.maybe_checkpoint().await;
                }

                // Sleep between polls
                tokio::time::sleep(subscriber.config.poll_interval()).await;
            }

            *subscriber.status.write().await = SubscriptionStatus::Stopped;
        });

        let handle = SubscriberHandle {
            subscription_id: self.subscription_id.clone(),
            control_tx,
            status: self.status.clone(),
            running: self.running.clone(),
        };

        Ok((handle, event_rx))
    }

    /// Poll for new events
    pub async fn poll_events(&self) -> CdcResult<Vec<ChangeEvent>> {
        // In a real implementation, this would read from the WAL
        // For now, return buffered events or simulate
        let mut buffer = self.event_buffer.write().unwrap();
        let events: Vec<ChangeEvent> = buffer.drain(..).collect();

        let mut stats = self.stats.write().unwrap();
        stats.events_read += events.len() as u64;

        // Filter and deduplicate
        let mut result = Vec::new();
        for event in events {
            // Collection filter
            if !self.config.should_include(&event.collection) {
                stats.events_filtered += 1;
                continue;
            }

            // Deduplication
            if self.dedup_cache.check_and_mark(&event) {
                stats.events_deduplicated += 1;
                continue;
            }

            // Update current LSN
            self.current_lsn.store(event.lsn, Ordering::SeqCst);

            // Mark as pending
            self.position_tracker
                .mark_pending(&self.subscription_id, Position::from_lsn(event.lsn));

            stats.events_delivered += 1;
            result.push(event);
        }

        Ok(result)
    }

    /// Acknowledge an LSN
    pub async fn acknowledge(&self, lsn: u64) -> CdcResult<()> {
        self.position_tracker
            .acknowledge(&self.subscription_id, lsn)?;
        self.stats.write().unwrap().events_acknowledged += 1;

        // Commit exactly-once transaction if enabled
        if let Some(ref eo) = self.exactly_once {
            let key = IdempotencyKey::new("wal", lsn, &self.subscription_id);
            // Create and commit a transaction for this ack
            let txn_id = eo.begin_transaction(key).unwrap_or_default();
            if !txn_id.is_empty() {
                eo.commit(&txn_id)?;
            }
        }

        Ok(())
    }

    /// Maybe checkpoint (based on interval)
    async fn maybe_checkpoint(&self) {
        let should_checkpoint = {
            let last = self.last_checkpoint.read().unwrap();
            last.elapsed() >= Duration::from_millis(self.config.checkpoint_interval_ms)
        };

        if should_checkpoint {
            if let Err(e) = self.checkpoint().await {
                tracing::error!("Checkpoint failed: {}", e);
            }
        }
    }

    /// Checkpoint current position
    pub async fn checkpoint(&self) -> CdcResult<()> {
        self.position_tracker.checkpoint(&self.subscription_id).await?;
        self.stats.write().unwrap().checkpoints += 1;
        *self.last_checkpoint.write().unwrap() = Instant::now();
        Ok(())
    }

    /// Push events into the subscriber (for testing or injection)
    pub fn push_events(&self, events: Vec<ChangeEvent>) {
        let mut buffer = self.event_buffer.write().unwrap();
        buffer.extend(events);
    }

    /// Get pending count
    pub fn pending_count(&self) -> usize {
        self.position_tracker.pending_count(&self.subscription_id)
    }

    /// Check if has pending events
    pub fn has_pending(&self) -> bool {
        self.position_tracker.has_pending(&self.subscription_id)
    }

    /// Get current position
    pub fn current_position(&self) -> Option<Position> {
        self.position_tracker.get(&self.subscription_id)
    }
}

/// Builder for creating WalSubscriber instances
pub struct WalSubscriberBuilder {
    subscription_id: String,
    config: OutboundConfig,
    position_tracker: Option<Arc<PositionTracker>>,
    exactly_once: Option<Arc<ExactlyOnceManager>>,
}

impl WalSubscriberBuilder {
    /// Create a new builder
    pub fn new(subscription_id: impl Into<String>) -> Self {
        Self {
            subscription_id: subscription_id.into(),
            config: OutboundConfig::new(),
            position_tracker: None,
            exactly_once: None,
        }
    }

    /// Set configuration
    pub fn with_config(mut self, config: OutboundConfig) -> Self {
        self.config = config;
        self
    }

    /// Set position tracker
    pub fn with_position_tracker(mut self, tracker: Arc<PositionTracker>) -> Self {
        self.position_tracker = Some(tracker);
        self
    }

    /// Enable exactly-once delivery
    pub fn with_exactly_once(mut self, config: ExactlyOnceConfig) -> Self {
        self.exactly_once = Some(Arc::new(ExactlyOnceManager::new(config)));
        self
    }

    /// Add collection to subscribe
    pub fn subscribe_collection(mut self, collection: impl Into<String>) -> Self {
        self.config.collections.insert(collection.into());
        self
    }

    /// Start from beginning
    pub fn from_beginning(mut self) -> Self {
        self.config.start_position = StartPosition::Beginning;
        self
    }

    /// Start from specific LSN
    pub fn from_lsn(mut self, lsn: u64) -> Self {
        self.config.start_position = StartPosition::Lsn(lsn);
        self
    }

    /// Build the subscriber
    pub fn build(self) -> Arc<WalSubscriber> {
        let mut subscriber = WalSubscriber::new(self.subscription_id, self.config);

        if let Some(tracker) = self.position_tracker {
            subscriber.position_tracker = tracker;
        }

        if let Some(eo) = self.exactly_once {
            subscriber.exactly_once = Some(eo);
        }

        Arc::new(subscriber)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_events(count: usize) -> Vec<ChangeEvent> {
        (0..count)
            .map(|i| {
                let mut event = ChangeEvent::new(
                    SourceInfo::proximadb("testdb", "test_server"),
                    Operation::Insert,
                    "products",
                    format!("prod_{}", i),
                );
                event.lsn = i as u64;
                event
            })
            .collect()
    }

    #[test]
    fn test_subscriber_creation() {
        let config = OutboundConfig::new().with_collection("products");
        let subscriber = WalSubscriber::new("test_sub", config);
        assert_eq!(subscriber.subscription_id(), "test_sub");
        assert_eq!(subscriber.current_lsn(), 0);
    }

    #[test]
    fn test_subscriber_builder() {
        let subscriber = WalSubscriberBuilder::new("test_sub")
            .subscribe_collection("products")
            .subscribe_collection("users")
            .from_beginning()
            .build();

        assert_eq!(subscriber.subscription_id(), "test_sub");
        assert!(subscriber.config.collections.contains("products"));
        assert!(subscriber.config.collections.contains("users"));
    }

    #[tokio::test]
    async fn test_subscriber_initialize() {
        let config = OutboundConfig::new().from_lsn(100);
        let subscriber = WalSubscriber::new("test_sub", config);

        subscriber.initialize().await.unwrap();

        assert_eq!(subscriber.current_lsn(), 100);
        assert_eq!(subscriber.status().await, SubscriptionStatus::Active);
    }

    #[test]
    fn test_push_events() {
        let subscriber = WalSubscriber::new("test_sub", OutboundConfig::new());
        let events = create_test_events(5);

        subscriber.push_events(events);

        let buffer = subscriber.event_buffer.read().unwrap();
        assert_eq!(buffer.len(), 5);
    }

    #[tokio::test]
    async fn test_poll_events() {
        let config = OutboundConfig::new().with_collection("products");
        let subscriber = WalSubscriber::new("test_sub", config);

        subscriber.push_events(create_test_events(3));

        let events = subscriber.poll_events().await.unwrap();
        assert_eq!(events.len(), 3);

        let stats = subscriber.stats();
        assert_eq!(stats.events_read, 3);
        assert_eq!(stats.events_delivered, 3);
    }

    #[tokio::test]
    async fn test_collection_filter() {
        let config = OutboundConfig::new().with_collection("users"); // Not products
        let subscriber = WalSubscriber::new("test_sub", config);

        // Push products events (should be filtered)
        subscriber.push_events(create_test_events(3));

        let events = subscriber.poll_events().await.unwrap();
        assert_eq!(events.len(), 0);

        let stats = subscriber.stats();
        assert_eq!(stats.events_filtered, 3);
    }

    #[tokio::test]
    async fn test_deduplication() {
        let config = OutboundConfig::new().with_collection("products");
        let subscriber = WalSubscriber::new("test_sub", config);

        // Create events once and push the same events twice
        let events = create_test_events(3);
        let events_clone = events.clone();

        subscriber.push_events(events);
        let events1 = subscriber.poll_events().await.unwrap();
        assert_eq!(events1.len(), 3);

        // Push the same events again (cloned)
        subscriber.push_events(events_clone);
        let events2 = subscriber.poll_events().await.unwrap();
        assert_eq!(events2.len(), 0); // All duplicates

        let stats = subscriber.stats();
        assert_eq!(stats.events_deduplicated, 3);
    }

    #[tokio::test]
    async fn test_acknowledge() {
        let config = OutboundConfig::new();
        let subscriber = WalSubscriber::new("test_sub", config);

        subscriber.push_events(create_test_events(3));
        let _ = subscriber.poll_events().await.unwrap();

        // Should have pending
        assert!(subscriber.has_pending());

        // Acknowledge all
        subscriber.acknowledge(2).await.unwrap();

        // Should update stats
        let stats = subscriber.stats();
        assert_eq!(stats.events_acknowledged, 1);
    }

    #[tokio::test]
    async fn test_checkpoint() {
        let config = OutboundConfig::new();
        let subscriber = WalSubscriber::new("test_sub", config);

        subscriber.initialize().await.unwrap();
        subscriber.checkpoint().await.unwrap();

        let stats = subscriber.stats();
        assert_eq!(stats.checkpoints, 1);
    }

    #[test]
    fn test_subscription_status_serialization() {
        let status = SubscriptionStatus::Active;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"active\"");

        let parsed: SubscriptionStatus = serde_json::from_str("\"paused\"").unwrap();
        assert_eq!(parsed, SubscriptionStatus::Paused);
    }
}

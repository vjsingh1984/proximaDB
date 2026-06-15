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

//! Subscription Manager
//!
//! Manages the lifecycle of live query subscriptions, including:
//! - Creating and closing subscriptions
//! - Deduplicating subscriptions with same fingerprint
//! - Dispatching vector changes to relevant subscriptions
//! - Cleaning up stale subscriptions

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::evaluator::QueryEvaluator;
use super::subscription::{
    QueryFingerprint, QueryUpdate, ResultChange, ScoredResult, Subscription, SubscriptionConfig,
    SubscriptionId, SubscriptionState, SubscriptionStats, UpdateType,
};

/// Handle returned when creating a subscription
pub struct SubscriptionHandle {
    /// Subscription ID
    pub id: SubscriptionId,
    /// Receiver for query updates
    pub updates: mpsc::Receiver<QueryUpdate>,
}

/// Configuration for the subscription manager
#[derive(Debug, Clone)]
pub struct SubscriptionManagerConfig {
    /// Maximum number of subscriptions
    pub max_subscriptions: usize,
    /// Maximum subscriptions per collection
    pub max_per_collection: usize,
    /// Cleanup interval for stale subscriptions
    pub cleanup_interval: Duration,
    /// Subscription idle timeout
    pub idle_timeout: Duration,
    /// Channel buffer size for updates
    pub update_buffer_size: usize,
    /// Maximum subscriptions sharing a fingerprint
    pub max_fingerprint_sharing: usize,
}

impl Default for SubscriptionManagerConfig {
    fn default() -> Self {
        Self {
            max_subscriptions: 10_000,
            max_per_collection: 1_000,
            cleanup_interval: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(300),
            update_buffer_size: 100,
            max_fingerprint_sharing: 100,
        }
    }
}

/// Subscription manager error
#[derive(Debug, Clone)]
pub enum SubscriptionError {
    /// Maximum subscriptions reached
    TooManySubscriptions,
    /// Maximum subscriptions for collection reached
    TooManyPerCollection(String),
    /// Subscription not found
    NotFound(SubscriptionId),
    /// Invalid configuration
    InvalidConfig(String),
    /// Internal error
    Internal(String),
}

impl std::fmt::Display for SubscriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManySubscriptions => write!(f, "maximum subscriptions reached"),
            Self::TooManyPerCollection(col) => {
                write!(f, "maximum subscriptions for collection {} reached", col)
            }
            Self::NotFound(id) => write!(f, "subscription {} not found", id),
            Self::InvalidConfig(msg) => write!(f, "invalid configuration: {}", msg),
            Self::Internal(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for SubscriptionError {}

/// Manages live query subscriptions
pub struct SubscriptionManager {
    /// All active subscriptions
    subscriptions: DashMap<SubscriptionId, Subscription>,
    /// Index from fingerprint to subscription IDs (for dedup)
    fingerprint_index: DashMap<QueryFingerprint, HashSet<SubscriptionId>>,
    /// Index from collection to subscription IDs (for dispatch)
    collection_index: DashMap<String, HashSet<SubscriptionId>>,
    /// Query evaluator for incremental evaluation
    evaluator: Arc<QueryEvaluator>,
    /// Configuration
    config: SubscriptionManagerConfig,
    /// Metrics
    metrics: SubscriptionMetrics,
}

/// Subscription manager metrics
struct SubscriptionMetrics {
    /// Total subscriptions created
    subscriptions_created: AtomicU64,
    /// Total subscriptions closed
    subscriptions_closed: AtomicU64,
    /// Total updates dispatched
    updates_dispatched: AtomicU64,
    /// Total vectors evaluated
    vectors_evaluated: AtomicU64,
    /// Total deduplication hits
    dedup_hits: AtomicU64,
}

impl Default for SubscriptionMetrics {
    fn default() -> Self {
        Self {
            subscriptions_created: AtomicU64::new(0),
            subscriptions_closed: AtomicU64::new(0),
            updates_dispatched: AtomicU64::new(0),
            vectors_evaluated: AtomicU64::new(0),
            dedup_hits: AtomicU64::new(0),
        }
    }
}

impl SubscriptionManager {
    /// Create a new subscription manager
    pub fn new(config: SubscriptionManagerConfig) -> Self {
        Self {
            subscriptions: DashMap::new(),
            fingerprint_index: DashMap::new(),
            collection_index: DashMap::new(),
            evaluator: Arc::new(QueryEvaluator::new()),
            config,
            metrics: SubscriptionMetrics::default(),
        }
    }

    /// Create a new subscription manager with default configuration
    pub fn with_defaults() -> Self {
        Self::new(SubscriptionManagerConfig::default())
    }

    /// Create a new subscription
    pub async fn subscribe(
        &self,
        collection: String,
        query_vector: Vec<f32>,
        config: SubscriptionConfig,
        filter: Option<String>,
    ) -> Result<SubscriptionHandle, SubscriptionError> {
        // Check capacity
        if self.subscriptions.len() >= self.config.max_subscriptions {
            return Err(SubscriptionError::TooManySubscriptions);
        }

        // Check per-collection limit
        if let Some(ids) = self.collection_index.get(&collection)
            && ids.len() >= self.config.max_per_collection
        {
            return Err(SubscriptionError::TooManyPerCollection(collection));
        }

        // Create channel for updates
        let (tx, rx) = mpsc::channel(self.config.update_buffer_size);

        // Create subscription
        let subscription = Subscription::new(collection.clone(), query_vector, config, filter, tx);
        let id = subscription.id.clone();
        let fingerprint = subscription.fingerprint.clone();

        // Add to indices
        self.collection_index
            .entry(collection)
            .or_default()
            .insert(id.clone());

        self.fingerprint_index
            .entry(fingerprint.clone())
            .or_default()
            .insert(id.clone());

        // Check for fingerprint sharing (deduplication opportunity)
        if let Some(fp_subs) = self.fingerprint_index.get(&fingerprint)
            && fp_subs.len() > 1
        {
            self.metrics.dedup_hits.fetch_add(1, Ordering::Relaxed);
            debug!(
                "Subscription {} shares fingerprint with {} others",
                id,
                fp_subs.len() - 1
            );
        }

        // Store subscription
        self.subscriptions.insert(id.clone(), subscription);
        self.metrics
            .subscriptions_created
            .fetch_add(1, Ordering::Relaxed);

        info!("Created subscription {} for collection", id);

        Ok(SubscriptionHandle { id, updates: rx })
    }

    /// Activate a subscription and send initial results
    pub async fn activate(
        &self,
        id: &SubscriptionId,
        initial_results: Vec<ScoredResult>,
    ) -> Result<(), SubscriptionError> {
        let mut sub = self
            .subscriptions
            .get_mut(id)
            .ok_or_else(|| SubscriptionError::NotFound(id.clone()))?;

        if sub.state != SubscriptionState::Initializing {
            return Err(SubscriptionError::InvalidConfig(
                "subscription already activated".to_string(),
            ));
        }

        // Set initial results
        for (idx, result) in initial_results.iter().enumerate() {
            sub.current_results.insert(ScoredResult {
                vector_id: result.vector_id.clone(),
                score: result.score,
                position: idx as u32,
            });
        }

        // Send initial update if configured
        if sub.config.include_initial {
            let update = QueryUpdate::initial(&sub.id, initial_results);
            if let Err(e) = sub.update_sender.try_send(update) {
                warn!("Failed to send initial update: {}", e);
            }
            sub.mark_update_sent();
        }

        sub.state = SubscriptionState::Active;
        sub.last_evaluated = Some(Instant::now());

        Ok(())
    }

    /// Close a subscription
    pub fn unsubscribe(&self, id: &SubscriptionId) -> Result<(), SubscriptionError> {
        // Remove from main map
        let sub = self
            .subscriptions
            .remove(id)
            .ok_or_else(|| SubscriptionError::NotFound(id.clone()))?
            .1;

        // Remove from collection index
        if let Some(mut ids) = self.collection_index.get_mut(&sub.collection) {
            ids.remove(id);
        }

        // Remove from fingerprint index
        if let Some(mut ids) = self.fingerprint_index.get_mut(&sub.fingerprint) {
            ids.remove(id);
        }

        self.metrics
            .subscriptions_closed
            .fetch_add(1, Ordering::Relaxed);
        info!("Closed subscription {}", id);

        Ok(())
    }

    /// Notify subscriptions about new vectors in a collection
    pub async fn notify_insert(
        &self,
        collection: &str,
        vectors: Vec<(String, Vec<f32>, f32)>, // (id, vector, score_hint)
    ) -> usize {
        let mut notified = 0;

        // Get subscriptions for this collection
        let sub_ids = match self.collection_index.get(collection) {
            Some(ids) => ids.clone(),
            None => return 0,
        };

        self.metrics
            .vectors_evaluated
            .fetch_add(vectors.len() as u64, Ordering::Relaxed);

        for sub_id in sub_ids {
            if let Some(mut sub) = self.subscriptions.get_mut(&sub_id) {
                if !sub.is_active() {
                    continue;
                }

                // Evaluate new vectors against this subscription
                let new_results = self.evaluator.evaluate_vectors(
                    &sub.query_vector,
                    &vectors,
                    sub.config.top_k as usize,
                    sub.config.score_threshold,
                );

                if !new_results.is_empty() {
                    // Merge with current results
                    let mut all_results: Vec<_> = sub.current_results.iter().cloned().collect();
                    all_results.extend(new_results);
                    all_results.sort();
                    all_results.truncate(sub.config.top_k as usize);

                    // Update positions
                    let all_results: Vec<_> = all_results
                        .into_iter()
                        .enumerate()
                        .map(|(i, mut r)| {
                            r.position = i as u32;
                            r
                        })
                        .collect();

                    // Get changes
                    let changes = sub.update_results(all_results);

                    if !changes.is_empty() && sub.should_send_update() {
                        let update = QueryUpdate::incremental(&sub.id, UpdateType::Insert, changes);
                        if sub.update_sender.try_send(update).is_ok() {
                            sub.mark_update_sent();
                            notified += 1;
                        }
                    }
                }
            }
        }

        self.metrics
            .updates_dispatched
            .fetch_add(notified as u64, Ordering::Relaxed);
        notified
    }

    /// Notify subscriptions about removed vectors
    pub async fn notify_remove(&self, collection: &str, vector_ids: &[String]) -> usize {
        let mut notified = 0;

        let sub_ids = match self.collection_index.get(collection) {
            Some(ids) => ids.clone(),
            None => return 0,
        };

        let removed_set: HashSet<_> = vector_ids.iter().collect();

        for sub_id in sub_ids {
            if let Some(mut sub) = self.subscriptions.get_mut(&sub_id) {
                if !sub.is_active() {
                    continue;
                }

                // Check if any removed vectors are in current results
                let affected: Vec<_> = sub
                    .current_results
                    .iter()
                    .filter(|r| removed_set.contains(&r.vector_id))
                    .cloned()
                    .collect();

                if !affected.is_empty() {
                    // Remove affected vectors
                    sub.current_results
                        .retain(|r| !removed_set.contains(&r.vector_id));

                    let changes: Vec<_> = affected
                        .into_iter()
                        .map(|r| ResultChange::Removed {
                            vector_id: r.vector_id,
                            old_score: r.score,
                            old_position: r.position,
                        })
                        .collect();

                    if sub.should_send_update() {
                        let update = QueryUpdate::incremental(&sub.id, UpdateType::Remove, changes);
                        if sub.update_sender.try_send(update).is_ok() {
                            sub.mark_update_sent();
                            notified += 1;
                        }
                    }
                }
            }
        }

        self.metrics
            .updates_dispatched
            .fetch_add(notified as u64, Ordering::Relaxed);
        notified
    }

    /// Get subscription by ID
    pub fn get(&self, id: &SubscriptionId) -> Option<SubscriptionStats> {
        self.subscriptions.get(id).map(|s| s.stats())
    }

    /// Get all subscriptions for a collection
    pub fn get_for_collection(&self, collection: &str) -> Vec<SubscriptionStats> {
        match self.collection_index.get(collection) {
            Some(ids) => ids
                .iter()
                .filter_map(|id| self.subscriptions.get(id).map(|s| s.stats()))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Get manager statistics
    pub fn stats(&self) -> ManagerStats {
        ManagerStats {
            total_subscriptions: self.subscriptions.len(),
            collections_with_subscriptions: self.collection_index.len(),
            unique_fingerprints: self.fingerprint_index.len(),
            subscriptions_created: self.metrics.subscriptions_created.load(Ordering::Relaxed),
            subscriptions_closed: self.metrics.subscriptions_closed.load(Ordering::Relaxed),
            updates_dispatched: self.metrics.updates_dispatched.load(Ordering::Relaxed),
            vectors_evaluated: self.metrics.vectors_evaluated.load(Ordering::Relaxed),
            dedup_hits: self.metrics.dedup_hits.load(Ordering::Relaxed),
        }
    }

    /// Clean up stale subscriptions
    pub async fn cleanup_stale(&self) -> usize {
        let now = Instant::now();
        let mut cleaned = 0;

        let to_remove: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|entry| {
                let sub = entry.value();
                let idle_time = sub.last_evaluated.unwrap_or(sub.created_at);
                now.duration_since(idle_time) > self.config.idle_timeout
            })
            .map(|entry| entry.key().clone())
            .collect();

        for id in to_remove {
            if self.unsubscribe(&id).is_ok() {
                cleaned += 1;
            }
        }

        if cleaned > 0 {
            info!("Cleaned up {} stale subscriptions", cleaned);
        }

        cleaned
    }

    /// Start background cleanup task
    pub fn start_cleanup_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let interval = self.config.cleanup_interval;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                self.cleanup_stale().await;
            }
        })
    }

    /// Pause a subscription
    pub fn pause(&self, id: &SubscriptionId) -> Result<(), SubscriptionError> {
        let mut sub = self
            .subscriptions
            .get_mut(id)
            .ok_or_else(|| SubscriptionError::NotFound(id.clone()))?;

        if sub.state == SubscriptionState::Active {
            sub.state = SubscriptionState::Paused;
            Ok(())
        } else {
            Err(SubscriptionError::InvalidConfig(
                "subscription not active".to_string(),
            ))
        }
    }

    /// Resume a paused subscription
    pub fn resume(&self, id: &SubscriptionId) -> Result<(), SubscriptionError> {
        let mut sub = self
            .subscriptions
            .get_mut(id)
            .ok_or_else(|| SubscriptionError::NotFound(id.clone()))?;

        if sub.state == SubscriptionState::Paused {
            sub.state = SubscriptionState::Active;
            Ok(())
        } else {
            Err(SubscriptionError::InvalidConfig(
                "subscription not paused".to_string(),
            ))
        }
    }
}

/// Manager statistics
#[derive(Debug, Clone)]
pub struct ManagerStats {
    /// Total active subscriptions
    pub total_subscriptions: usize,
    /// Number of collections with subscriptions
    pub collections_with_subscriptions: usize,
    /// Number of unique query fingerprints
    pub unique_fingerprints: usize,
    /// Total subscriptions created
    pub subscriptions_created: u64,
    /// Total subscriptions closed
    pub subscriptions_closed: u64,
    /// Total updates dispatched
    pub updates_dispatched: u64,
    /// Total vectors evaluated
    pub vectors_evaluated: u64,
    /// Deduplication hits
    pub dedup_hits: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subscribe_unsubscribe() {
        let manager = SubscriptionManager::with_defaults();

        let handle = manager
            .subscribe(
                "test_collection".to_string(),
                vec![0.1, 0.2, 0.3],
                SubscriptionConfig::default(),
                None,
            )
            .await
            .expect("subscription should succeed");

        assert!(manager.get(&handle.id).is_some());

        manager
            .unsubscribe(&handle.id)
            .expect("unsubscribe should succeed");
        assert!(manager.get(&handle.id).is_none());
    }

    #[tokio::test]
    async fn test_fingerprint_deduplication() {
        let manager = SubscriptionManager::with_defaults();
        let vector = vec![0.1, 0.2, 0.3];

        let handle1 = manager
            .subscribe(
                "test".to_string(),
                vector.clone(),
                SubscriptionConfig::default(),
                None,
            )
            .await
            .expect("first subscription should succeed");

        let handle2 = manager
            .subscribe(
                "test".to_string(),
                vector,
                SubscriptionConfig::default(),
                None,
            )
            .await
            .expect("second subscription should succeed");

        let stats = manager.stats();
        assert_eq!(stats.total_subscriptions, 2);
        assert_eq!(stats.unique_fingerprints, 1);
        assert_eq!(stats.dedup_hits, 1);

        manager
            .unsubscribe(&handle1.id)
            .expect("unsubscribe handle1 should succeed");
        manager
            .unsubscribe(&handle2.id)
            .expect("unsubscribe handle2 should succeed");
    }

    #[tokio::test]
    async fn test_collection_limit() {
        let config = SubscriptionManagerConfig {
            max_per_collection: 2,
            ..Default::default()
        };
        let manager = SubscriptionManager::new(config);

        let _h1 = manager
            .subscribe(
                "test".to_string(),
                vec![0.1],
                SubscriptionConfig::default(),
                None,
            )
            .await
            .expect("first subscription should succeed");

        let _h2 = manager
            .subscribe(
                "test".to_string(),
                vec![0.2],
                SubscriptionConfig::default(),
                None,
            )
            .await
            .expect("second subscription should succeed");

        let result = manager
            .subscribe(
                "test".to_string(),
                vec![0.3],
                SubscriptionConfig::default(),
                None,
            )
            .await;

        assert!(matches!(
            result,
            Err(SubscriptionError::TooManyPerCollection(_))
        ));
    }

    #[tokio::test]
    async fn test_activate_subscription() {
        let manager = SubscriptionManager::with_defaults();

        let mut handle = manager
            .subscribe(
                "test".to_string(),
                vec![0.1, 0.2],
                SubscriptionConfig {
                    include_initial: true,
                    ..Default::default()
                },
                None,
            )
            .await
            .expect("subscription should succeed");

        let initial_results = vec![ScoredResult {
            vector_id: "v1".to_string(),
            score: 0.9,
            position: 0,
        }];

        manager
            .activate(&handle.id, initial_results)
            .await
            .expect("activation should succeed");

        // Should receive initial update
        let update = handle
            .updates
            .try_recv()
            .expect("should receive initial update");
        assert_eq!(update.update_type, UpdateType::Initial);
        assert_eq!(update.changes.len(), 1);
    }

    #[tokio::test]
    async fn test_pause_resume() {
        let manager = SubscriptionManager::with_defaults();

        let handle = manager
            .subscribe(
                "test".to_string(),
                vec![0.1],
                SubscriptionConfig::default(),
                None,
            )
            .await
            .expect("subscription should succeed");

        manager
            .activate(&handle.id, vec![])
            .await
            .expect("activation should succeed");

        // Pause
        manager.pause(&handle.id).expect("pause should succeed");
        let stats = manager.get(&handle.id).expect("subscription should exist");
        assert_eq!(stats.state, SubscriptionState::Paused);

        // Resume
        manager.resume(&handle.id).expect("resume should succeed");
        let stats = manager.get(&handle.id).expect("subscription should exist");
        assert_eq!(stats.state, SubscriptionState::Active);
    }

    #[tokio::test]
    async fn test_manager_stats() {
        let manager = SubscriptionManager::with_defaults();

        let _h1 = manager
            .subscribe(
                "col1".to_string(),
                vec![0.1],
                SubscriptionConfig::default(),
                None,
            )
            .await
            .expect("first subscription should succeed");

        let _h2 = manager
            .subscribe(
                "col2".to_string(),
                vec![0.2],
                SubscriptionConfig::default(),
                None,
            )
            .await
            .expect("second subscription should succeed");

        let stats = manager.stats();
        assert_eq!(stats.total_subscriptions, 2);
        assert_eq!(stats.collections_with_subscriptions, 2);
        assert_eq!(stats.subscriptions_created, 2);
    }
}

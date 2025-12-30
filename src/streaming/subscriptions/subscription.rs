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

//! Subscription types and configuration

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Unique identifier for a subscription
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubscriptionId(String);

impl SubscriptionId {
    /// Create a new unique subscription ID
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self(format!("sub_{}_{:x}", id, timestamp))
    }

    /// Get the string representation
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SubscriptionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Fingerprint for query deduplication
///
/// Two subscriptions with the same fingerprint will share evaluation results.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QueryFingerprint {
    /// Hash of the query vector (quantized for stability)
    pub vector_hash: u64,
    /// Collection name
    pub collection: String,
    /// Top-K parameter
    pub top_k: u32,
    /// Score threshold (quantized to 3 decimal places)
    pub score_threshold_quantized: i32,
    /// Filter hash (if any)
    pub filter_hash: Option<u64>,
}

impl QueryFingerprint {
    /// Create a fingerprint from subscription parameters
    pub fn from_params(
        vector: &[f32],
        collection: &str,
        top_k: u32,
        score_threshold: f32,
        filter: Option<&str>,
    ) -> Self {
        // Hash the vector (quantize to reduce sensitivity to floating point)
        let vector_hash = Self::hash_vector(vector);

        // Quantize score threshold to 3 decimal places
        let score_threshold_quantized = (score_threshold * 1000.0) as i32;

        // Hash the filter if present
        let filter_hash = filter.map(|f| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            f.hash(&mut hasher);
            hasher.finish()
        });

        Self {
            vector_hash,
            collection: collection.to_string(),
            top_k,
            score_threshold_quantized,
            filter_hash,
        }
    }

    /// Hash a vector for fingerprinting
    fn hash_vector(vector: &[f32]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // Quantize each component to reduce floating point sensitivity
        for &v in vector {
            let quantized = (v * 10000.0) as i32;
            quantized.hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// Subscription state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionState {
    /// Subscription is being initialized
    Initializing,
    /// Subscription is active and receiving updates
    Active,
    /// Subscription is paused (client requested)
    Paused,
    /// Subscription is being closed
    Closing,
    /// Subscription has been closed
    Closed,
}

/// Configuration for a subscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionConfig {
    /// Maximum number of results to track
    pub top_k: u32,
    /// Minimum score threshold for results
    pub score_threshold: f32,
    /// Whether to include initial results on subscription
    pub include_initial: bool,
    /// Debounce duration for updates (to batch rapid changes)
    pub debounce_ms: u64,
    /// Maximum time to wait before forcing an update
    pub max_delay_ms: u64,
    /// Whether to track position changes
    pub track_positions: bool,
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            top_k: 10,
            score_threshold: 0.0,
            include_initial: true,
            debounce_ms: 50,
            max_delay_ms: 200,
            track_positions: true,
        }
    }
}

/// A live query subscription
pub struct Subscription {
    /// Unique subscription ID
    pub id: SubscriptionId,
    /// Query fingerprint for deduplication
    pub fingerprint: QueryFingerprint,
    /// Collection being queried
    pub collection: String,
    /// Query vector
    pub query_vector: Vec<f32>,
    /// Configuration
    pub config: SubscriptionConfig,
    /// Current state
    pub state: SubscriptionState,
    /// Channel for sending updates to the client
    pub update_sender: mpsc::Sender<QueryUpdate>,
    /// Current result set (vector_id, score)
    pub current_results: BTreeSet<ScoredResult>,
    /// Creation time
    pub created_at: Instant,
    /// Last evaluation time
    pub last_evaluated: Option<Instant>,
    /// Last update sent time
    pub last_update_sent: Option<Instant>,
    /// Number of updates sent
    pub updates_sent: u64,
    /// Filter expression (optional)
    pub filter: Option<String>,
}

impl Subscription {
    /// Create a new subscription
    pub fn new(
        collection: String,
        query_vector: Vec<f32>,
        config: SubscriptionConfig,
        filter: Option<String>,
        update_sender: mpsc::Sender<QueryUpdate>,
    ) -> Self {
        let fingerprint = QueryFingerprint::from_params(
            &query_vector,
            &collection,
            config.top_k,
            config.score_threshold,
            filter.as_deref(),
        );

        Self {
            id: SubscriptionId::new(),
            fingerprint,
            collection,
            query_vector,
            config,
            state: SubscriptionState::Initializing,
            update_sender,
            current_results: BTreeSet::new(),
            created_at: Instant::now(),
            last_evaluated: None,
            last_update_sent: None,
            updates_sent: 0,
            filter,
        }
    }

    /// Check if subscription is active
    pub fn is_active(&self) -> bool {
        self.state == SubscriptionState::Active
    }

    /// Get subscription statistics
    pub fn stats(&self) -> SubscriptionStats {
        SubscriptionStats {
            id: self.id.clone(),
            collection: self.collection.clone(),
            state: self.state,
            result_count: self.current_results.len(),
            updates_sent: self.updates_sent,
            age_ms: self.created_at.elapsed().as_millis() as u64,
            last_update_ms: self.last_update_sent.map(|t| t.elapsed().as_millis() as u64),
        }
    }

    /// Update the result set and return changes
    pub fn update_results(&mut self, new_results: Vec<ScoredResult>) -> Vec<ResultChange> {
        let mut changes = Vec::new();
        let old_results = std::mem::take(&mut self.current_results);

        // Find removed results
        for old in &old_results {
            if !new_results.iter().any(|n| n.vector_id == old.vector_id) {
                changes.push(ResultChange::Removed {
                    vector_id: old.vector_id.clone(),
                    old_score: old.score,
                    old_position: old.position,
                });
            }
        }

        // Find new and changed results
        for (idx, new) in new_results.iter().enumerate() {
            let position = idx as u32;

            if let Some(old) = old_results.iter().find(|o| o.vector_id == new.vector_id) {
                // Check if position or score changed
                if self.config.track_positions && old.position != position {
                    changes.push(ResultChange::PositionChanged {
                        vector_id: new.vector_id.clone(),
                        old_position: old.position,
                        new_position: position,
                        score: new.score,
                    });
                } else if (old.score - new.score).abs() > 0.0001 {
                    changes.push(ResultChange::ScoreChanged {
                        vector_id: new.vector_id.clone(),
                        old_score: old.score,
                        new_score: new.score,
                        position,
                    });
                }
            } else {
                changes.push(ResultChange::Added {
                    vector_id: new.vector_id.clone(),
                    score: new.score,
                    position,
                });
            }

            self.current_results.insert(ScoredResult {
                vector_id: new.vector_id.clone(),
                score: new.score,
                position,
            });
        }

        self.last_evaluated = Some(Instant::now());
        changes
    }

    /// Check if debounce period has elapsed
    pub fn should_send_update(&self) -> bool {
        match self.last_update_sent {
            None => true,
            Some(last) => {
                let elapsed = last.elapsed();
                elapsed >= Duration::from_millis(self.config.debounce_ms)
                    || elapsed >= Duration::from_millis(self.config.max_delay_ms)
            }
        }
    }

    /// Mark update as sent
    pub fn mark_update_sent(&mut self) {
        self.last_update_sent = Some(Instant::now());
        self.updates_sent += 1;
    }
}

/// A scored result in the result set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredResult {
    /// Vector ID
    pub vector_id: String,
    /// Similarity score
    pub score: f32,
    /// Position in result set (0-indexed)
    pub position: u32,
}

impl PartialEq for ScoredResult {
    fn eq(&self, other: &Self) -> bool {
        self.vector_id == other.vector_id
    }
}

impl Eq for ScoredResult {}

impl PartialOrd for ScoredResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoredResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Sort by score descending, then by vector_id for stability
        other.score
            .partial_cmp(&self.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| self.vector_id.cmp(&other.vector_id))
    }
}

/// Change detected in result set
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResultChange {
    /// New result added to the set
    Added {
        vector_id: String,
        score: f32,
        position: u32,
    },
    /// Result removed from the set
    Removed {
        vector_id: String,
        old_score: f32,
        old_position: u32,
    },
    /// Result score changed
    ScoreChanged {
        vector_id: String,
        old_score: f32,
        new_score: f32,
        position: u32,
    },
    /// Result position changed
    PositionChanged {
        vector_id: String,
        old_position: u32,
        new_position: u32,
        score: f32,
    },
}

/// Update type for live queries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateType {
    /// Initial result set
    Initial,
    /// Vector was inserted and affected results
    Insert,
    /// Vector was removed and affected results
    Remove,
    /// Vector was updated and affected results
    Update,
    /// Position change in result set
    Reorder,
}

/// Query update sent to clients
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryUpdate {
    /// Update type
    pub update_type: UpdateType,
    /// Changed results
    pub changes: Vec<ResultChange>,
    /// Full current result set (if requested)
    pub full_results: Option<Vec<ScoredResult>>,
    /// Timestamp of the update
    pub timestamp: u64,
    /// Subscription ID
    pub subscription_id: String,
}

impl QueryUpdate {
    /// Create an initial update with full results
    pub fn initial(subscription_id: &SubscriptionId, results: Vec<ScoredResult>) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            update_type: UpdateType::Initial,
            changes: results
                .iter()
                .map(|r| ResultChange::Added {
                    vector_id: r.vector_id.clone(),
                    score: r.score,
                    position: r.position,
                })
                .collect(),
            full_results: Some(results),
            timestamp,
            subscription_id: subscription_id.to_string(),
        }
    }

    /// Create an incremental update
    pub fn incremental(
        subscription_id: &SubscriptionId,
        update_type: UpdateType,
        changes: Vec<ResultChange>,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        Self {
            update_type,
            changes,
            full_results: None,
            timestamp,
            subscription_id: subscription_id.to_string(),
        }
    }
}

/// Subscription statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionStats {
    /// Subscription ID
    pub id: SubscriptionId,
    /// Collection name
    pub collection: String,
    /// Current state
    pub state: SubscriptionState,
    /// Number of results in current set
    pub result_count: usize,
    /// Number of updates sent
    pub updates_sent: u64,
    /// Age in milliseconds
    pub age_ms: u64,
    /// Time since last update (ms)
    pub last_update_ms: Option<u64>,
}

// Implement Serialize for SubscriptionState
impl Serialize for SubscriptionState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            SubscriptionState::Initializing => serializer.serialize_str("initializing"),
            SubscriptionState::Active => serializer.serialize_str("active"),
            SubscriptionState::Paused => serializer.serialize_str("paused"),
            SubscriptionState::Closing => serializer.serialize_str("closing"),
            SubscriptionState::Closed => serializer.serialize_str("closed"),
        }
    }
}

// Implement Deserialize for SubscriptionState
impl<'de> Deserialize<'de> for SubscriptionState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "initializing" => Ok(SubscriptionState::Initializing),
            "active" => Ok(SubscriptionState::Active),
            "paused" => Ok(SubscriptionState::Paused),
            "closing" => Ok(SubscriptionState::Closing),
            "closed" => Ok(SubscriptionState::Closed),
            _ => Err(serde::de::Error::custom(format!(
                "unknown subscription state: {}",
                s
            ))),
        }
    }
}

// Implement Serialize/Deserialize for SubscriptionId
impl Serialize for SubscriptionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SubscriptionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(SubscriptionId(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscription_id_unique() {
        let id1 = SubscriptionId::new();
        let id2 = SubscriptionId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_query_fingerprint_same_params() {
        let vector = vec![0.1, 0.2, 0.3];
        let fp1 = QueryFingerprint::from_params(&vector, "test", 10, 0.8, None);
        let fp2 = QueryFingerprint::from_params(&vector, "test", 10, 0.8, None);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_query_fingerprint_different_vectors() {
        let v1 = vec![0.1, 0.2, 0.3];
        let v2 = vec![0.1, 0.2, 0.4];
        let fp1 = QueryFingerprint::from_params(&v1, "test", 10, 0.8, None);
        let fp2 = QueryFingerprint::from_params(&v2, "test", 10, 0.8, None);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn test_scored_result_ordering() {
        let r1 = ScoredResult {
            vector_id: "v1".to_string(),
            score: 0.9,
            position: 0,
        };
        let r2 = ScoredResult {
            vector_id: "v2".to_string(),
            score: 0.8,
            position: 1,
        };
        // Higher score should come first
        assert!(r1 < r2);
    }

    #[test]
    fn test_subscription_update_results() {
        let (tx, _rx) = mpsc::channel(10);
        let mut sub = Subscription::new(
            "test".to_string(),
            vec![0.1, 0.2],
            SubscriptionConfig::default(),
            None,
            tx,
        );
        sub.state = SubscriptionState::Active;

        // Initial results
        let results = vec![
            ScoredResult {
                vector_id: "v1".to_string(),
                score: 0.9,
                position: 0,
            },
            ScoredResult {
                vector_id: "v2".to_string(),
                score: 0.8,
                position: 1,
            },
        ];
        let changes = sub.update_results(results);
        assert_eq!(changes.len(), 2);

        // Update with new result
        let new_results = vec![
            ScoredResult {
                vector_id: "v3".to_string(),
                score: 0.95,
                position: 0,
            },
            ScoredResult {
                vector_id: "v1".to_string(),
                score: 0.9,
                position: 1,
            },
        ];
        let changes = sub.update_results(new_results);
        // Should have: v3 added, v2 removed, v1 position changed
        assert!(changes.len() >= 2);
    }

    #[test]
    fn test_subscription_config_defaults() {
        let config = SubscriptionConfig::default();
        assert_eq!(config.top_k, 10);
        assert!(config.include_initial);
    }
}

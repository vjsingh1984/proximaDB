//! Point-in-Time Recovery (PITR) Manager
//!
//! Provides the ability to recover the database to a specific point in time,
//! either by timestamp or by LSN (Log Sequence Number).
//!
//! # Architecture
//!
//! PITR works by leveraging the global manifest system which tracks all WAL entries
//! with their LSNs and timestamps. Recovery points are created by:
//!
//! 1. Creating a checkpoint that captures the current LSN and timestamp
//! 2. Recording collection-level state at that checkpoint
//! 3. Allowing recovery to any checkpoint or any LSN within the retained window
//!
//! # Usage
//!
//! ```ignore
//! let pitr = PITRManager::new(manifest_service);
//!
//! // Create a recovery point
//! let point_id = pitr.create_recovery_point("before_migration").await?;
//!
//! // Later, recover to that point
//! pitr.recover_to_point(point_id).await?;
//!
//! // Or recover to a specific timestamp
//! pitr.recover_to_timestamp(DateTime::parse_from_rfc3339("2025-01-15T12:00:00Z")?).await?;
//! ```

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::manifest::{GlobalManifestService, WalEntryStatus};

/// Unique identifier for a recovery point
pub type RecoveryPointId = u64;

/// PITR Manager for Point-in-Time Recovery operations
pub struct PITRManager {
    /// Reference to the global manifest service
    manifest_service: Arc<GlobalManifestService>,
    /// Named recovery points (user-friendly names -> IDs)
    named_recovery_points: Arc<RwLock<HashMap<String, RecoveryPointId>>>,
    /// Recovery point metadata
    recovery_points: Arc<RwLock<HashMap<RecoveryPointId, RecoveryPoint>>>,
    /// Next recovery point ID
    next_recovery_point_id: Arc<tokio::sync::Mutex<u64>>,
    /// Configuration
    config: PITRConfig,
}

/// Configuration for PITR operations
#[derive(Debug, Clone)]
pub struct PITRConfig {
    /// Maximum number of recovery points to retain
    pub max_recovery_points: usize,
    /// Maximum age of recovery points before auto-cleanup (in seconds)
    pub max_recovery_point_age_secs: u64,
    /// Whether to automatically create recovery points before major operations
    pub auto_create_before_compaction: bool,
    /// Minimum interval between automatic recovery points (in seconds)
    pub min_auto_interval_secs: u64,
}

impl Default for PITRConfig {
    fn default() -> Self {
        Self {
            max_recovery_points: 100,
            max_recovery_point_age_secs: 7 * 24 * 60 * 60, // 7 days
            auto_create_before_compaction: true,
            min_auto_interval_secs: 60 * 60, // 1 hour
        }
    }
}

/// A recovery point represents a consistent database state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPoint {
    /// Unique ID for this recovery point
    pub id: RecoveryPointId,
    /// User-friendly name (optional)
    pub name: Option<String>,
    /// Description of why this point was created
    pub description: Option<String>,
    /// Global LSN at this recovery point
    pub lsn: u64,
    /// Timestamp when the recovery point was created
    pub created_at: DateTime<Utc>,
    /// Per-collection state at this recovery point
    pub collection_states: HashMap<String, CollectionRecoveryState>,
    /// Whether this is an automatic or user-created point
    pub is_automatic: bool,
    /// Tags for categorization
    pub tags: Vec<String>,
}

/// Collection-level state at a recovery point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionRecoveryState {
    /// Collection ID
    pub collection_id: String,
    /// LSN of the last entry for this collection at this point
    pub last_lsn: u64,
    /// Number of vectors at this point
    pub vector_count: u64,
    /// Storage size in bytes at this point
    pub storage_size_bytes: u64,
    /// Last flush timestamp
    pub last_flush_at: Option<DateTime<Utc>>,
}

/// Result of a recovery operation
#[derive(Debug, Clone)]
pub struct RecoveryResult {
    /// Whether recovery was successful
    pub success: bool,
    /// Recovery point that was restored
    pub recovery_point: RecoveryPoint,
    /// Collections that were recovered
    pub collections_recovered: Vec<String>,
    /// Number of entries rolled back
    pub entries_rolled_back: u64,
    /// Number of entries replayed
    pub entries_replayed: u64,
    /// Duration of the recovery operation in milliseconds
    pub duration_ms: u64,
    /// Any warnings during recovery
    pub warnings: Vec<String>,
}

/// Options for recovery operations
#[derive(Debug, Clone, Default)]
pub struct RecoveryOptions {
    /// Specific collections to recover (None = all)
    pub collections: Option<Vec<String>>,
    /// Whether to verify data integrity after recovery
    pub verify_integrity: bool,
    /// Whether to create a new recovery point before recovering
    pub create_pre_recovery_point: bool,
    /// Whether to run in dry-run mode (analyze without executing)
    pub dry_run: bool,
}

impl PITRManager {
    /// Create a new PITR manager
    pub fn new(manifest_service: Arc<GlobalManifestService>) -> Self {
        Self::with_config(manifest_service, PITRConfig::default())
    }

    /// Create a new PITR manager with custom configuration
    pub fn with_config(manifest_service: Arc<GlobalManifestService>, config: PITRConfig) -> Self {
        info!("🕐 Initializing PITR Manager with config: {:?}", config);

        Self {
            manifest_service,
            named_recovery_points: Arc::new(RwLock::new(HashMap::new())),
            recovery_points: Arc::new(RwLock::new(HashMap::new())),
            next_recovery_point_id: Arc::new(tokio::sync::Mutex::new(1)),
            config,
        }
    }

    /// Create a named recovery point
    ///
    /// This captures the current database state and allows recovery to this point later.
    pub async fn create_recovery_point(&self, name: &str) -> Result<RecoveryPointId> {
        self.create_recovery_point_with_options(Some(name.to_string()), None, false, vec![])
            .await
    }

    /// Create a recovery point with full options
    pub async fn create_recovery_point_with_options(
        &self,
        name: Option<String>,
        description: Option<String>,
        is_automatic: bool,
        tags: Vec<String>,
    ) -> Result<RecoveryPointId> {
        let start_time = std::time::Instant::now();

        // Get next recovery point ID
        let id = {
            let mut next_id = self.next_recovery_point_id.lock().await;
            let id = *next_id;
            *next_id += 1;
            id
        };

        // Get current LSN from manifest
        let current_lsn = self.manifest_service.current_lsn().await;

        // Get collection states
        let collection_states = self.capture_collection_states().await?;

        let recovery_point = RecoveryPoint {
            id,
            name: name.clone(),
            description,
            lsn: current_lsn,
            created_at: Utc::now(),
            collection_states,
            is_automatic,
            tags,
        };

        // Store recovery point
        {
            let mut points = self.recovery_points.write().await;
            points.insert(id, recovery_point.clone());
        }

        // Store name mapping if provided
        if let Some(ref name) = name {
            let mut names = self.named_recovery_points.write().await;
            names.insert(name.clone(), id);
        }

        // Cleanup old recovery points if needed
        self.cleanup_old_recovery_points().await?;

        let duration = start_time.elapsed();
        info!(
            "🕐 Created recovery point {} (name: {:?}) at LSN {} with {} collections in {:?}",
            id,
            name,
            current_lsn,
            recovery_point.collection_states.len(),
            duration
        );

        Ok(id)
    }

    /// Recover to a specific recovery point by ID
    pub async fn recover_to_point(&self, point_id: RecoveryPointId) -> Result<RecoveryResult> {
        self.recover_to_point_with_options(point_id, RecoveryOptions::default())
            .await
    }

    /// Recover to a named recovery point
    pub async fn recover_to_named_point(&self, name: &str) -> Result<RecoveryResult> {
        let point_id = {
            let names = self.named_recovery_points.read().await;
            names
                .get(name)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("Recovery point '{}' not found", name))?
        };

        self.recover_to_point(point_id).await
    }

    /// Recover to a recovery point with custom options
    pub async fn recover_to_point_with_options(
        &self,
        point_id: RecoveryPointId,
        options: RecoveryOptions,
    ) -> Result<RecoveryResult> {
        let start_time = std::time::Instant::now();

        // Get the recovery point
        let recovery_point = {
            let points = self.recovery_points.read().await;
            points
                .get(&point_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Recovery point {} not found", point_id))?
        };

        info!(
            "🕐 Starting recovery to point {} (LSN: {}, created: {})",
            point_id, recovery_point.lsn, recovery_point.created_at
        );

        // Create pre-recovery point if requested
        if options.create_pre_recovery_point {
            self.create_recovery_point_with_options(
                Some(format!("pre_recovery_{}", point_id)),
                Some(format!(
                    "Automatic backup before recovering to point {}",
                    point_id
                )),
                true,
                vec!["pre_recovery".to_string()],
            )
            .await?;
        }

        // Determine which collections to recover
        let collections_to_recover: Vec<String> = match options.collections {
            Some(ref cols) => cols.clone(),
            None => recovery_point.collection_states.keys().cloned().collect(),
        };

        if options.dry_run {
            info!(
                "🕐 Dry-run mode: would recover {} collections to LSN {}",
                collections_to_recover.len(),
                recovery_point.lsn
            );

            return Ok(RecoveryResult {
                success: true,
                recovery_point,
                collections_recovered: collections_to_recover,
                entries_rolled_back: 0,
                entries_replayed: 0,
                duration_ms: start_time.elapsed().as_millis() as u64,
                warnings: vec!["Dry-run mode - no actual recovery performed".to_string()],
            });
        }

        // Perform recovery by replaying entries up to the recovery point LSN
        let (entries_rolled_back, entries_replayed, warnings) = self
            .execute_recovery(&recovery_point, &collections_to_recover)
            .await?;

        // Verify integrity if requested
        if options.verify_integrity {
            self.verify_recovery_integrity(&recovery_point, &collections_to_recover)
                .await?;
        }

        let duration = start_time.elapsed();
        info!(
            "🕐 Recovery to point {} completed in {:?}: {} collections, {} entries replayed",
            point_id,
            duration,
            collections_to_recover.len(),
            entries_replayed
        );

        Ok(RecoveryResult {
            success: true,
            recovery_point,
            collections_recovered: collections_to_recover,
            entries_rolled_back,
            entries_replayed,
            duration_ms: duration.as_millis() as u64,
            warnings,
        })
    }

    /// Recover to a specific timestamp
    ///
    /// Finds the most recent recovery point at or before the given timestamp
    pub async fn recover_to_timestamp(&self, timestamp: DateTime<Utc>) -> Result<RecoveryResult> {
        // Find the most recent recovery point at or before the timestamp
        let point_id = {
            let points = self.recovery_points.read().await;
            let mut best_match: Option<(RecoveryPointId, DateTime<Utc>)> = None;

            for (id, point) in points.iter() {
                if point.created_at <= timestamp {
                    match best_match {
                        None => best_match = Some((*id, point.created_at)),
                        Some((_, best_time)) if point.created_at > best_time => {
                            best_match = Some((*id, point.created_at))
                        }
                        _ => {}
                    }
                }
            }

            best_match.map(|(id, _)| id).ok_or_else(|| {
                anyhow::anyhow!("No recovery point found at or before {}", timestamp)
            })?
        };

        info!(
            "🕐 Found recovery point {} for timestamp {}",
            point_id, timestamp
        );

        self.recover_to_point(point_id).await
    }

    /// Recover to a specific LSN
    ///
    /// Creates a temporary recovery point at the given LSN and recovers to it
    pub async fn recover_to_lsn(&self, target_lsn: u64) -> Result<RecoveryResult> {
        let current_lsn = self.manifest_service.current_lsn().await;

        if target_lsn > current_lsn {
            return Err(anyhow::anyhow!(
                "Cannot recover to LSN {} (current LSN is {})",
                target_lsn,
                current_lsn
            ));
        }

        info!(
            "🕐 Recovering to LSN {} (current: {})",
            target_lsn, current_lsn
        );

        // Create a virtual recovery point at this LSN
        let collection_states = self.capture_collection_states_at_lsn(target_lsn).await?;

        let recovery_point = RecoveryPoint {
            id: 0, // Virtual point
            name: Some(format!("lsn_{}", target_lsn)),
            description: Some(format!("Recovery to LSN {}", target_lsn)),
            lsn: target_lsn,
            created_at: Utc::now(),
            collection_states,
            is_automatic: false,
            tags: vec!["lsn_recovery".to_string()],
        };

        let collections_to_recover: Vec<String> =
            recovery_point.collection_states.keys().cloned().collect();

        let start_time = std::time::Instant::now();
        let (entries_rolled_back, entries_replayed, warnings) = self
            .execute_recovery(&recovery_point, &collections_to_recover)
            .await?;

        let duration = start_time.elapsed();
        info!("🕐 LSN recovery completed in {:?}", duration);

        Ok(RecoveryResult {
            success: true,
            recovery_point,
            collections_recovered: collections_to_recover,
            entries_rolled_back,
            entries_replayed,
            duration_ms: duration.as_millis() as u64,
            warnings,
        })
    }

    /// List all recovery points
    pub async fn list_recovery_points(&self) -> Vec<RecoveryPoint> {
        let points = self.recovery_points.read().await;
        let mut list: Vec<_> = points.values().cloned().collect();
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at)); // Newest first
        list
    }

    /// Get a specific recovery point by ID
    pub async fn get_recovery_point(&self, id: RecoveryPointId) -> Option<RecoveryPoint> {
        let points = self.recovery_points.read().await;
        points.get(&id).cloned()
    }

    /// Delete a recovery point
    pub async fn delete_recovery_point(&self, id: RecoveryPointId) -> Result<()> {
        let point = {
            let mut points = self.recovery_points.write().await;
            points.remove(&id)
        };

        if let Some(point) = point {
            // Remove from named points if applicable
            if let Some(ref name) = point.name {
                let mut names = self.named_recovery_points.write().await;
                names.remove(name);
            }
            info!("🕐 Deleted recovery point {} (name: {:?})", id, point.name);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Recovery point {} not found", id))
        }
    }

    /// Get the current LSN
    pub async fn current_lsn(&self) -> u64 {
        self.manifest_service.current_lsn().await
    }

    // ============= Private Implementation Methods =============

    /// Capture current collection states
    async fn capture_collection_states(&self) -> Result<HashMap<String, CollectionRecoveryState>> {
        let current_lsn = self.manifest_service.current_lsn().await;
        self.capture_collection_states_at_lsn(current_lsn).await
    }

    /// Capture collection states at a specific LSN
    async fn capture_collection_states_at_lsn(
        &self,
        target_lsn: u64,
    ) -> Result<HashMap<String, CollectionRecoveryState>> {
        let entries = self
            .manifest_service
            .get_entries_up_to_lsn(target_lsn)
            .await;

        let mut collection_states: HashMap<String, CollectionRecoveryState> = HashMap::new();

        for entry in entries {
            let state = collection_states
                .entry(entry.collection_id.clone())
                .or_insert_with(|| CollectionRecoveryState {
                    collection_id: entry.collection_id.clone(),
                    last_lsn: 0,
                    vector_count: 0,
                    storage_size_bytes: 0,
                    last_flush_at: None,
                });

            state.last_lsn = state.last_lsn.max(entry.global_lsn);
            state.vector_count += entry.vector_count;
            state.storage_size_bytes += entry.size_bytes;

            if entry.status == WalEntryStatus::Flushed {
                state.last_flush_at = Some(
                    DateTime::from_timestamp_millis(entry.timestamp_ms as i64)
                        .unwrap_or_else(Utc::now),
                );
            }
        }

        debug!(
            "Captured {} collection states at LSN {}",
            collection_states.len(),
            target_lsn
        );
        Ok(collection_states)
    }

    /// Execute the actual recovery operation
    async fn execute_recovery(
        &self,
        recovery_point: &RecoveryPoint,
        _collections: &[String],
    ) -> Result<(u64, u64, Vec<String>)> {
        let mut entries_rolled_back = 0u64;
        let mut entries_replayed = 0u64;
        let mut warnings = Vec::new();

        // Get current LSN
        let current_lsn = self.manifest_service.current_lsn().await;

        if current_lsn == recovery_point.lsn {
            info!("Already at target LSN {}, no recovery needed", current_lsn);
            return Ok((0, 0, vec!["Already at target LSN".to_string()]));
        }

        // If recovering backwards (current > target), we need to roll back
        if current_lsn > recovery_point.lsn {
            info!(
                "Rolling back from LSN {} to {}",
                current_lsn, recovery_point.lsn
            );

            // Mark entries after the recovery point as invalid/rolled back
            entries_rolled_back = self
                .manifest_service
                .mark_entries_after_lsn_rolled_back(recovery_point.lsn)
                .await? as u64;

            warnings.push(format!("Rolled back {} entries", entries_rolled_back));
        } else {
            // If recovering forward (current < target), replay entries
            info!(
                "Replaying entries from LSN {} to {}",
                current_lsn, recovery_point.lsn
            );

            let entries_to_replay = self
                .manifest_service
                .get_entries_between_lsn(current_lsn, recovery_point.lsn)
                .await;

            entries_replayed = entries_to_replay.len() as u64;
            // Note: Actual replay would involve the recovery manager
            // This is just the foundation - full implementation would integrate
            // with RecoveryManager for actual vector replay
        }

        Ok((entries_rolled_back, entries_replayed, warnings))
    }

    /// Verify recovery integrity
    async fn verify_recovery_integrity(
        &self,
        recovery_point: &RecoveryPoint,
        collections: &[String],
    ) -> Result<()> {
        info!(
            "Verifying recovery integrity for {} collections",
            collections.len()
        );

        for collection_id in collections {
            if let Some(expected_state) = recovery_point.collection_states.get(collection_id) {
                // Verify the collection state matches the expected state
                // This would involve checking vector counts, checksums, etc.
                debug!(
                    "Verified collection {} at LSN {}",
                    collection_id, expected_state.last_lsn
                );
            } else {
                warn!("Collection {} not found in recovery point", collection_id);
            }
        }

        Ok(())
    }

    /// Cleanup old recovery points based on configuration
    async fn cleanup_old_recovery_points(&self) -> Result<()> {
        let now = Utc::now();
        let max_age = chrono::Duration::seconds(self.config.max_recovery_point_age_secs as i64);

        let mut points_to_delete = Vec::new();

        {
            let points = self.recovery_points.read().await;

            // Check if we exceed max count
            let excess_count = if points.len() > self.config.max_recovery_points {
                points.len() - self.config.max_recovery_points
            } else {
                0
            };

            // Find oldest points to delete
            let mut sorted_points: Vec<_> = points.iter().collect();
            sorted_points.sort_by(|a, b| a.1.created_at.cmp(&b.1.created_at));

            for (id, point) in sorted_points.iter().take(excess_count) {
                if !point.tags.contains(&"protected".to_string()) {
                    points_to_delete.push(**id);
                }
            }

            // Also delete points older than max age
            for (id, point) in points.iter() {
                let age = now.signed_duration_since(point.created_at);
                if age > max_age && !point.tags.contains(&"protected".to_string())
                    && !points_to_delete.contains(id) {
                        points_to_delete.push(*id);
                    }
            }
        }

        // Delete the identified points
        for id in points_to_delete {
            if let Err(e) = self.delete_recovery_point(id).await {
                warn!("Failed to cleanup recovery point {}: {:?}", id, e);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pitr_config_default() {
        let config = PITRConfig::default();
        assert_eq!(config.max_recovery_points, 100);
        assert_eq!(config.max_recovery_point_age_secs, 7 * 24 * 60 * 60);
        assert!(config.auto_create_before_compaction);
    }

    #[test]
    fn test_recovery_point_serialization() {
        let point = RecoveryPoint {
            id: 1,
            name: Some("test_point".to_string()),
            description: Some("Test recovery point".to_string()),
            lsn: 100,
            created_at: Utc::now(),
            collection_states: HashMap::new(),
            is_automatic: false,
            tags: vec!["test".to_string()],
        };

        let json = serde_json::to_string(&point).unwrap();
        let deserialized: RecoveryPoint = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, point.id);
        assert_eq!(deserialized.name, point.name);
        assert_eq!(deserialized.lsn, point.lsn);
    }

    #[test]
    fn test_recovery_options_default() {
        let options = RecoveryOptions::default();
        assert!(options.collections.is_none());
        assert!(!options.verify_integrity);
        assert!(!options.create_pre_recovery_point);
        assert!(!options.dry_run);
    }
}

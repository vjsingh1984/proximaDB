//! # Materialized View Refresh Strategies and Scheduling
//!
//! Provides refresh strategy definitions and scheduling for materialized views.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::definition::{MaterializedViewError, MaterializedViewId, MaterializedViewResult};

/// Refresh strategy for materialized views
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum RefreshStrategy {
    /// Refresh only when explicitly requested
    #[default]
    Manual,

    /// Refresh at fixed intervals
    Periodic {
        /// Interval between refreshes (in seconds)
        #[serde(
            serialize_with = "duration_to_secs",
            deserialize_with = "secs_to_duration"
        )]
        interval: Duration,
    },

    /// Refresh when underlying data changes
    OnChange {
        /// Debounce duration to batch rapid changes (in seconds)
        #[serde(
            serialize_with = "duration_to_secs",
            deserialize_with = "secs_to_duration"
        )]
        debounce: Duration,
    },
}

/// Serialize Duration as seconds
fn duration_to_secs<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_u64(duration.as_secs())
}

/// Deserialize Duration from seconds
fn secs_to_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let secs = u64::deserialize(deserializer)?;
    Ok(Duration::from_secs(secs))
}

impl RefreshStrategy {
    /// Create a periodic refresh strategy
    pub fn periodic(interval: Duration) -> Self {
        RefreshStrategy::Periodic { interval }
    }

    /// Create an on-change refresh strategy with debouncing
    pub fn on_change(debounce: Duration) -> Self {
        RefreshStrategy::OnChange { debounce }
    }

    /// Validate the refresh strategy
    pub fn validate(&self) -> MaterializedViewResult<()> {
        match self {
            RefreshStrategy::Manual => Ok(()),
            RefreshStrategy::Periodic { interval } => {
                if interval.as_secs() < 1 {
                    return Err(MaterializedViewError::InvalidRefreshStrategy(
                        "Periodic interval must be at least 1 second".to_string(),
                    ));
                }
                Ok(())
            }
            RefreshStrategy::OnChange { debounce } => {
                if debounce.as_millis() < 100 {
                    return Err(MaterializedViewError::InvalidRefreshStrategy(
                        "On-change debounce must be at least 100ms".to_string(),
                    ));
                }
                Ok(())
            }
        }
    }

    /// Check if this is a scheduled strategy (non-manual)
    pub fn is_scheduled(&self) -> bool {
        !matches!(self, RefreshStrategy::Manual)
    }

    /// Get the next refresh time from now
    pub fn next_refresh_from(&self, last_refresh: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
        match self {
            RefreshStrategy::Manual => None,
            RefreshStrategy::Periodic { interval } => {
                let base = last_refresh.unwrap_or_else(Utc::now);
                Some(
                    base + chrono::Duration::from_std(*interval)
                        .unwrap_or(chrono::Duration::hours(1)),
                )
            }
            RefreshStrategy::OnChange { .. } => None, // Triggered by events, not scheduled
        }
    }
}

impl std::fmt::Display for RefreshStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefreshStrategy::Manual => write!(f, "MANUAL"),
            RefreshStrategy::Periodic { interval } => {
                write!(f, "PERIODIC INTERVAL '{:?}'", interval)
            }
            RefreshStrategy::OnChange { debounce } => {
                write!(f, "ON CHANGE DEBOUNCE '{:?}'", debounce)
            }
        }
    }
}

/// Event type for refresh operations
#[derive(Debug, Clone)]
pub enum RefreshEventType {
    /// Manual refresh request
    Manual,
    /// Scheduled periodic refresh
    Scheduled,
    /// Triggered by data change
    DataChange {
        /// Collections that changed
        collections: Vec<String>,
    },
    /// Initial population after creation
    Initial,
}

/// Refresh event for a materialized view
#[derive(Debug, Clone)]
pub struct RefreshEvent {
    /// View name
    pub view_name: MaterializedViewId,
    /// Event type
    pub event_type: RefreshEventType,
    /// Timestamp when the event was created
    pub created_at: DateTime<Utc>,
    /// Priority (higher = more urgent)
    pub priority: u8,
}

impl RefreshEvent {
    /// Create a new refresh event
    pub fn new(view_name: impl Into<String>, event_type: RefreshEventType) -> Self {
        Self {
            view_name: view_name.into(),
            event_type,
            created_at: Utc::now(),
            priority: 0,
        }
    }

    /// Create a manual refresh event
    pub fn manual(view_name: impl Into<String>) -> Self {
        Self::new(view_name, RefreshEventType::Manual)
    }

    /// Create a scheduled refresh event
    pub fn scheduled(view_name: impl Into<String>) -> Self {
        Self::new(view_name, RefreshEventType::Scheduled)
    }

    /// Create a data change refresh event
    pub fn data_change(view_name: impl Into<String>, collections: Vec<String>) -> Self {
        Self::new(view_name, RefreshEventType::DataChange { collections })
    }

    /// Create an initial refresh event
    pub fn initial(view_name: impl Into<String>) -> Self {
        Self::new(view_name, RefreshEventType::Initial)
    }

    /// Set priority
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }
}

/// Result of a refresh operation
#[derive(Debug)]
pub struct RefreshResult {
    /// View name
    pub view_name: MaterializedViewId,
    /// Whether the refresh succeeded
    pub success: bool,
    /// Duration of the refresh
    pub duration: Duration,
    /// Number of rows in the result
    pub row_count: usize,
    /// Error message if failed
    pub error: Option<String>,
    /// Timestamp when the refresh completed
    pub completed_at: DateTime<Utc>,
}

impl RefreshResult {
    /// Create a successful result
    pub fn success(view_name: impl Into<String>, duration: Duration, row_count: usize) -> Self {
        Self {
            view_name: view_name.into(),
            success: true,
            duration,
            row_count,
            error: None,
            completed_at: Utc::now(),
        }
    }

    /// Create a failed result
    pub fn failure(
        view_name: impl Into<String>,
        duration: Duration,
        error: impl Into<String>,
    ) -> Self {
        Self {
            view_name: view_name.into(),
            success: false,
            duration,
            row_count: 0,
            error: Some(error.into()),
            completed_at: Utc::now(),
        }
    }
}

/// Context for refresh operations
pub struct RefreshContext {
    /// Maximum concurrent refreshes
    pub max_concurrent: usize,
    /// Default timeout for refresh operations
    pub timeout: Duration,
    /// Whether to retry on failure
    pub retry_on_failure: bool,
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Retry delay
    pub retry_delay: Duration,
}

impl Default for RefreshContext {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            timeout: Duration::from_secs(300),
            retry_on_failure: true,
            max_retries: 3,
            retry_delay: Duration::from_secs(5),
        }
    }
}

/// Scheduler for periodic and on-change refreshes
pub struct RefreshScheduler {
    /// Scheduled views with their next refresh times
    scheduled_views: DashMap<MaterializedViewId, ScheduledView>,
    /// Pending data change events (for debouncing)
    pending_changes: DashMap<MaterializedViewId, PendingChange>,
    /// Event sender for refresh events
    event_tx: mpsc::Sender<RefreshEvent>,
    /// Whether the scheduler is running
    #[allow(dead_code)]
    is_running: AtomicBool,
    /// Statistics
    stats: MaterializedViewSchedulerStats,
}

/// A scheduled view entry
struct ScheduledView {
    /// View name
    view_name: MaterializedViewId,
    /// Refresh strategy
    strategy: RefreshStrategy,
    /// Next scheduled refresh time
    next_refresh: Option<DateTime<Utc>>,
    /// Last refresh time
    last_refresh: Option<DateTime<Utc>>,
    /// Dependencies (collections this view depends on)
    dependencies: Vec<String>,
}

/// Pending change for debouncing
struct PendingChange {
    /// Collections that changed
    collections: Vec<String>,
    /// When the first change was detected
    first_change_at: Instant,
    /// Debounce duration
    debounce: Duration,
}

/// Backwards-compat alias for [`MaterializedViewSchedulerStats`].
pub type SchedulerStats = MaterializedViewSchedulerStats;

/// Scheduler statistics
#[derive(Debug, Default)]
pub struct MaterializedViewSchedulerStats {
    /// Total scheduled refreshes triggered
    pub scheduled_triggers: AtomicU64,
    /// Total data change triggers
    pub change_triggers: AtomicU64,
    /// Total refreshes completed
    pub refreshes_completed: AtomicU64,
    /// Total refreshes failed
    pub refreshes_failed: AtomicU64,
}

impl RefreshScheduler {
    /// Create a new refresh scheduler
    pub fn new() -> (Self, mpsc::Receiver<RefreshEvent>) {
        let (tx, rx) = mpsc::channel(1000);
        (
            Self {
                scheduled_views: DashMap::new(),
                pending_changes: DashMap::new(),
                event_tx: tx,
                is_running: AtomicBool::new(false),
                stats: MaterializedViewSchedulerStats::default(),
            },
            rx,
        )
    }

    /// Register a view for scheduling
    pub fn register(
        &self,
        view_name: impl Into<String>,
        strategy: RefreshStrategy,
        dependencies: Vec<String>,
    ) {
        let view_name = view_name.into();

        if !strategy.is_scheduled() {
            debug!(view = %view_name, "View has manual refresh, not scheduling");
            return;
        }

        let next_refresh = strategy.next_refresh_from(None);

        self.scheduled_views.insert(
            view_name.clone(),
            ScheduledView {
                view_name: view_name.clone(),
                strategy,
                next_refresh,
                last_refresh: None,
                dependencies,
            },
        );

        info!(view = %view_name, "Registered view for scheduled refresh");
    }

    /// Unregister a view from scheduling
    pub fn unregister(&self, view_name: &str) {
        self.scheduled_views.remove(view_name);
        self.pending_changes.remove(view_name);
        debug!(view = %view_name, "Unregistered view from scheduling");
    }

    /// Update the schedule for a view after a successful refresh
    pub fn update_after_refresh(&self, view_name: &str) {
        if let Some(mut entry) = self.scheduled_views.get_mut(view_name) {
            let now = Utc::now();
            entry.last_refresh = Some(now);
            entry.next_refresh = entry.strategy.next_refresh_from(Some(now));

            debug!(
                view = %view_name,
                next_refresh = ?entry.next_refresh,
                "Updated schedule after refresh"
            );
        }
    }

    /// Notify of a data change in a collection
    pub async fn notify_change(&self, collection: &str) -> MaterializedViewResult<()> {
        let mut affected_views = Vec::new();

        // Find views that depend on this collection
        for entry in &self.scheduled_views {
            if entry.dependencies.contains(&collection.to_string())
                && let RefreshStrategy::OnChange { debounce } = &entry.strategy
            {
                affected_views.push((entry.view_name.clone(), *debounce));
            }
        }

        for (view_name, debounce) in affected_views {
            self.handle_change_event(&view_name, collection, debounce)
                .await?;
        }

        Ok(())
    }

    /// Handle a change event for a view with debouncing
    async fn handle_change_event(
        &self,
        view_name: &str,
        collection: &str,
        debounce: Duration,
    ) -> MaterializedViewResult<()> {
        let now = Instant::now();

        if let Some(mut pending) = self.pending_changes.get_mut(view_name) {
            // Add collection to existing pending change
            if !pending.collections.contains(&collection.to_string()) {
                pending.collections.push(collection.to_string());
            }

            // Check if debounce period has elapsed
            if pending.first_change_at.elapsed() >= pending.debounce {
                let collections = pending.collections.clone();
                drop(pending);
                self.pending_changes.remove(view_name);

                // Send refresh event
                let event = RefreshEvent::data_change(view_name, collections);
                self.event_tx
                    .send(event)
                    .await
                    .map_err(|e| MaterializedViewError::Internal(e.to_string()))?;

                self.stats.change_triggers.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            // Create new pending change
            self.pending_changes.insert(
                view_name.to_string(),
                PendingChange {
                    collections: vec![collection.to_string()],
                    first_change_at: now,
                    debounce,
                },
            );
        }

        Ok(())
    }

    /// Check for scheduled refreshes and emit events
    pub async fn check_scheduled(&self) -> MaterializedViewResult<()> {
        let now = Utc::now();

        for entry in &self.scheduled_views {
            if let Some(next_refresh) = entry.next_refresh
                && now >= next_refresh
            {
                let event = RefreshEvent::scheduled(&entry.view_name);
                if let Err(e) = self.event_tx.send(event).await {
                    warn!(
                        view = %entry.view_name,
                        error = %e,
                        "Failed to send scheduled refresh event"
                    );
                } else {
                    self.stats
                        .scheduled_triggers
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        Ok(())
    }

    /// Flush pending debounced changes
    pub async fn flush_pending_changes(&self) -> MaterializedViewResult<()> {
        let _now = Instant::now();
        let mut to_flush = Vec::new();

        for entry in &self.pending_changes {
            if entry.first_change_at.elapsed() >= entry.debounce {
                to_flush.push((entry.key().clone(), entry.collections.clone()));
            }
        }

        for (view_name, collections) in to_flush {
            self.pending_changes.remove(&view_name);
            let event = RefreshEvent::data_change(&view_name, collections);
            self.event_tx
                .send(event)
                .await
                .map_err(|e| MaterializedViewError::Internal(e.to_string()))?;
            self.stats.change_triggers.fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Get scheduler statistics
    pub fn stats(&self) -> &MaterializedViewSchedulerStats {
        &self.stats
    }

    /// Get the number of scheduled views
    pub fn scheduled_count(&self) -> usize {
        self.scheduled_views.len()
    }

    /// Get the number of pending changes
    pub fn pending_count(&self) -> usize {
        self.pending_changes.len()
    }

    /// Check if a view is scheduled
    pub fn is_scheduled(&self, view_name: &str) -> bool {
        self.scheduled_views.contains_key(view_name)
    }

    /// Get the next refresh time for a view
    pub fn next_refresh_time(&self, view_name: &str) -> Option<DateTime<Utc>> {
        self.scheduled_views
            .get(view_name)
            .and_then(|v| v.next_refresh)
    }

    /// List all scheduled views
    pub fn list_scheduled(
        &self,
    ) -> Vec<(MaterializedViewId, RefreshStrategy, Option<DateTime<Utc>>)> {
        self.scheduled_views
            .iter()
            .map(|e| (e.view_name.clone(), e.strategy.clone(), e.next_refresh))
            .collect()
    }
}

impl Default for RefreshScheduler {
    fn default() -> Self {
        Self::new().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_strategy_manual() {
        let strategy = RefreshStrategy::Manual;
        assert!(!strategy.is_scheduled());
        assert!(strategy.validate().is_ok());
        assert_eq!(strategy.to_string(), "MANUAL");
    }

    #[test]
    fn test_refresh_strategy_periodic() {
        let strategy = RefreshStrategy::periodic(Duration::from_secs(3600));
        assert!(strategy.is_scheduled());
        assert!(strategy.validate().is_ok());
        assert!(strategy.to_string().contains("PERIODIC"));
    }

    #[test]
    fn test_refresh_strategy_on_change() {
        let strategy = RefreshStrategy::on_change(Duration::from_secs(5));
        assert!(strategy.is_scheduled());
        assert!(strategy.validate().is_ok());
        assert!(strategy.to_string().contains("ON CHANGE"));
    }

    #[test]
    fn test_refresh_strategy_validation() {
        // Too short periodic interval
        let short_periodic = RefreshStrategy::Periodic {
            interval: Duration::from_millis(100),
        };
        assert!(short_periodic.validate().is_err());

        // Too short debounce
        let short_debounce = RefreshStrategy::OnChange {
            debounce: Duration::from_millis(50),
        };
        assert!(short_debounce.validate().is_err());
    }

    #[test]
    fn test_refresh_strategy_next_refresh() {
        let manual = RefreshStrategy::Manual;
        assert!(manual.next_refresh_from(None).is_none());

        let periodic = RefreshStrategy::periodic(Duration::from_secs(3600));
        let now = Utc::now();
        let next = periodic.next_refresh_from(Some(now));
        assert!(next.is_some());
        assert!(next.unwrap() > now);

        let on_change = RefreshStrategy::on_change(Duration::from_secs(5));
        assert!(on_change.next_refresh_from(None).is_none());
    }

    #[test]
    fn test_refresh_event_creation() {
        let manual = RefreshEvent::manual("test_view");
        assert_eq!(manual.view_name, "test_view");
        assert!(matches!(manual.event_type, RefreshEventType::Manual));

        let scheduled = RefreshEvent::scheduled("test_view");
        assert!(matches!(scheduled.event_type, RefreshEventType::Scheduled));

        let data_change = RefreshEvent::data_change("test_view", vec!["users".to_string()]);
        assert!(matches!(
            data_change.event_type,
            RefreshEventType::DataChange { .. }
        ));

        let initial = RefreshEvent::initial("test_view");
        assert!(matches!(initial.event_type, RefreshEventType::Initial));
    }

    #[test]
    fn test_refresh_result() {
        let success = RefreshResult::success("test", Duration::from_millis(100), 1000);
        assert!(success.success);
        assert_eq!(success.row_count, 1000);
        assert!(success.error.is_none());

        let failure =
            RefreshResult::failure("test", Duration::from_millis(50), "Connection failed");
        assert!(!failure.success);
        assert_eq!(failure.row_count, 0);
        assert!(failure.error.is_some());
    }

    #[test]
    fn test_refresh_context_default() {
        let ctx = RefreshContext::default();
        assert_eq!(ctx.max_concurrent, 4);
        assert_eq!(ctx.timeout, Duration::from_secs(300));
        assert!(ctx.retry_on_failure);
        assert_eq!(ctx.max_retries, 3);
    }

    #[tokio::test]
    async fn test_scheduler_register_unregister() {
        let (scheduler, _rx) = RefreshScheduler::new();

        // Manual strategy should not be scheduled
        scheduler.register("manual_view", RefreshStrategy::Manual, vec![]);
        assert!(!scheduler.is_scheduled("manual_view"));

        // Periodic strategy should be scheduled
        scheduler.register(
            "periodic_view",
            RefreshStrategy::periodic(Duration::from_secs(3600)),
            vec!["users".to_string()],
        );
        assert!(scheduler.is_scheduled("periodic_view"));
        assert_eq!(scheduler.scheduled_count(), 1);

        // Unregister
        scheduler.unregister("periodic_view");
        assert!(!scheduler.is_scheduled("periodic_view"));
        assert_eq!(scheduler.scheduled_count(), 0);
    }

    #[tokio::test]
    async fn test_scheduler_list_scheduled() {
        let (scheduler, _rx) = RefreshScheduler::new();

        scheduler.register(
            "view1",
            RefreshStrategy::periodic(Duration::from_secs(3600)),
            vec![],
        );
        scheduler.register(
            "view2",
            RefreshStrategy::on_change(Duration::from_secs(5)),
            vec!["products".to_string()],
        );

        let scheduled = scheduler.list_scheduled();
        assert_eq!(scheduled.len(), 2);
    }

    #[tokio::test]
    async fn test_scheduler_update_after_refresh() {
        let (scheduler, _rx) = RefreshScheduler::new();

        scheduler.register(
            "test_view",
            RefreshStrategy::periodic(Duration::from_secs(3600)),
            vec![],
        );

        let before = scheduler.next_refresh_time("test_view");
        assert!(before.is_some());

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        scheduler.update_after_refresh("test_view");

        let after = scheduler.next_refresh_time("test_view");
        assert!(after.is_some());
        // After refresh, the next refresh time should be later
        assert!(after > before);
    }
}

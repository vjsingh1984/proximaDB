//! # Cache Invalidation Logic
//!
//! Provides cache invalidation mechanisms that hook into WAL/CDC for real-time
//! invalidation when data is modified.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                   Cache Invalidation Flow                    │
//! │                                                              │
//! │  Write Operation (INSERT/UPDATE/DELETE)                      │
//! │           ↓                                                  │
//! │  WAL / CDC Event                                            │
//! │           ↓                                                  │
//! │  CacheInvalidator.on_change_event()                         │
//! │           ↓                                                  │
//! │  Extract affected collections                                │
//! │           ↓                                                  │
//! │  QueryResultCache.invalidate_collection()                   │
//! │           ↓                                                  │
//! │  Affected queries removed from cache                        │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::query::cache::{QueryResultCache, CacheInvalidator};
//!
//! // Create cache and invalidator
//! let cache = Arc::new(QueryResultCache::with_defaults());
//! let invalidator = CacheInvalidator::new(cache.clone());
//!
//! // Hook into your write path
//! on_collection_write("products", |collection, op| {
//!     invalidator.invalidate_collection(collection);
//! });
//! ```

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use tracing::{debug, info};

use super::query_result_cache::QueryResultCache;

/// Types of change operations that trigger invalidation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOperation {
    /// Insert operation
    Insert,
    /// Update operation
    Update,
    /// Delete operation
    Delete,
    /// Truncate/clear collection
    Truncate,
    /// Schema change
    SchemaChange,
    /// Unknown or other operation
    Other,
}

impl ChangeOperation {
    /// Check if this operation modifies data
    pub fn modifies_data(&self) -> bool {
        matches!(
            self,
            ChangeOperation::Insert
                | ChangeOperation::Update
                | ChangeOperation::Delete
                | ChangeOperation::Truncate
        )
    }

    /// Check if this operation requires full cache invalidation
    pub fn requires_full_invalidation(&self) -> bool {
        matches!(
            self,
            ChangeOperation::Truncate | ChangeOperation::SchemaChange
        )
    }
}

/// A change event that can trigger cache invalidation
#[derive(Debug, Clone)]
pub struct InvalidationEvent {
    /// The collection that was modified
    pub collection: String,
    /// The type of operation
    pub operation: ChangeOperation,
    /// Optional record key that was affected
    pub record_key: Option<String>,
    /// Logical sequence number (if available)
    pub lsn: Option<u64>,
    /// Transaction ID (if available)
    pub transaction_id: Option<String>,
}

impl InvalidationEvent {
    /// Create a new invalidation event
    pub fn new(collection: impl Into<String>, operation: ChangeOperation) -> Self {
        Self {
            collection: collection.into(),
            operation,
            record_key: None,
            lsn: None,
            transaction_id: None,
        }
    }

    /// Add record key
    pub fn with_record_key(mut self, key: impl Into<String>) -> Self {
        self.record_key = Some(key.into());
        self
    }

    /// Add LSN
    pub fn with_lsn(mut self, lsn: u64) -> Self {
        self.lsn = Some(lsn);
        self
    }

    /// Add transaction ID
    pub fn with_transaction_id(mut self, txn_id: impl Into<String>) -> Self {
        self.transaction_id = Some(txn_id.into());
        self
    }
}

/// Configuration for cache invalidation behavior
#[derive(Debug, Clone)]
pub struct InvalidationConfig {
    /// Enable batching of invalidation events (default: true)
    pub batch_invalidations: bool,
    /// Maximum batch size before forcing flush (default: 100)
    pub max_batch_size: usize,
    /// Maximum batch delay before forcing flush (default: 10ms)
    pub max_batch_delay: Duration,
    /// Enable transaction-aware invalidation (default: true)
    pub transaction_aware: bool,
    /// Log invalidation events at debug level (default: true)
    pub log_events: bool,
}

impl Default for InvalidationConfig {
    fn default() -> Self {
        Self {
            batch_invalidations: true,
            max_batch_size: 100,
            max_batch_delay: Duration::from_millis(10),
            transaction_aware: true,
            log_events: true,
        }
    }
}

/// Statistics for invalidation operations
#[derive(Debug, Default)]
pub struct InvalidationStats {
    /// Total invalidation events received
    pub events_received: AtomicU64,
    /// Total collections invalidated
    pub collections_invalidated: AtomicU64,
    /// Total cache entries invalidated
    pub entries_invalidated: AtomicU64,
    /// Events skipped (filtered out)
    pub events_skipped: AtomicU64,
    /// Batch flushes performed
    pub batch_flushes: AtomicU64,
}

impl InvalidationStats {
    /// Get current statistics as a snapshot
    pub fn snapshot(&self) -> InvalidationStatsSnapshot {
        InvalidationStatsSnapshot {
            events_received: self.events_received.load(Ordering::Relaxed),
            collections_invalidated: self.collections_invalidated.load(Ordering::Relaxed),
            entries_invalidated: self.entries_invalidated.load(Ordering::Relaxed),
            events_skipped: self.events_skipped.load(Ordering::Relaxed),
            batch_flushes: self.batch_flushes.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of invalidation statistics
#[derive(Debug, Clone)]
pub struct InvalidationStatsSnapshot {
    /// Total invalidation events received
    pub events_received: u64,
    /// Total collections invalidated
    pub collections_invalidated: u64,
    /// Total cache entries invalidated
    pub entries_invalidated: u64,
    /// Events skipped (filtered out)
    pub events_skipped: u64,
    /// Batch flushes performed
    pub batch_flushes: u64,
}

/// Cache invalidator that processes change events and invalidates cache entries
///
/// The invalidator can be used in two modes:
/// 1. Direct mode: Call `invalidate_*` methods directly from write paths
/// 2. Event-driven mode: Subscribe to CDC/WAL events and call `on_change_event`
pub struct CacheInvalidator {
    /// Reference to the query result cache
    cache: Arc<QueryResultCache>,
    /// Configuration
    config: InvalidationConfig,
    /// Collections to watch (empty = watch all)
    watched_collections: DashMap<String, bool>,
    /// Pending invalidations for batching
    pending_batch: DashMap<String, HashSet<Option<String>>>,
    /// Statistics
    stats: InvalidationStats,
    /// Pending transactions (for transaction-aware invalidation)
    pending_transactions: DashMap<String, Vec<InvalidationEvent>>,
}

impl CacheInvalidator {
    /// Create a new cache invalidator
    pub fn new(cache: Arc<QueryResultCache>) -> Self {
        Self::with_config(cache, InvalidationConfig::default())
    }

    /// Create a new cache invalidator with custom configuration
    pub fn with_config(cache: Arc<QueryResultCache>, config: InvalidationConfig) -> Self {
        Self {
            cache,
            config,
            watched_collections: DashMap::new(),
            pending_batch: DashMap::new(),
            stats: InvalidationStats::default(),
            pending_transactions: DashMap::new(),
        }
    }

    /// Add a collection to watch for invalidation
    ///
    /// If no collections are added, all collections are watched.
    pub fn watch_collection(&self, collection: impl Into<String>) {
        self.watched_collections.insert(collection.into(), true);
    }

    /// Remove a collection from watch list
    pub fn unwatch_collection(&self, collection: &str) {
        self.watched_collections.remove(collection);
    }

    /// Check if a collection is being watched
    fn is_watched(&self, collection: &str) -> bool {
        // If no specific collections are watched, watch all
        if self.watched_collections.is_empty() {
            return true;
        }
        self.watched_collections.contains_key(collection)
    }

    /// Process a change event and invalidate affected cache entries
    ///
    /// This is the main entry point for event-driven invalidation.
    pub fn on_change_event(&self, event: InvalidationEvent) -> usize {
        self.stats.events_received.fetch_add(1, Ordering::Relaxed);

        // Check if collection is watched
        if !self.is_watched(&event.collection) {
            self.stats.events_skipped.fetch_add(1, Ordering::Relaxed);
            return 0;
        }

        // Handle transaction-aware invalidation
        if self.config.transaction_aware {
            if let Some(ref txn_id) = event.transaction_id {
                // Defer invalidation until transaction commits
                self.pending_transactions
                    .entry(txn_id.clone())
                    .or_insert_with(Vec::new)
                    .push(event);
                return 0;
            }
        }

        // Check if batching is enabled
        if self.config.batch_invalidations {
            self.add_to_batch(&event);
            // Batch will be flushed when max size is reached or on explicit flush
            if self.should_flush_batch() {
                return self.flush_batch();
            }
            return 0;
        }

        // Direct invalidation
        self.invalidate_for_event(&event)
    }

    /// Invalidate cache entries for a specific collection
    ///
    /// This is a direct invalidation method for use in write paths.
    pub fn invalidate_collection(&self, collection: &str) -> usize {
        let invalidated = self.cache.invalidate_collection(collection);

        self.stats
            .collections_invalidated
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .entries_invalidated
            .fetch_add(invalidated as u64, Ordering::Relaxed);

        if self.config.log_events && invalidated > 0 {
            debug!(collection, invalidated, "Cache invalidation for collection");
        }

        invalidated
    }

    /// Invalidate cache entries for multiple collections
    pub fn invalidate_collections(&self, collections: &[&str]) -> usize {
        let mut total = 0;
        for collection in collections {
            total += self.invalidate_collection(collection);
        }
        total
    }

    /// Notify that a transaction has committed
    ///
    /// This triggers invalidation for all events in the transaction.
    pub fn on_transaction_commit(&self, transaction_id: &str) -> usize {
        if let Some((_, events)) = self.pending_transactions.remove(transaction_id) {
            let mut total = 0;
            for event in events {
                total += self.invalidate_for_event(&event);
            }
            if self.config.log_events && total > 0 {
                info!(
                    transaction_id,
                    invalidated = total,
                    "Cache invalidation for committed transaction"
                );
            }
            total
        } else {
            0
        }
    }

    /// Notify that a transaction has rolled back
    ///
    /// This discards all pending invalidation events for the transaction.
    pub fn on_transaction_rollback(&self, transaction_id: &str) {
        if let Some((_, events)) = self.pending_transactions.remove(transaction_id) {
            debug!(
                transaction_id,
                discarded = events.len(),
                "Discarded invalidation events for rolled back transaction"
            );
        }
    }

    /// Flush any pending batch invalidations
    pub fn flush_batch(&self) -> usize {
        let collections: Vec<String> = self
            .pending_batch
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        if collections.is_empty() {
            return 0;
        }

        self.pending_batch.clear();

        let mut total = 0;
        for collection in &collections {
            total += self.cache.invalidate_collection(collection);
        }

        self.stats.batch_flushes.fetch_add(1, Ordering::Relaxed);
        self.stats
            .collections_invalidated
            .fetch_add(collections.len() as u64, Ordering::Relaxed);
        self.stats
            .entries_invalidated
            .fetch_add(total as u64, Ordering::Relaxed);

        if self.config.log_events && total > 0 {
            debug!(
                collections = collections.len(),
                invalidated = total,
                "Flushed batch invalidation"
            );
        }

        total
    }

    /// Check if batch should be flushed
    fn should_flush_batch(&self) -> bool {
        self.pending_batch.len() >= self.config.max_batch_size
    }

    /// Add an event to the pending batch
    fn add_to_batch(&self, event: &InvalidationEvent) {
        self.pending_batch
            .entry(event.collection.clone())
            .or_insert_with(HashSet::new)
            .insert(event.record_key.clone());
    }

    /// Internal method to invalidate for a single event
    fn invalidate_for_event(&self, event: &InvalidationEvent) -> usize {
        // For full invalidation operations, clear everything for the collection
        if event.operation.requires_full_invalidation() {
            return self.invalidate_collection(&event.collection);
        }

        // For normal data modifications, invalidate the collection
        if event.operation.modifies_data() {
            return self.invalidate_collection(&event.collection);
        }

        0
    }

    /// Get current statistics
    pub fn stats(&self) -> InvalidationStatsSnapshot {
        self.stats.snapshot()
    }

    /// Clear all pending state
    pub fn clear(&self) {
        self.pending_batch.clear();
        self.pending_transactions.clear();
    }

    /// Get the underlying cache reference
    pub fn cache(&self) -> &Arc<QueryResultCache> {
        &self.cache
    }
}

/// A listener trait for cache invalidation events
///
/// Implement this trait to receive notifications when cache entries are invalidated.
pub trait InvalidationListener: Send + Sync {
    /// Called when cache entries are invalidated for a collection
    fn on_invalidation(&self, collection: &str, entries_invalidated: usize);

    /// Called when a batch of invalidations is flushed
    fn on_batch_flush(&self, collections: &[String], total_invalidated: usize);
}

/// A broadcast invalidator that notifies multiple listeners
pub struct BroadcastInvalidator {
    /// The underlying invalidator
    invalidator: Arc<CacheInvalidator>,
    /// Registered listeners
    listeners: DashMap<String, Arc<dyn InvalidationListener>>,
}

impl BroadcastInvalidator {
    /// Create a new broadcast invalidator
    pub fn new(invalidator: Arc<CacheInvalidator>) -> Self {
        Self {
            invalidator,
            listeners: DashMap::new(),
        }
    }

    /// Register a listener
    pub fn register(&self, id: impl Into<String>, listener: Arc<dyn InvalidationListener>) {
        self.listeners.insert(id.into(), listener);
    }

    /// Unregister a listener
    pub fn unregister(&self, id: &str) {
        self.listeners.remove(id);
    }

    /// Process a change event and notify listeners
    pub fn on_change_event(&self, event: InvalidationEvent) -> usize {
        let collection = event.collection.clone();
        let invalidated = self.invalidator.on_change_event(event);

        if invalidated > 0 {
            for entry in self.listeners.iter() {
                entry.value().on_invalidation(&collection, invalidated);
            }
        }

        invalidated
    }

    /// Flush batch and notify listeners
    pub fn flush_batch(&self) -> usize {
        // Collect collections before flushing
        let collections: Vec<String> = self
            .invalidator
            .pending_batch
            .iter()
            .map(|e| e.key().clone())
            .collect();

        let total = self.invalidator.flush_batch();

        if total > 0 && !collections.is_empty() {
            for entry in self.listeners.iter() {
                entry.value().on_batch_flush(&collections, total);
            }
        }

        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_cache() -> Arc<QueryResultCache> {
        use super::super::query_result_cache::{QueryKey, QueryResultCacheConfig};
        use crate::query::federated::ExecutionResult;
        use arrow::array::{ArrayRef, RecordBatch, StringArray};
        use arrow::datatypes::{DataType, Field, Schema};

        let config = QueryResultCacheConfig::default();
        let cache = Arc::new(QueryResultCache::new(config));

        // Pre-populate with some entries
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, false)]));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(StringArray::from(vec!["1"])) as ArrayRef],
        )
        .unwrap();

        let result = ExecutionResult::from_batch(batch);

        // Add entries for different collections
        for collection in &["products", "users", "orders"] {
            let key = QueryKey::from_sql(&format!("SELECT * FROM {}", collection));
            cache
                .insert(key, result.clone(), vec![collection.to_string()])
                .unwrap();
        }

        cache
    }

    #[test]
    fn test_invalidator_creation() {
        let cache = create_test_cache();
        let invalidator = CacheInvalidator::new(cache.clone());

        assert_eq!(cache.len(), 3);
        assert!(invalidator.watched_collections.is_empty());
    }

    #[test]
    fn test_direct_invalidation() {
        let cache = create_test_cache();
        let invalidator = CacheInvalidator::new(cache.clone());

        assert_eq!(cache.len(), 3);

        let invalidated = invalidator.invalidate_collection("products");
        assert_eq!(invalidated, 1);
        assert_eq!(cache.len(), 2);

        let stats = invalidator.stats();
        assert_eq!(stats.collections_invalidated, 1);
        assert_eq!(stats.entries_invalidated, 1);
    }

    #[test]
    fn test_event_based_invalidation() {
        let cache = create_test_cache();
        let config = InvalidationConfig {
            batch_invalidations: false, // Disable batching for immediate invalidation
            ..Default::default()
        };
        let invalidator = CacheInvalidator::with_config(cache.clone(), config);

        assert_eq!(cache.len(), 3);

        let event = InvalidationEvent::new("users", ChangeOperation::Update);
        let invalidated = invalidator.on_change_event(event);

        assert_eq!(invalidated, 1);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_watched_collections() {
        let cache = create_test_cache();
        let config = InvalidationConfig {
            batch_invalidations: false,
            ..Default::default()
        };
        let invalidator = CacheInvalidator::with_config(cache.clone(), config);

        // Watch only products
        invalidator.watch_collection("products");

        // Event for watched collection should invalidate
        let event1 = InvalidationEvent::new("products", ChangeOperation::Update);
        let invalidated1 = invalidator.on_change_event(event1);
        assert_eq!(invalidated1, 1);

        // Event for unwatched collection should be skipped
        let event2 = InvalidationEvent::new("users", ChangeOperation::Update);
        let invalidated2 = invalidator.on_change_event(event2);
        assert_eq!(invalidated2, 0);

        let stats = invalidator.stats();
        assert_eq!(stats.events_skipped, 1);
    }

    #[test]
    fn test_batch_invalidation() {
        let cache = create_test_cache();
        let config = InvalidationConfig {
            batch_invalidations: true,
            max_batch_size: 5, // Low threshold for testing
            ..Default::default()
        };
        let invalidator = CacheInvalidator::with_config(cache.clone(), config);

        // Add events without triggering flush
        invalidator.on_change_event(InvalidationEvent::new("products", ChangeOperation::Insert));
        invalidator.on_change_event(InvalidationEvent::new("users", ChangeOperation::Insert));

        // Cache should still have all entries (not flushed yet)
        assert_eq!(cache.len(), 3);

        // Manual flush
        let flushed = invalidator.flush_batch();
        assert_eq!(flushed, 2); // products and users
        assert_eq!(cache.len(), 1); // Only orders remains
    }

    #[test]
    fn test_transaction_aware_invalidation() {
        let cache = create_test_cache();
        let config = InvalidationConfig {
            batch_invalidations: false,
            transaction_aware: true,
            ..Default::default()
        };
        let invalidator = CacheInvalidator::with_config(cache.clone(), config);

        // Event with transaction ID should be deferred
        let event = InvalidationEvent::new("products", ChangeOperation::Update)
            .with_transaction_id("txn_123");
        let invalidated = invalidator.on_change_event(event);
        assert_eq!(invalidated, 0); // Deferred
        assert_eq!(cache.len(), 3); // No change yet

        // Commit transaction triggers invalidation
        let committed = invalidator.on_transaction_commit("txn_123");
        assert_eq!(committed, 1);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn test_transaction_rollback() {
        let cache = create_test_cache();
        let config = InvalidationConfig {
            batch_invalidations: false,
            transaction_aware: true,
            ..Default::default()
        };
        let invalidator = CacheInvalidator::with_config(cache.clone(), config);

        // Event with transaction ID
        let event = InvalidationEvent::new("products", ChangeOperation::Update)
            .with_transaction_id("txn_456");
        invalidator.on_change_event(event);

        // Rollback discards pending invalidations
        invalidator.on_transaction_rollback("txn_456");

        // Cache should be unchanged
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn test_change_operations() {
        assert!(ChangeOperation::Insert.modifies_data());
        assert!(ChangeOperation::Update.modifies_data());
        assert!(ChangeOperation::Delete.modifies_data());
        assert!(ChangeOperation::Truncate.modifies_data());
        assert!(!ChangeOperation::Other.modifies_data());

        assert!(ChangeOperation::Truncate.requires_full_invalidation());
        assert!(ChangeOperation::SchemaChange.requires_full_invalidation());
        assert!(!ChangeOperation::Insert.requires_full_invalidation());
    }

    #[test]
    fn test_invalidation_event_builder() {
        let event = InvalidationEvent::new("products", ChangeOperation::Insert)
            .with_record_key("prod_123")
            .with_lsn(42)
            .with_transaction_id("txn_789");

        assert_eq!(event.collection, "products");
        assert_eq!(event.operation, ChangeOperation::Insert);
        assert_eq!(event.record_key, Some("prod_123".to_string()));
        assert_eq!(event.lsn, Some(42));
        assert_eq!(event.transaction_id, Some("txn_789".to_string()));
    }

    #[test]
    fn test_multiple_collections_invalidation() {
        let cache = create_test_cache();
        let invalidator = CacheInvalidator::new(cache.clone());

        let total = invalidator.invalidate_collections(&["products", "users"]);
        assert_eq!(total, 2);
        assert_eq!(cache.len(), 1);
    }

    // Test for BroadcastInvalidator
    struct TestListener {
        call_count: AtomicU64,
    }

    impl TestListener {
        fn new() -> Self {
            Self {
                call_count: AtomicU64::new(0),
            }
        }

        fn call_count(&self) -> u64 {
            self.call_count.load(Ordering::Relaxed)
        }
    }

    impl InvalidationListener for TestListener {
        fn on_invalidation(&self, _collection: &str, _entries_invalidated: usize) {
            self.call_count.fetch_add(1, Ordering::Relaxed);
        }

        fn on_batch_flush(&self, _collections: &[String], _total_invalidated: usize) {
            self.call_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn test_broadcast_invalidator() {
        let cache = create_test_cache();
        let config = InvalidationConfig {
            batch_invalidations: false,
            ..Default::default()
        };
        let invalidator = Arc::new(CacheInvalidator::with_config(cache.clone(), config));
        let broadcast = BroadcastInvalidator::new(invalidator);

        let listener = Arc::new(TestListener::new());
        broadcast.register("test", listener.clone());

        let event = InvalidationEvent::new("products", ChangeOperation::Update);
        broadcast.on_change_event(event);

        assert_eq!(listener.call_count(), 1);
    }
}

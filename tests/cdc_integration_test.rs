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

//! CDC Integration Tests
//!
//! End-to-end tests for the Change Data Capture module.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use proximadb::cdc::event::{
    ChangeEvent,
    Operation,
    RecordState,
    SourceInfo,
    TransactionInfo,
};
use proximadb::cdc::outbound::{
    DeduplicationCache,
    DeduplicationStrategy,
    EventRouter,
    ExactlyOnceManager,
    IdempotencyKey,
    OutboundConfig,
    Position,
    PositionTracker,
    RouteRule,
    SubscriptionStatus,
    TransactionState,
    WalSubscriber,
};
use proximadb::cdc::transform::{
    FilterRule,
    FilterRuleSet,
    SchemaMapper,
    TransformPipeline,
};
use proximadb::cdc::sinks::{
    KafkaConfig,
    WebhookConfig,
};

// =============================================================================
// Helper Functions
// =============================================================================

fn create_test_source() -> SourceInfo {
    SourceInfo::proximadb("test_db", "test_server")
}

fn create_insert_event(collection: &str, key: &str, lsn: u64) -> ChangeEvent {
    let mut event = ChangeEvent::new(
        create_test_source(),
        Operation::Insert,
        collection,
        key,
    );
    event.lsn = lsn;

    let mut metadata = HashMap::new();
    metadata.insert("tenant".to_string(), serde_json::json!("acme"));
    metadata.insert("priority".to_string(), serde_json::json!(1));
    event.after = Some(RecordState::with_metadata(metadata));

    event
}

fn create_update_event(collection: &str, key: &str, lsn: u64) -> ChangeEvent {
    let mut event = ChangeEvent::new(
        create_test_source(),
        Operation::Update,
        collection,
        key,
    );
    event.lsn = lsn;

    let before = RecordState::with_vector(vec![0.1, 0.2, 0.3]);
    let after = RecordState::with_vector(vec![0.4, 0.5, 0.6]);

    event.before = Some(before);
    event.after = Some(after);

    event
}

fn create_delete_event(collection: &str, key: &str, lsn: u64) -> ChangeEvent {
    let mut event = ChangeEvent::new(
        create_test_source(),
        Operation::Delete,
        collection,
        key,
    );
    event.lsn = lsn;
    event.before = Some(RecordState::with_vector(vec![0.1, 0.2, 0.3]));
    event
}

// =============================================================================
// Event Type Tests
// =============================================================================

#[test]
fn test_change_event_serialization_roundtrip() {
    let event = create_insert_event("products", "prod_1", 100);

    // Serialize to JSON
    let json = event.to_json_bytes().expect("serialization failed");

    // Deserialize back
    let parsed = ChangeEvent::from_json_bytes(&json).expect("deserialization failed");

    assert_eq!(parsed.collection, "products");
    assert_eq!(parsed.key, "prod_1");
    assert_eq!(parsed.lsn, 100);
    assert!(parsed.is_insert());
}

#[test]
fn test_operation_types() {
    let insert = create_insert_event("c", "k", 1);
    let update = create_update_event("c", "k", 2);
    let delete = create_delete_event("c", "k", 3);

    assert!(insert.is_insert());
    assert!(!insert.is_update());
    assert!(!insert.is_delete());

    assert!(update.is_update());
    assert!(update.before.is_some());
    assert!(update.after.is_some());

    assert!(delete.is_delete());
    assert!(delete.before.is_some());
    assert!(delete.after.is_none());
}

#[test]
fn test_transaction_info() {
    let mut event = create_insert_event("products", "p1", 100);

    let tx = TransactionInfo::new("tx_12345")
        .with_total_events(5)
        .with_position(2);

    event = event.with_transaction(tx);

    assert!(event.transaction.is_some());
    let tx = event.transaction.unwrap();
    assert_eq!(tx.id, "tx_12345");
    assert_eq!(tx.total_events, 5);
    assert_eq!(tx.event_position, 2);
    assert!(!tx.is_last_event());
}

// =============================================================================
// Deduplication Tests
// =============================================================================

#[test]
fn test_deduplication_by_event_id() {
    let cache = DeduplicationCache::new(100)
        .with_strategy(DeduplicationStrategy::ByEventId);

    let event1 = create_insert_event("products", "p1", 100);
    let event2 = create_insert_event("products", "p2", 101);

    // First time should not be duplicate
    assert!(!cache.check_and_mark(&event1));

    // Same event should be duplicate
    assert!(cache.check_and_mark(&event1));

    // Different event should not be duplicate
    assert!(!cache.check_and_mark(&event2));

    // Stats should reflect
    let stats = cache.stats();
    assert_eq!(stats.unique, 2);
    assert_eq!(stats.duplicates, 1);
}

#[test]
fn test_deduplication_by_lsn() {
    let cache = DeduplicationCache::new(100)
        .with_strategy(DeduplicationStrategy::ByLsn);

    let mut event1 = create_insert_event("products", "p1", 100);
    let mut event2 = create_insert_event("products", "p2", 100); // Same LSN

    cache.mark_seen(&event1);

    // Same LSN should be duplicate even with different key
    assert!(cache.is_duplicate(&event2));

    event2.lsn = 101;
    assert!(!cache.is_duplicate(&event2));
}

#[test]
fn test_deduplication_cache_eviction() {
    let cache = DeduplicationCache::new(5); // Small cache

    // Add more than capacity
    for i in 0..10 {
        let event = create_insert_event("c", &format!("k_{}", i), i as u64);
        cache.mark_seen(&event);
    }

    // Should have evicted some entries
    assert!(cache.size() <= 5);

    let stats = cache.stats();
    assert!(stats.evictions > 0);
}

// =============================================================================
// Position Tracking Tests
// =============================================================================

#[test]
fn test_position_tracker() {
    let tracker = PositionTracker::new();

    // Initially no position
    assert!(tracker.get("sub1").is_none());

    // Set position
    tracker.set("sub1", Position::from_lsn(100));
    assert_eq!(tracker.get("sub1").unwrap().lsn, 100);

    // Update position
    tracker.set("sub1", Position::from_lsn(200));
    assert_eq!(tracker.get("sub1").unwrap().lsn, 200);
}

#[test]
fn test_pending_tracking() {
    let tracker = PositionTracker::new();

    // Mark some positions as pending
    tracker.mark_pending("sub1", Position::from_lsn(100));
    tracker.mark_pending("sub1", Position::from_lsn(101));
    tracker.mark_pending("sub1", Position::from_lsn(102));

    assert_eq!(tracker.pending_count("sub1"), 3);
    assert!(tracker.has_pending("sub1"));

    // Acknowledge up to 101
    tracker.acknowledge("sub1", 101).unwrap();

    // Should only have 102 pending
    assert_eq!(tracker.pending_count("sub1"), 1);

    // Position should be updated
    assert_eq!(tracker.get("sub1").unwrap().lsn, 101);
}

#[test]
fn test_position_special_values() {
    let beginning = Position::beginning();
    let latest = Position::latest();

    assert!(beginning.is_beginning());
    assert!(!beginning.is_latest());

    assert!(latest.is_latest());
    assert!(!latest.is_beginning());

    assert!(beginning < latest);
}

// =============================================================================
// Exactly-Once Tests
// =============================================================================

#[test]
fn test_exactly_once_transaction_lifecycle() {
    let manager = ExactlyOnceManager::with_defaults();

    let key = IdempotencyKey::new("event_1", 100, "kafka");

    // Begin transaction
    let txn_id = manager.begin_transaction(key.clone()).expect("begin failed");
    assert_eq!(manager.get_state(&txn_id), Some(TransactionState::Pending));

    // Add events
    manager.add_event(&txn_id, 100).unwrap();
    manager.add_event(&txn_id, 101).unwrap();

    // Commit
    manager.commit(&txn_id).expect("commit failed");

    // Transaction should be removed after commit
    assert!(manager.get_state(&txn_id).is_none());

    // Key should be marked as completed (duplicate)
    assert!(manager.is_duplicate(&key));
}

#[test]
fn test_exactly_once_abort_and_retry() {
    let manager = ExactlyOnceManager::with_defaults();

    let key = IdempotencyKey::new("event_1", 100, "kafka");

    // Begin and abort
    let txn_id = manager.begin_transaction(key.clone()).unwrap();
    manager.abort(&txn_id, Some("test error".to_string())).unwrap();

    // Should be able to retry
    let new_key = IdempotencyKey::new("event_1", 100, "kafka");
    let new_txn_id = manager.retry(new_key).expect("retry failed");

    assert_ne!(txn_id, new_txn_id);

    // Now commit
    manager.commit(&new_txn_id).unwrap();
}

#[test]
fn test_exactly_once_two_phase_commit() {
    let manager = ExactlyOnceManager::with_defaults();

    let key = IdempotencyKey::new("event_1", 100, "kafka");

    // Begin
    let txn_id = manager.begin_transaction(key).unwrap();

    // Prepare (phase 1)
    manager.prepare(&txn_id).expect("prepare failed");
    assert_eq!(manager.get_state(&txn_id), Some(TransactionState::Prepared));

    // Commit (phase 2)
    manager.commit(&txn_id).expect("commit failed");
}

// =============================================================================
// Event Router Tests
// =============================================================================

#[test]
fn test_router_basic_routing() {
    let router = EventRouter::new();

    // Route products to kafka
    router.add_rule(
        RouteRule::new("products_to_kafka")
            .with_collection("products*")
            .with_sink("kafka")
    );

    // Route users to webhook
    router.add_rule(
        RouteRule::new("users_to_webhook")
            .with_collection("users")
            .with_sink("webhook")
    );

    // Products should go to kafka
    let event = create_insert_event("products", "p1", 100);
    let decision = router.route(event);
    assert!(decision.has_match);
    assert!(decision.sink_ids.contains(&"kafka".to_string()));

    // Users should go to webhook
    let event = create_insert_event("users", "u1", 101);
    let decision = router.route(event);
    assert!(decision.has_match);
    assert!(decision.sink_ids.contains(&"webhook".to_string()));

    // Unknown collection should have no match
    let event = create_insert_event("unknown", "x1", 102);
    let decision = router.route(event);
    assert!(!decision.has_match);
}

#[test]
fn test_router_default_sink() {
    let router = EventRouter::new();

    router.add_rule(
        RouteRule::new("products_only")
            .with_collection("products")
            .with_sink("kafka")
    );

    router.set_default_sinks(vec!["backup".to_string()]);

    // Products go to kafka
    let event = create_insert_event("products", "p1", 100);
    let decision = router.route(event);
    assert!(decision.sink_ids.contains(&"kafka".to_string()));

    // Others go to default backup
    let event = create_insert_event("other", "o1", 101);
    let decision = router.route(event);
    assert!(decision.sink_ids.contains(&"backup".to_string()));
}

#[test]
fn test_router_operation_filter() {
    let router = EventRouter::new();

    // Only route inserts
    router.add_rule(
        RouteRule::new("inserts_only")
            .with_collection("*")
            .with_operations(vec![Operation::Insert])
            .with_sink("insert_handler")
    );

    // Insert should match
    let event = create_insert_event("products", "p1", 100);
    let decision = router.route(event);
    assert!(decision.has_match);

    // Update should not match
    let event = create_update_event("products", "p1", 101);
    let decision = router.route(event);
    assert!(!decision.has_match);

    // Delete should not match
    let event = create_delete_event("products", "p1", 102);
    let decision = router.route(event);
    assert!(!decision.has_match);
}

#[test]
fn test_router_terminal_rule() {
    let router = EventRouter::new();

    // First rule is terminal
    router.add_rule(
        RouteRule::new("terminal_rule")
            .with_collection("products")
            .with_sink("primary")
            .with_priority(-1) // Higher priority
            .terminal()
    );

    // Second rule should be skipped for products
    router.add_rule(
        RouteRule::new("backup_rule")
            .with_collection("*")
            .with_sink("backup")
    );

    // Products only go to primary (terminal stops processing)
    let event = create_insert_event("products", "p1", 100);
    let decision = router.route(event);
    assert_eq!(decision.matched_rules.len(), 1);
    assert!(decision.sink_ids.contains(&"primary".to_string()));
    assert!(!decision.sink_ids.contains(&"backup".to_string()));

    // Other collections match backup
    let event = create_insert_event("users", "u1", 101);
    let decision = router.route(event);
    assert!(decision.sink_ids.contains(&"backup".to_string()));
}

// =============================================================================
// Subscriber Tests
// =============================================================================

#[tokio::test]
async fn test_subscriber_initialization() {
    let config = OutboundConfig::new()
        .with_name("test_sub")
        .with_collection("products")
        .from_lsn(100);

    let subscriber = WalSubscriber::new("test_sub", config);

    subscriber.initialize().await.expect("init failed");

    assert_eq!(subscriber.current_lsn(), 100);
    assert_eq!(subscriber.status().await, SubscriptionStatus::Active);
}

#[tokio::test]
async fn test_subscriber_poll_and_ack() {
    let config = OutboundConfig::new()
        .with_collection("products");

    let subscriber = WalSubscriber::new("test_sub", config);

    // Push some test events
    let events = vec![
        create_insert_event("products", "p1", 100),
        create_insert_event("products", "p2", 101),
        create_insert_event("products", "p3", 102),
    ];
    subscriber.push_events(events);

    // Poll events
    let polled = subscriber.poll_events().await.expect("poll failed");
    assert_eq!(polled.len(), 3);

    // Should have pending
    assert!(subscriber.has_pending());
    assert_eq!(subscriber.pending_count(), 3);

    // Acknowledge up to LSN 101
    subscriber.acknowledge(101).await.expect("ack failed");

    // Should have 1 pending (LSN 102)
    assert_eq!(subscriber.pending_count(), 1);
}

#[tokio::test]
async fn test_subscriber_collection_filter() {
    let config = OutboundConfig::new()
        .with_collection("products"); // Only products

    let subscriber = WalSubscriber::new("test_sub", config);

    // Push mixed events
    let events = vec![
        create_insert_event("products", "p1", 100),
        create_insert_event("users", "u1", 101),     // Filtered
        create_insert_event("products", "p2", 102),
        create_insert_event("orders", "o1", 103),    // Filtered
    ];
    subscriber.push_events(events);

    // Poll - should only get products
    let polled = subscriber.poll_events().await.expect("poll failed");
    assert_eq!(polled.len(), 2);

    let stats = subscriber.stats();
    assert_eq!(stats.events_filtered, 2);
}

// =============================================================================
// Transform Pipeline Tests
// =============================================================================

#[test]
fn test_transform_pipeline_filter() {
    // Add filter to only include inserts
    let filter = FilterRuleSet::new()
        .with_rule(FilterRule::include_operations(vec![Operation::Insert]));
    let pipeline = TransformPipeline::new().with_filter(filter);

    let events = vec![
        create_insert_event("products", "p1", 100),
        create_update_event("products", "p2", 101),
        create_delete_event("products", "p3", 102),
    ];

    let result = pipeline.transform_batch(events).expect("transform failed");

    // Only insert should pass
    assert_eq!(result.len(), 1);
    assert!(result[0].is_insert());
}

#[test]
fn test_transform_pipeline_schema_mapping() {
    // Map products to items
    let mapper = SchemaMapper::new().map_collection("items");
    let pipeline = TransformPipeline::new().with_schema_mapper(mapper);

    let events = vec![
        create_insert_event("products", "p1", 100),
    ];

    let result = pipeline.transform_batch(events).expect("transform failed");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].collection, "items");
}

// =============================================================================
// Configuration Tests
// =============================================================================

#[test]
fn test_outbound_config_builder() {
    let config = OutboundConfig::new()
        .with_name("my_subscription")
        .with_collection("products")
        .with_collection("users")
        .with_exactly_once(true)
        .with_batch_size(50)
        .from_beginning();

    assert_eq!(config.name, "my_subscription");
    assert!(config.collections.contains("products"));
    assert!(config.collections.contains("users"));
    assert!(config.exactly_once);
    assert_eq!(config.batch_size, 50);
    assert!(config.should_include("products"));
    assert!(config.should_include("users"));
    assert!(!config.should_include("orders"));
}

#[test]
fn test_kafka_config() {
    let config = KafkaConfig::new(vec!["localhost:9092".to_string()])
        .with_topic_pattern("cdc_{collection}")
        .with_acks(proximadb::cdc::sinks::KafkaAcks::All)
        .with_batch_size(100);

    assert_eq!(config.bootstrap_servers, vec!["localhost:9092"]);
    assert_eq!(config.topic_pattern, "cdc_{collection}");
    assert_eq!(config.batch_size, 100);
}

#[test]
fn test_webhook_config() {
    let config = WebhookConfig::new("https://example.com/webhook")
        .with_timeout(5000);

    assert_eq!(config.url, "https://example.com/webhook");
    assert_eq!(config.timeout_ms, 5000);
}

// =============================================================================
// End-to-End Flow Tests
// =============================================================================

#[tokio::test]
async fn test_end_to_end_cdc_flow() {
    // Setup: Create a complete CDC pipeline

    // 1. Deduplication cache
    let dedup = Arc::new(DeduplicationCache::new(1000));

    // 2. Exactly-once manager
    let exactly_once = Arc::new(ExactlyOnceManager::with_defaults());

    // 3. Event router
    let router = Arc::new(EventRouter::new());
    router.add_rule(
        RouteRule::new("all_to_kafka")
            .with_collection("*")
            .with_sink("kafka")
    );

    // 4. Transform pipeline
    let filter = FilterRuleSet::new()
        .with_rule(FilterRule::include_operations(vec![Operation::Insert, Operation::Update]));
    let pipeline = TransformPipeline::new().with_filter(filter);

    // Simulate CDC flow
    let input_events = vec![
        create_insert_event("products", "p1", 100),
        create_update_event("products", "p2", 101),
        create_delete_event("products", "p3", 102), // Will be filtered
        create_insert_event("products", "p4", 103),
    ];

    // Step 1: Transform (filter deletes)
    let events = pipeline.transform_batch(input_events).expect("transform failed");
    assert_eq!(events.len(), 3); // Delete was filtered

    // Step 2: Deduplicate
    let mut unique_events = Vec::new();
    for event in events {
        if !dedup.check_and_mark(&event) {
            unique_events.push(event);
        }
    }
    assert_eq!(unique_events.len(), 3);

    // Step 3: Route events
    for event in &unique_events {
        let decision = router.route(event.clone());
        assert!(decision.has_match);
        assert!(decision.sink_ids.contains(&"kafka".to_string()));
    }

    // Step 4: Begin exactly-once transaction for batch
    let key = IdempotencyKey::new("batch", 100, "kafka");
    let txn_id = exactly_once.begin_transaction(key).unwrap();

    for event in &unique_events {
        exactly_once.add_event(&txn_id, event.lsn).unwrap();
    }

    // Step 5: Commit transaction
    exactly_once.commit(&txn_id).unwrap();

    // Verify stats
    let eo_stats = exactly_once.stats();
    assert_eq!(eo_stats.transactions_committed, 1);

    let dedup_stats = dedup.stats();
    assert_eq!(dedup_stats.unique, 3);
}

#[tokio::test]
async fn test_subscriber_with_router() {
    // Create subscriber
    let config = OutboundConfig::new()
        .with_collection("products")
        .with_exactly_once(false);

    let subscriber = WalSubscriber::new("test_sub", config);

    // Create router
    let router = EventRouter::new();
    router.add_rule(
        RouteRule::new("products_to_kafka")
            .with_collection("products")
            .with_sink("kafka")
    );
    router.add_rule(
        RouteRule::new("products_to_webhook")
            .with_collection("products")
            .with_sink("webhook")
    );

    // Push events
    subscriber.push_events(vec![
        create_insert_event("products", "p1", 100),
        create_insert_event("products", "p2", 101),
    ]);

    // Poll events
    let events = subscriber.poll_events().await.unwrap();

    // Route each event
    for event in events {
        let decision = router.route(event);
        assert!(decision.has_match);
        assert_eq!(decision.matched_rules.len(), 2);
        assert!(decision.sink_ids.contains(&"kafka".to_string()));
        assert!(decision.sink_ids.contains(&"webhook".to_string()));
    }
}

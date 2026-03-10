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

//! # Event Sourcing Integration Tests
//!
//! Tests the event sourcing engine for append-only audit trails,
//! temporal replay, and MiFID II compliance.

use proximadb::storage::engines::impls::eventlog::{
    index::EventIndex, snapshot::SnapshotManager, temporal::TemporalQueryEngine, Event,
    EventLogConfig, EventLogEngine,
};
use proximadb::storage::persistence::filesystem::{
    local::LocalFileSystem, UnifiedCachingFilesystem,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use chrono::Utc;

/// Test basic event append and read
#[tokio::test]
async fn test_eventlog_append_and_read() {
    let base_dir = PathBuf::from("/tmp/test_eventlog_append_read");
    std::fs::create_dir_all(&base_dir).expect("Failed to create test directory");

    let config = EventLogConfig {
        base_dir: base_dir.clone(),
        ..Default::default()
    };

    // Create filesystem
    let local_config = proximadb::storage::persistence::filesystem::local::LocalConfig::default();
    let local_fs = LocalFileSystem::new(local_config)
        .await
        .expect("Failed to create local filesystem");
    let fs = Arc::new(UnifiedCachingFilesystem::new(
        Arc::new(local_fs),
        "eventlog_test".to_string(),
        "eventlog".to_string(),
    ));

    let engine = EventLogEngine::new(config, fs).expect("Failed to create eventlog engine");

    // Append events
    let event1 = Event {
        sequence: 0,
        entity_id: "account:123".to_string(),
        event_type: "AccountCreated".to_string(),
        data: serde_json::json!({"balance": 1000}),
        timestamp: Utc::now(),
        causation_id: None,
        metadata: HashMap::new(),
    };

    let event2 = Event {
        sequence: 0,
        entity_id: "account:123".to_string(),
        event_type: "MoneyDeposited".to_string(),
        data: serde_json::json!({"amount": 500}),
        timestamp: Utc::now(),
        causation_id: None,
        metadata: HashMap::new(),
    };

    let appended1 = engine.append_event(event1).await.expect("Failed to append event1");
    let appended2 = engine.append_event(event2).await.expect("Failed to append event2");

    assert_eq!(appended1.sequence, 1);
    assert_eq!(appended2.sequence, 2);

    // Read events
    let events = engine
        .read_events(&"account:123".to_string(), 0, 10)
        .await
        .expect("Failed to read events");

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "AccountCreated");
    assert_eq!(events[1].event_type, "MoneyDeposited");

    // Cleanup
    let _ = std::fs::remove_dir_all(base_dir);
}

/// Test immutable audit trail
#[tokio::test]
async fn test_eventlog_immutable_audit_trail() {
    let base_dir = PathBuf::from("/tmp/test_eventlog_immutable");
    std::fs::create_dir_all(&base_dir).expect("Failed to create test directory");

    let config = EventLogConfig {
        base_dir: base_dir.clone(),
        ..Default::default()
    };

    // Create filesystem
    let local_config = proximadb::storage::persistence::filesystem::local::LocalConfig::default();
    let local_fs = LocalFileSystem::new(local_config)
        .await
        .expect("Failed to create local filesystem");
    let fs = Arc::new(UnifiedCachingFilesystem::new(
        Arc::new(local_fs),
        "eventlog_immutable".to_string(),
        "eventlog".to_string(),
    ));

    let engine = EventLogEngine::new(config, fs).expect("Failed to create eventlog engine");

    // Append trade events (simulating ibkrtrading use case)
    for i in 1..=10 {
        let event = Event {
            sequence: 0,
            entity_id: "trade:ABC123".to_string(),
            event_type: "TradeExecuted".to_string(),
            data: serde_json::json!({
                "symbol": "AAPL",
                "quantity": 100 * i,
                "price": 150.50 + i as f64,
            }),
            timestamp: Utc::now(),
            causation_id: Some(format!("order:{}", i)),
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("user_id".to_string(), serde_json::json!("trader_1"));
                meta.insert("request_id".to_string(), serde_json::json!(format!("req_{}", i)));
                meta.insert("origin".to_string(), serde_json::json!("api"));
                meta
            },
        };

        let appended = engine.append_event(event).await.expect("Failed to append trade event");
        assert_eq!(appended.sequence, i as u64);
    }

    // Verify monotonically increasing sequence numbers
    let events = engine
        .read_events(&"trade:ABC123".to_string(), 0, 100)
        .await
        .expect("Failed to read trade events");

    assert_eq!(events.len(), 10);
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, (i + 1) as u64);
        assert_eq!(event.entity_id, "trade:ABC123");
        assert_eq!(event.event_type, "TradeExecuted");
    }

    // Verify stats
    let stats = engine.get_stats().await;
    assert_eq!(stats.total_events, 10);

    // Cleanup
    let _ = std::fs::remove_dir_all(base_dir);
}

/// Test MiFID II regulatory compliance
#[tokio::test]
async fn test_eventlog_mifid2_compliance() {
    let base_dir = PathBuf::from("/tmp/test_eventlog_mifid2");
    std::fs::create_dir_all(&base_dir).expect("Failed to create test directory");

    let config = EventLogConfig {
        base_dir: base_dir.clone(),
        regulatory_mode: true, // Enable MiFID II compliance
        ..Default::default()
    };

    // Create filesystem
    let local_config = proximadb::storage::persistence::filesystem::local::LocalConfig::default();
    let local_fs = LocalFileSystem::new(local_config)
        .await
        .expect("Failed to create local filesystem");
    let fs = Arc::new(UnifiedCachingFilesystem::new(
        Arc::new(local_fs),
        "eventlog_mifid2".to_string(),
        "eventlog".to_string(),
    ));

    let engine = EventLogEngine::new(config, fs).expect("Failed to create eventlog engine");

    // Valid trade event with required metadata
    let valid_event = Event {
        sequence: 0,
        entity_id: "trade:EU456".to_string(),
        event_type: "TradeExecuted".to_string(),
        data: serde_json::json!({"symbol": "EUR/USD", "amount": 1000000}),
        timestamp: Utc::now(),
        causation_id: None,
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("user_id".to_string(), serde_json::json!("trader_eu_1"));
            meta.insert("request_id".to_string(), serde_json::json!("req_mifid_001"));
            meta.insert("origin".to_string(), serde_json::json!("trading_terminal"));
            meta
        },
    };

    // Should succeed
    let result = engine.append_event(valid_event).await;
    assert!(result.is_ok(), "Valid event with required metadata should succeed");

    // Invalid event missing required metadata
    let invalid_event = Event {
        sequence: 0,
        entity_id: "trade:EU789".to_string(),
        event_type: "TradeExecuted".to_string(),
        data: serde_json::json!({"symbol": "GBP/USD", "amount": 500000}),
        timestamp: Utc::now(),
        causation_id: None,
        metadata: HashMap::new(), // Missing required fields!
    };

    // Should fail
    let result = engine.append_event(invalid_event).await;
    assert!(result.is_err(), "Event without required metadata should fail in regulatory mode");

    // Cleanup
    let _ = std::fs::remove_dir_all(base_dir);
}

/// Test temporal queries - point-in-time reconstruction
#[tokio::test]
async fn test_eventlog_temporal_queries() {
    let base_dir = PathBuf::from("/tmp/test_eventlog_temporal");
    std::fs::create_dir_all(&base_dir).expect("Failed to create test directory");

    let config = EventLogConfig {
        base_dir: base_dir.clone(),
        ..Default::default()
    };

    // Create filesystem
    let local_config = proximadb::storage::persistence::filesystem::local::LocalConfig::default();
    let local_fs = LocalFileSystem::new(local_config)
        .await
        .expect("Failed to create local filesystem");
    let fs = Arc::new(UnifiedCachingFilesystem::new(
        Arc::new(local_fs),
        "eventlog_temporal".to_string(),
        "eventlog".to_string(),
    ));

    let engine = EventLogEngine::new(config.clone(), fs).expect("Failed to create eventlog engine");

    // Append events for an account
    let initial_balance = 1000;
    let deposit_amount = 500;
    let withdrawal_amount = 200;

    let event1 = Event {
        sequence: 0,
        entity_id: "account:456".to_string(),
        event_type: "AccountCreated".to_string(),
        data: serde_json::json!({"balance": initial_balance}),
        timestamp: Utc::now(),
        causation_id: None,
        metadata: HashMap::new(),
    };

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let event2 = Event {
        sequence: 0,
        entity_id: "account:456".to_string(),
        event_type: "MoneyDeposited".to_string(),
        data: serde_json::json!({"amount": deposit_amount}),
        timestamp: Utc::now(),
        causation_id: None,
        metadata: HashMap::new(),
    };

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let event3 = Event {
        sequence: 0,
        entity_id: "account:456".to_string(),
        event_type: "MoneyWithdrawn".to_string(),
        data: serde_json::json!({"amount": withdrawal_amount}),
        timestamp: Utc::now(),
        causation_id: None,
        metadata: HashMap::new(),
    };

    let appended1 = engine.append_event(event1).await.expect("Failed to append event1");
    let appended2 = engine.append_event(event2).await.expect("Failed to append event2");
    let appended3 = engine.append_event(event3).await.expect("Failed to append event3");

    // Get current state
    let current_state = engine
        .get_entity_state(&"account:456".to_string())
        .await
        .expect("Failed to get current state");

    // Verify state has been updated through events
    assert!(current_state.is_object());

    // Get state as of event 2 (after deposit, before withdrawal)
    let state_as_of = engine
        .get_state_as_of(&"account:456".to_string(), appended2.timestamp)
        .await
        .expect("Failed to get state as of");

    assert!(state_as_of.is_object());

    // Replay events
    let replayed = engine
        .replay_events(&"account:456".to_string(), 1, Some(2))
        .await
        .expect("Failed to replay events");

    assert_eq!(replayed.len(), 2);

    // Cleanup
    let _ = std::fs::remove_dir_all(base_dir);
}

/// Test snapshot creation and loading
#[tokio::test]
async fn test_eventlog_snapshots() {
    let base_dir = PathBuf::from("/tmp/test_eventlog_snapshots");
    std::fs::create_dir_all(&base_dir).expect("Failed to create test directory");

    let config = EventLogConfig {
        base_dir: base_dir.clone(),
        snapshot_interval: 5, // Snapshot every 5 events
        ..Default::default()
    };

    // Create filesystem
    let local_config = proximadb::storage::persistence::filesystem::local::LocalConfig::default();
    let local_fs = LocalFileSystem::new(local_config)
        .await
        .expect("Failed to create local filesystem");
    let fs = Arc::new(UnifiedCachingFilesystem::new(
        Arc::new(local_fs),
        "eventlog_snapshots".to_string(),
        "eventlog".to_string(),
    ));

    let engine = EventLogEngine::new(config, fs).expect("Failed to create eventlog engine");

    // Append 10 events (should trigger 2 snapshots)
    for i in 1..=10 {
        let event = Event {
            sequence: 0,
            entity_id: "order:snapshot_test".to_string(),
            event_type: "OrderUpdated".to_string(),
            data: serde_json::json!({"version": i}),
            timestamp: Utc::now(),
            causation_id: None,
            metadata: HashMap::new(),
        };

        engine.append_event(event).await.expect("Failed to append event");
    }

    // Check stats
    let stats = engine.get_stats().await;
    assert_eq!(stats.total_events, 10);
    assert_eq!(stats.snapshots_created, 2); // At sequences 5 and 10

    // Cleanup
    let _ = std::fs::remove_dir_all(base_dir);
}

/// Test event replay
#[tokio::test]
async fn test_eventlog_replay() {
    let base_dir = PathBuf::from("/tmp/test_eventlog_replay");
    std::fs::create_dir_all(&base_dir).expect("Failed to create test directory");

    let config = EventLogConfig {
        base_dir: base_dir.clone(),
        ..Default::default()
    };

    // Create filesystem
    let local_config = proximadb::storage::persistence::filesystem::local::LocalConfig::default();
    let local_fs = LocalFileSystem::new(local_config)
        .await
        .expect("Failed to create local filesystem");
    let fs = Arc::new(UnifiedCachingFilesystem::new(
        Arc::new(local_fs),
        "eventlog_replay".to_string(),
        "eventlog".to_string(),
    ));

    let engine = EventLogEngine::new(config, fs).expect("Failed to create eventlog engine");

    // Create a sequence of events representing order lifecycle
    let events = vec![
        ("OrderCreated", serde_json::json!({"status": "pending"})),
        ("OrderValidated", serde_json::json!({"status": "validated"})),
        ("OrderFilled", serde_json::json!({"status": "filled", "quantity": 100})),
        ("OrderSettled", serde_json::json!({"status": "settled"})),
    ];

    for (event_type, data) in &events {
        let event = Event {
            sequence: 0,
            entity_id: "order:lifecycle".to_string(),
            event_type: event_type.to_string(),
            data: data.clone(),
            timestamp: Utc::now(),
            causation_id: None,
            metadata: HashMap::new(),
        };

        engine.append_event(event).await.expect("Failed to append event");
    }

    // Replay all events
    let replayed = engine
        .replay_events(&"order:lifecycle".to_string(), 0, None)
        .await
        .expect("Failed to replay events");

    assert_eq!(replayed.len(), 4);
    assert_eq!(replayed[0].event_type, "OrderCreated");
    assert_eq!(replayed[1].event_type, "OrderValidated");
    assert_eq!(replayed[2].event_type, "OrderFilled");
    assert_eq!(replayed[3].event_type, "OrderSettled");

    // Replay partial (first 2 events)
    let partial_replay = engine
        .replay_events(&"order:lifecycle".to_string(), 0, Some(2))
        .await
        .expect("Failed to replay partial events");

    assert_eq!(partial_replay.len(), 2);

    // Cleanup
    let _ = std::fs::remove_dir_all(base_dir);
}

/// Test high-volume event ingestion (simulating ibkrtrading 119MB/day)
#[tokio::test]
async fn test_eventlog_high_volume_ingestion() {
    let base_dir = PathBuf::from("/tmp/test_eventlog_volume");
    std::fs::create_dir_all(&base_dir).expect("Failed to create test directory");

    let config = EventLogConfig {
        base_dir: base_dir.clone(),
        enable_compression: true,
        ..Default::default()
    };

    // Create filesystem
    let local_config = proximadb::storage::persistence::filesystem::local::LocalConfig::default();
    let local_fs = LocalFileSystem::new(local_config)
        .await
        .expect("Failed to create local filesystem");
    let fs = Arc::new(UnifiedCachingFilesystem::new(
        Arc::new(local_fs),
        "eventlog_volume".to_string(),
        "eventlog".to_string(),
    ));

    let engine = Arc::new(EventLogEngine::new(config, fs).expect("Failed to create eventlog engine"));

    // Simulate high-volume trade events (smaller scale for test)
    let num_events = 100;
    let mut handles = Vec::new();

    for batch in 0..10 {
        let engine_clone = engine.clone();
        let handle = tokio::spawn(async move {
            for i in 0..(num_events / 10) {
                let event = Event {
                    sequence: 0,
                    entity_id: format!("trade:batch_{}", batch),
                    event_type: "TradeExecuted".to_string(),
                    data: serde_json::json!({
                        "symbol": "AAPL",
                        "quantity": 100 + i as i32,
                        "price": 150.50 + i as f64,
                    }),
                    timestamp: Utc::now(),
                    causation_id: Some(format!("order:{}", batch * 10 + i)),
                    metadata: {
                        let mut meta = HashMap::new();
                        meta.insert("user_id".to_string(), serde_json::json!(format!("trader_{}", batch)));
                        meta.insert("request_id".to_string(), serde_json::json!(format!("req_{}", i)));
                        meta.insert("origin".to_string(), serde_json::json!("api"));
                        meta
                    },
                };

                engine_clone.append_event(event).await.expect("Failed to append event");
            }
        });
        handles.push(handle);
    }

    // Wait for all batches
    for handle in handles {
        handle.await.expect("Failed to join task");
    }

    // Verify all events were written
    let stats = engine.get_stats().await;
    assert_eq!(stats.total_events, num_events);

    // Cleanup
    let _ = std::fs::remove_dir_all(base_dir);
}

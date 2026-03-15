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

//! CDC End-to-End Tests
//!
//! This module tests the full CDC pipeline:
//! - Event creation, serialization, and deserialization
//! - MySQL binlog decoder parsing
//! - MongoDB change stream event parsing
//! - Webhook sink with dry-run mode
//! - Kafka sink (simulated without broker)
//! - Connector -> Decoder -> Sink pipeline integration

use std::collections::HashMap;
use std::sync::Arc;

// CDC connectors - MongoDB
use proximadb::cdc::connectors::mongodb::change_event::{
    ChangeStreamOperation, DocumentKey, MongoChangeEvent, Namespace, ResumeToken, UpdateDescription,
};
use proximadb::cdc::connectors::mongodb::config::{MongoCollectionConfig, MongoDbConfig};
use proximadb::cdc::connectors::mongodb::connector::MongoDbConnector;

// CDC connectors - MySQL
use proximadb::cdc::connectors::mysql::config::{BinlogPosition, MySqlConfig};
use proximadb::cdc::connectors::mysql::connector::MySqlConnector;
use proximadb::cdc::connectors::mysql::decoder::{
    BinlogDecoder, BinlogEvent, ColumnType, ColumnValue, EventType, RowData, RowEvent,
    RowEventType, TableMapEvent,
};

// CDC core types
use proximadb::cdc::event::{ChangeEvent, Operation, RecordState, SourceInfo, TransactionInfo};
use proximadb::cdc::offset::{MemoryOffsetStore, Offset, OffsetStore};
use proximadb::cdc::source::{CdcSource, SourceStatus};

// CDC sinks (re-exported at sinks level)
use proximadb::cdc::sinks::{CdcSink, KafkaConfig, KafkaSink, WebhookConfig, WebhookSink};

// MessageFormat is re-exported via traits
use proximadb::cdc::sinks::MessageFormat;

// ============================================================================
// Event Serialization/Deserialization Tests
// ============================================================================

#[test]
fn test_change_event_json_roundtrip() {
    let source = SourceInfo::postgres("testdb", "public", "server1");
    let state = RecordState::with_vector(vec![0.1, 0.2, 0.3])
        .add_metadata("name", serde_json::json!("test_user"))
        .add_metadata("age", serde_json::json!(30));

    let event = ChangeEvent::new_insert(source, "users".to_string(), "user_1".to_string(), state)
        .with_lsn(12345);

    // Serialize to JSON bytes
    let bytes = event
        .to_json_bytes()
        .expect("Should serialize change event to JSON");
    assert!(!bytes.is_empty());

    // Deserialize back
    let parsed =
        ChangeEvent::from_json_bytes(&bytes).expect("Should deserialize change event from JSON");

    assert_eq!(parsed.collection, "users");
    assert_eq!(parsed.key, "user_1");
    assert_eq!(parsed.lsn, 12345);
    assert!(parsed.is_insert());
    assert!(parsed.after.is_some());

    let after = parsed.after.as_ref().expect("Should have after state");
    assert!(after.vector.is_some());
    assert_eq!(after.vector.as_ref().expect("vector present").len(), 3);
    assert!(after.metadata.contains_key("name"));
    assert!(after.metadata.contains_key("age"));
}

#[test]
fn test_change_event_all_operations_serialize() {
    let source = SourceInfo::mysql("testdb", "server1");

    let operations = vec![
        Operation::Insert,
        Operation::Update,
        Operation::Delete,
        Operation::Truncate,
        Operation::SchemaChange,
        Operation::Snapshot,
        Operation::Begin,
        Operation::Commit,
        Operation::Rollback,
    ];

    for op in operations {
        let event = ChangeEvent::new(source.clone(), op, "test_collection", "key_1");

        let bytes = event
            .to_json_bytes()
            .expect("Should serialize any operation");
        let parsed =
            ChangeEvent::from_json_bytes(&bytes).expect("Should deserialize any operation");
        assert_eq!(parsed.operation, op);
    }
}

#[test]
fn test_change_event_with_transaction_info() {
    let source = SourceInfo::postgres("db", "public", "srv");
    let tx = TransactionInfo::new("tx_001")
        .with_total_events(5)
        .with_position(2);

    let event = ChangeEvent::new(source, Operation::Insert, "orders", "order_1")
        .with_transaction(tx)
        .with_header("trace_id", "abc-123");

    let bytes = event
        .to_json_bytes()
        .expect("Should serialize with tx info");
    let parsed = ChangeEvent::from_json_bytes(&bytes).expect("Should deserialize with tx info");

    assert!(parsed.transaction.is_some());
    let tx = parsed.transaction.expect("transaction present");
    assert_eq!(tx.id, "tx_001");
    assert_eq!(tx.total_events, 5);
    assert_eq!(tx.event_position, 2);
    assert_eq!(parsed.headers.get("trace_id"), Some(&"abc-123".to_string()));
}

#[test]
fn test_record_state_with_raw_data() {
    let raw = serde_json::json!({
        "_id": "doc123",
        "name": "Test",
        "embedding": [1.0, 2.0, 3.0],
        "tags": ["a", "b"]
    });

    let state = RecordState::from_raw(raw.clone());
    assert!(state.raw.is_some());
    assert_eq!(state.raw.as_ref().expect("raw present"), &raw);
}

// ============================================================================
// MySQL Binlog Decoder Tests
// ============================================================================

#[test]
fn test_decoder_event_type_all_variants() {
    let mappings = vec![
        (1u8, EventType::StartEvent),
        (2, EventType::QueryEvent),
        (3, EventType::StopEvent),
        (4, EventType::RotateEvent),
        (5, EventType::IntvarEvent),
        (15, EventType::FormatDescriptionEvent),
        (16, EventType::XidEvent),
        (19, EventType::TableMapEvent),
        (23, EventType::WriteRowsEventV1),
        (24, EventType::UpdateRowsEventV1),
        (25, EventType::DeleteRowsEventV1),
        (30, EventType::WriteRowsEvent),
        (31, EventType::UpdateRowsEvent),
        (32, EventType::DeleteRowsEvent),
        (33, EventType::GtidEvent),
        (34, EventType::AnonymousGtidEvent),
        (35, EventType::PreviousGtidsEvent),
        (200, EventType::Unknown),
    ];

    for (byte, expected) in mappings {
        assert_eq!(EventType::from(byte), expected);
    }
}

#[test]
fn test_decoder_column_type_all_variants() {
    let mappings = vec![
        (0u8, ColumnType::Decimal),
        (1, ColumnType::Tiny),
        (2, ColumnType::Short),
        (3, ColumnType::Long),
        (4, ColumnType::Float),
        (5, ColumnType::Double),
        (6, ColumnType::Null),
        (7, ColumnType::Timestamp),
        (8, ColumnType::LongLong),
        (9, ColumnType::Int24),
        (10, ColumnType::Date),
        (11, ColumnType::Time),
        (12, ColumnType::DateTime),
        (13, ColumnType::Year),
        (15, ColumnType::Varchar),
        (16, ColumnType::Bit),
        (245, ColumnType::Json),
        (246, ColumnType::NewDecimal),
        (247, ColumnType::Enum),
        (248, ColumnType::Set),
        (249, ColumnType::TinyBlob),
        (250, ColumnType::MediumBlob),
        (251, ColumnType::LongBlob),
        (252, ColumnType::Blob),
        (253, ColumnType::VarString),
        (254, ColumnType::String),
        (255, ColumnType::Geometry),
        (100, ColumnType::Unknown),
    ];

    for (byte, expected) in mappings {
        assert_eq!(ColumnType::from(byte), expected);
    }
}

#[test]
fn test_decoder_rejects_short_data() {
    let mut decoder = BinlogDecoder::new();
    // Event header is 19 bytes; anything shorter should fail
    let result = decoder.decode(&[0u8; 18]);
    assert!(result.is_err());

    let result = decoder.decode(&[0u8; 0]);
    assert!(result.is_err());
}

#[test]
fn test_decoder_handles_minimal_xid_event() {
    let mut decoder = BinlogDecoder::new();

    // Build a valid event header (19 bytes) + XID data (8 bytes)
    let mut data = Vec::new();
    // timestamp (4 bytes)
    data.extend_from_slice(&1000u32.to_le_bytes());
    // event_type = XidEvent (16)
    data.push(16);
    // server_id (4 bytes)
    data.extend_from_slice(&1u32.to_le_bytes());
    // event_length = 19 (header) + 8 (xid) = 27
    data.extend_from_slice(&27u32.to_le_bytes());
    // next_position (4 bytes)
    data.extend_from_slice(&27u32.to_le_bytes());
    // flags (2 bytes)
    data.extend_from_slice(&0u16.to_le_bytes());
    // XID data (8 bytes)
    data.extend_from_slice(&42u64.to_le_bytes());

    let result = decoder.decode(&data);
    assert!(result.is_ok(), "XID event should decode successfully");

    if let Ok(Some(BinlogEvent::Xid { xid })) = result {
        assert_eq!(xid, 42);
    } else {
        panic!("Expected Xid event");
    }
}

#[test]
fn test_decoder_handles_unknown_event_type() {
    let mut decoder = BinlogDecoder::new();

    // Build header with unknown event type 99
    let mut data = Vec::new();
    data.extend_from_slice(&1000u32.to_le_bytes()); // timestamp
    data.push(99); // unknown event type
    data.extend_from_slice(&1u32.to_le_bytes()); // server_id
    data.extend_from_slice(&19u32.to_le_bytes()); // event_length = header only
    data.extend_from_slice(&19u32.to_le_bytes()); // next_position
    data.extend_from_slice(&0u16.to_le_bytes()); // flags

    let result = decoder.decode(&data);
    assert!(result.is_ok());
    // Unknown events should return None (skipped)
    assert!(result.expect("decode ok").is_none());
}

#[test]
fn test_decoder_table_map_cache() {
    let mut decoder = BinlogDecoder::new();

    // Insert a table map manually
    let _table_map = TableMapEvent {
        table_id: 42,
        schema: "mydb".to_string(),
        table: "users".to_string(),
        columns: vec![],
    };

    // Verify get_table_map works
    assert!(decoder.get_table_map(42).is_none());

    // We can't easily construct a valid TABLE_MAP_EVENT binary,
    // but we can test the cache directly
    decoder.clear_table_maps();
    assert!(decoder.get_table_map(42).is_none());
}

#[test]
fn test_column_value_as_string_all_variants() {
    assert!(ColumnValue::Null.as_string().is_none());
    assert_eq!(ColumnValue::Int(-42).as_string(), Some("-42".to_string()));
    assert_eq!(ColumnValue::UInt(42).as_string(), Some("42".to_string()));
    assert_eq!(
        ColumnValue::Float(3.14).as_string(),
        Some("3.14".to_string())
    );
    assert_eq!(
        ColumnValue::String("hello".to_string()).as_string(),
        Some("hello".to_string())
    );

    // Valid UTF-8 bytes
    assert_eq!(
        ColumnValue::Bytes(b"abc".to_vec()).as_string(),
        Some("abc".to_string())
    );
    // Invalid UTF-8 bytes
    assert!(ColumnValue::Bytes(vec![0xFF, 0xFE]).as_string().is_none());

    assert_eq!(
        ColumnValue::Json(serde_json::json!({"a": 1})).as_string(),
        Some("{\"a\":1}".to_string())
    );
}

#[test]
fn test_binlog_event_helpers() {
    let table_map = TableMapEvent {
        table_id: 1,
        schema: "testdb".to_string(),
        table: "orders".to_string(),
        columns: vec![],
    };

    // WriteRows event
    let write = BinlogEvent::WriteRows(RowEvent {
        table_id: 1,
        event_type: RowEventType::Insert,
        table_map: Some(table_map.clone()),
        rows: vec![],
    });
    assert_eq!(write.database(), "testdb");
    assert_eq!(write.table_name(), Some("orders".to_string()));
    assert!(write.row_data().is_none()); // No rows

    // Rotate event
    let rotate = BinlogEvent::Rotate {
        filename: "mysql-bin.000002".to_string(),
        position: 4,
    };
    assert_eq!(rotate.position(), Some(4));
    assert_eq!(rotate.database(), "");
    assert!(rotate.table_name().is_none());

    // Xid event
    let xid = BinlogEvent::Xid { xid: 100 };
    assert!(xid.event_type().is_none());
    assert!(xid.row_id().is_none());
}

#[test]
fn test_binlog_event_row_data_json() {
    let table_map = TableMapEvent {
        table_id: 1,
        schema: "db".to_string(),
        table: "t".to_string(),
        columns: vec![],
    };

    let row_data = RowData {
        before: None,
        after: Some(vec![
            ColumnValue::String("hello".to_string()),
            ColumnValue::Int(42),
            ColumnValue::Null,
        ]),
    };

    let write = BinlogEvent::WriteRows(RowEvent {
        table_id: 1,
        event_type: RowEventType::Insert,
        table_map: Some(table_map),
        rows: vec![row_data],
    });

    let json = write.row_data();
    assert!(json.is_some());

    let obj = json.expect("json present");
    assert!(obj.is_object());
    // Columns are named col_0, col_1, etc. when table_map has no column names
    assert_eq!(obj.get("col_0"), Some(&serde_json::json!("hello")));
    assert_eq!(obj.get("col_1"), Some(&serde_json::json!(42)));
    assert_eq!(obj.get("col_2"), Some(&serde_json::Value::Null));
}

#[test]
fn test_table_map_event_full_name() {
    let tm = TableMapEvent {
        table_id: 99,
        schema: "production".to_string(),
        table: "inventory".to_string(),
        columns: vec![],
    };
    assert_eq!(tm.full_name(), "production.inventory");
}

// ============================================================================
// MongoDB Change Stream Event Parsing Tests
// ============================================================================

#[test]
fn test_mongo_change_event_serialization_roundtrip() {
    let event = MongoChangeEvent {
        id: ResumeToken::new("token_abc123"),
        operation_type: ChangeStreamOperation::Insert,
        cluster_time: None,
        ns: Namespace::new("testdb", "users"),
        document_key: Some(DocumentKey {
            id: serde_json::json!("user_001"),
            extra: HashMap::new(),
        }),
        full_document: Some(serde_json::json!({
            "_id": "user_001",
            "name": "Alice",
            "embedding": [0.1, 0.2, 0.3]
        })),
        full_document_before_change: None,
        update_description: None,
        txn_number: None,
        lsid: None,
    };

    let json = serde_json::to_string(&event).expect("Should serialize MongoChangeEvent");
    let parsed: MongoChangeEvent =
        serde_json::from_str(&json).expect("Should deserialize MongoChangeEvent");

    assert_eq!(parsed.operation_type, ChangeStreamOperation::Insert);
    assert_eq!(parsed.ns.db, "testdb");
    assert_eq!(parsed.ns.coll, "users");
    assert_eq!(parsed.get_id(), Some("user_001".to_string()));
    assert!(parsed.full_document.is_some());
}

#[test]
fn test_mongo_change_event_update_with_description() {
    let mut updated_fields = HashMap::new();
    updated_fields.insert("name".to_string(), serde_json::json!("Bob Updated"));
    updated_fields.insert("score".to_string(), serde_json::json!(95));

    let event = MongoChangeEvent {
        id: ResumeToken::new("token_update"),
        operation_type: ChangeStreamOperation::Update,
        cluster_time: Some(serde_json::json!({"$timestamp": {"t": 1234, "i": 1}})),
        ns: Namespace::new("mydb", "scores"),
        document_key: Some(DocumentKey {
            id: serde_json::json!({"$oid": "507f1f77bcf86cd799439011"}),
            extra: HashMap::new(),
        }),
        full_document: Some(serde_json::json!({
            "name": "Bob Updated",
            "score": 95
        })),
        full_document_before_change: Some(serde_json::json!({
            "name": "Bob",
            "score": 80
        })),
        update_description: Some(UpdateDescription {
            updated_fields,
            removed_fields: vec!["old_field".to_string()],
            truncated_arrays: vec![],
        }),
        txn_number: Some(42),
        lsid: None,
    };

    assert!(event.is_update());
    assert_eq!(event.get_id(), Some("507f1f77bcf86cd799439011".to_string()));

    let desc = event
        .update_description
        .as_ref()
        .expect("update_description present");
    assert!(desc.has_updates());
    assert!(desc.has_removals());
    assert_eq!(
        desc.get_updated("name"),
        Some(&serde_json::json!("Bob Updated"))
    );
}

#[test]
fn test_mongo_all_operation_types() {
    let ops = vec![
        (ChangeStreamOperation::Insert, true, false, false, false),
        (ChangeStreamOperation::Update, false, true, false, false),
        (ChangeStreamOperation::Replace, false, false, false, true),
        (ChangeStreamOperation::Delete, false, false, true, false),
    ];

    for (op, is_insert, is_update, is_delete, is_replace) in ops {
        let event = MongoChangeEvent {
            id: ResumeToken::new("token"),
            operation_type: op,
            cluster_time: None,
            ns: Namespace::new("db", "coll"),
            document_key: None,
            full_document: None,
            full_document_before_change: None,
            update_description: None,
            txn_number: None,
            lsid: None,
        };

        assert_eq!(event.is_insert(), is_insert, "op={:?}", op);
        assert_eq!(event.is_update(), is_update, "op={:?}", op);
        assert_eq!(event.is_delete(), is_delete, "op={:?}", op);
        assert_eq!(event.is_replace(), is_replace, "op={:?}", op);
    }
}

// ============================================================================
// Webhook Sink Tests (using dry-run mode)
// ============================================================================

#[tokio::test]
async fn test_webhook_sink_dry_run_single_event() {
    let config = WebhookConfig::new("https://example.com/webhook/{collection}");
    let sink = WebhookSink::new_dry_run(config);

    let event = ChangeEvent::new(
        SourceInfo::postgres("db", "public", "srv"),
        Operation::Insert,
        "users",
        "u1",
    );

    let result = sink.send(event).await;
    assert!(result.is_ok());

    let stats = sink.stats();
    assert_eq!(stats.events_sent, 1);
    assert!(stats.bytes_sent > 0);
}

#[tokio::test]
async fn test_webhook_sink_dry_run_batch() {
    let config = WebhookConfig::new("https://example.com/events");
    let sink = WebhookSink::new_dry_run(config);

    let events: Vec<ChangeEvent> = (0..5)
        .map(|i| {
            ChangeEvent::new(
                SourceInfo::mysql("db", "srv"),
                Operation::Update,
                "products",
                format!("prod_{}", i),
            )
        })
        .collect();

    let result = sink.send_batch(events).await;
    assert!(result.is_ok());

    let stats = sink.stats();
    assert_eq!(stats.events_sent, 5);
}

#[tokio::test]
async fn test_webhook_sink_dry_run_flush_and_close() {
    let config = WebhookConfig::new("https://example.com/events");
    let sink = WebhookSink::new_dry_run(config);

    // Flush on empty buffer should succeed
    let result = sink.flush().await;
    assert!(result.is_ok());

    // Close should succeed
    let result = sink.close().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_webhook_sink_url_resolution() {
    let config = WebhookConfig::new("https://api.example.com/{collection}/{key}/{operation}");

    let event = ChangeEvent::new(
        SourceInfo::postgres("db", "public", "srv"),
        Operation::Delete,
        "users",
        "user_42",
    );

    let resolved = config.resolve_url(&event);
    assert_eq!(resolved, "https://api.example.com/users/user_42/delete");
}

// ============================================================================
// Kafka Sink Tests (simulated without broker)
// ============================================================================

#[tokio::test]
async fn test_kafka_sink_simulated_send() {
    let config = KafkaConfig::new(vec!["localhost:9092"]);
    let sink = KafkaSink::new(config);

    // Connect (simulated without cdc-kafka feature)
    sink.connect()
        .await
        .expect("Connect should succeed in simulated mode");
    assert!(sink.is_connected());

    let event = ChangeEvent::new(
        SourceInfo::postgres("testdb", "public", "server1"),
        Operation::Insert,
        "orders",
        "order_1",
    );

    let result = sink.send(event).await;
    assert!(result.is_ok());

    let stats = sink.stats();
    assert_eq!(stats.events_sent, 1);
}

#[tokio::test]
async fn test_kafka_sink_simulated_batch() {
    let config = KafkaConfig::new(vec!["broker1:9092", "broker2:9092"]);
    let sink = KafkaSink::new(config);
    sink.connect().await.expect("Connect ok");

    let events: Vec<ChangeEvent> = (0..10)
        .map(|i| {
            ChangeEvent::new(
                SourceInfo::mongodb("analytics", "srv"),
                Operation::Insert,
                "events",
                format!("evt_{}", i),
            )
        })
        .collect();

    let result = sink.send_batch(events).await;
    assert!(result.is_ok());

    let stats = sink.stats();
    assert_eq!(stats.events_sent, 10);
    assert_eq!(stats.batches_sent, 1);
}

#[tokio::test]
async fn test_kafka_sink_topic_resolution() {
    let config =
        KafkaConfig::new(vec!["localhost:9092"]).with_topic_pattern("cdc.{database}.{collection}");

    let event = ChangeEvent::new(
        SourceInfo::postgres("mydb", "public", "srv"),
        Operation::Insert,
        "users",
        "u1",
    );

    let topic = config.resolve_topic(&event);
    assert_eq!(topic, "cdc.mydb.users");
}

#[tokio::test]
async fn test_kafka_sink_key_resolution() {
    let config = KafkaConfig::new(vec!["localhost:9092"]).with_key_pattern("{collection}.{key}");

    let event = ChangeEvent::new(
        SourceInfo::postgres("db", "public", "srv"),
        Operation::Update,
        "orders",
        "order_99",
    );

    let key = config.resolve_key(&event);
    assert_eq!(key, "orders.order_99");
}

#[tokio::test]
async fn test_kafka_sink_flush_close() {
    let config = KafkaConfig::new(vec!["localhost:9092"]);
    let sink = KafkaSink::new(config);
    sink.connect().await.expect("Connect ok");

    sink.flush().await.expect("Flush should succeed");
    sink.close().await.expect("Close should succeed");
    assert!(!sink.is_connected());
}

// ============================================================================
// Message Format Tests
// ============================================================================

#[test]
fn test_message_format_json_serialization() {
    let event = ChangeEvent::new(
        SourceInfo::mysql("db", "srv"),
        Operation::Insert,
        "test",
        "k1",
    );

    let format = MessageFormat::Json;
    let bytes = format.serialize(&event).expect("JSON serialize ok");

    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("Valid JSON output");
    assert_eq!(json["operation"], "insert");
    assert_eq!(json["collection"], "test");
    assert_eq!(json["key"], "k1");
}

// ============================================================================
// Offset Store Tests (Integration)
// ============================================================================

#[tokio::test]
async fn test_offset_store_roundtrip() {
    let store = MemoryOffsetStore::new();

    let offset = Offset::new("mysql_12345", 99999)
        .with_metadata("filename", "mysql-bin.000003")
        .with_metadata("gtid", "abc-def:42");

    store.store(&offset).await.expect("Should store offset");

    let retrieved = store
        .get("mysql_12345")
        .await
        .expect("Should get offset")
        .expect("Offset should exist");

    assert_eq!(retrieved.lsn, 99999);
    assert_eq!(
        retrieved.metadata.get("filename"),
        Some(&"mysql-bin.000003".to_string())
    );
    assert_eq!(
        retrieved.metadata.get("gtid"),
        Some(&"abc-def:42".to_string())
    );
}

#[tokio::test]
async fn test_offset_store_overwrite() {
    let store = MemoryOffsetStore::new();

    let offset1 = Offset::new("source_1", 100);
    store.store(&offset1).await.expect("Store ok");

    let offset2 = Offset::new("source_1", 200);
    store.store(&offset2).await.expect("Store ok");

    let retrieved = store
        .get("source_1")
        .await
        .expect("Get ok")
        .expect("Exists");
    assert_eq!(retrieved.lsn, 200); // Should be updated
}

// ============================================================================
// MySQL Connector Integration Tests
// ============================================================================

#[tokio::test]
async fn test_mysql_connector_lifecycle() {
    let config = MySqlConfig::new("mysql://localhost/test").with_server_id(99999);
    let offset_store = Arc::new(MemoryOffsetStore::new());

    let connector = MySqlConnector::new(config, offset_store)
        .await
        .expect("Should create connector");

    assert_eq!(connector.status(), SourceStatus::Created);
    assert_eq!(connector.mysql_config().server_id, 99999);

    // Test position tracking
    assert!(connector.current_position().await.is_none());
    connector
        .update_position(BinlogPosition::new("mysql-bin.000001", 4))
        .await;
    let pos = connector.current_position().await.expect("Position set");
    assert_eq!(pos.filename, "mysql-bin.000001");
    assert_eq!(pos.position, 4);

    // Test GTID tracking
    assert!(connector.current_gtid().await.is_none());
    connector
        .update_gtid("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee:1".to_string())
        .await;
    let gtid = connector.current_gtid().await.expect("GTID set");
    assert!(gtid.contains("aaaaaaaa"));
}

#[tokio::test]
async fn test_mysql_connector_to_change_event() {
    let config = MySqlConfig::new("mysql://localhost/test").with_server_id(100);
    let offset_store = Arc::new(MemoryOffsetStore::new());
    let connector = MySqlConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    // Test WriteRows conversion
    let table_map = TableMapEvent {
        table_id: 1,
        schema: "mydb".to_string(),
        table: "users".to_string(),
        columns: vec![],
    };

    let write_event = BinlogEvent::WriteRows(RowEvent {
        table_id: 1,
        event_type: RowEventType::Insert,
        table_map: Some(table_map.clone()),
        rows: vec![],
    });

    let change = connector.to_change_event(write_event).await;
    assert!(change.is_some());
    let ce = change.expect("change event exists");
    assert!(ce.is_insert());
    assert_eq!(ce.collection, "mydb.users");

    // Test Gtid event (should return None but update state)
    let gtid_event = BinlogEvent::Gtid {
        gtid: "test-gtid:5".to_string(),
    };
    let change = connector.to_change_event(gtid_event).await;
    assert!(change.is_none());
    assert_eq!(
        connector.current_gtid().await,
        Some("test-gtid:5".to_string())
    );

    // Test Rotate event (should return None but update position)
    let rotate_event = BinlogEvent::Rotate {
        filename: "mysql-bin.000002".to_string(),
        position: 4,
    };
    let change = connector.to_change_event(rotate_event).await;
    assert!(change.is_none());
    let pos = connector
        .current_position()
        .await
        .expect("position updated");
    assert_eq!(pos.filename, "mysql-bin.000002");
}

#[tokio::test]
async fn test_mysql_connector_process_data() {
    let config = MySqlConfig::new("mysql://localhost/test").with_server_id(200);
    let offset_store = Arc::new(MemoryOffsetStore::new());
    let connector = MySqlConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    let (tx, _rx) = tokio::sync::mpsc::channel(10);

    // Process data that is too short - should return error
    let result = connector.process_data(&[0u8; 5], &tx).await;
    assert!(result.is_err());

    // Process a valid XID event
    let mut data = Vec::new();
    data.extend_from_slice(&1000u32.to_le_bytes()); // timestamp
    data.push(16); // XidEvent
    data.extend_from_slice(&1u32.to_le_bytes()); // server_id
    data.extend_from_slice(&27u32.to_le_bytes()); // event_length
    data.extend_from_slice(&27u32.to_le_bytes()); // next_position
    data.extend_from_slice(&0u16.to_le_bytes()); // flags
    data.extend_from_slice(&99u64.to_le_bytes()); // xid

    let result = connector.process_data(&data, &tx).await;
    // XID events don't produce ChangeEvents, so count should be 0
    assert!(result.is_ok());
    assert_eq!(result.expect("ok"), 0);
}

#[tokio::test]
async fn test_mysql_connector_current_offset_gtid() {
    let config = MySqlConfig::new("mysql://localhost/test").with_server_id(300);
    let offset_store = Arc::new(MemoryOffsetStore::new());
    let connector = MySqlConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    // No offset initially
    let offset = connector.current_offset().await.expect("No error");
    assert!(offset.is_none());

    // Set GTID
    connector.update_gtid("my-gtid:10".to_string()).await;
    let offset = connector
        .current_offset()
        .await
        .expect("No error")
        .expect("Offset exists");
    assert_eq!(offset.metadata.get("gtid"), Some(&"my-gtid:10".to_string()));
}

#[tokio::test]
async fn test_mysql_connector_current_offset_position() {
    let config = MySqlConfig::new("mysql://localhost/test").with_server_id(400);
    let offset_store = Arc::new(MemoryOffsetStore::new());
    let connector = MySqlConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    connector
        .update_position(BinlogPosition::new("binlog.000005", 54321))
        .await;

    let offset = connector
        .current_offset()
        .await
        .expect("No error")
        .expect("Offset exists");
    assert_eq!(offset.lsn, 54321);
    assert_eq!(
        offset.metadata.get("filename"),
        Some(&"binlog.000005".to_string())
    );
}

// ============================================================================
// MongoDB Connector Integration Tests
// ============================================================================

#[tokio::test]
async fn test_mongodb_connector_lifecycle() {
    let config = MongoDbConfig::new("mongodb://localhost:27017").with_database("testdb");
    let offset_store = Arc::new(MemoryOffsetStore::new());

    let connector = MongoDbConnector::new(config, offset_store)
        .await
        .expect("Should create connector");

    assert_eq!(connector.status(), SourceStatus::Created);
    assert_eq!(
        connector.mongo_config().database,
        Some("testdb".to_string())
    );
}

#[tokio::test]
async fn test_mongodb_connector_resume_token_tracking() {
    let config = MongoDbConfig::new("mongodb://localhost:27017").with_database("testdb");
    let offset_store = Arc::new(MemoryOffsetStore::new());

    let connector = MongoDbConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    assert!(connector.resume_token().await.is_none());

    connector
        .update_resume_token(ResumeToken::new("resume_token_xyz"))
        .await;

    let token = connector.resume_token().await.expect("Token set");
    assert_eq!(token.as_str(), "resume_token_xyz");
}

#[tokio::test]
async fn test_mongodb_connector_to_change_event_insert() {
    let config = MongoDbConfig::new("mongodb://localhost:27017").with_database("testdb");
    let offset_store = Arc::new(MemoryOffsetStore::new());

    let connector = MongoDbConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    let mongo_event = MongoChangeEvent {
        id: ResumeToken::new("token_1"),
        operation_type: ChangeStreamOperation::Insert,
        cluster_time: None,
        ns: Namespace::new("testdb", "products"),
        document_key: Some(DocumentKey {
            id: serde_json::json!("prod_001"),
            extra: HashMap::new(),
        }),
        full_document: Some(serde_json::json!({
            "_id": "prod_001",
            "name": "Widget",
            "price": 9.99
        })),
        full_document_before_change: None,
        update_description: None,
        txn_number: None,
        lsid: None,
    };

    let change = connector.to_change_event(&mongo_event);
    assert!(change.is_some());

    let ce = change.expect("change event exists");
    assert!(ce.is_insert());
    assert_eq!(ce.collection, "testdb.products");
    assert_eq!(ce.key, "prod_001");
    assert!(ce.after.is_some());
}

#[tokio::test]
async fn test_mongodb_connector_to_change_event_delete() {
    let config = MongoDbConfig::new("mongodb://localhost:27017").with_database("testdb");
    let offset_store = Arc::new(MemoryOffsetStore::new());

    let connector = MongoDbConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    let mongo_event = MongoChangeEvent {
        id: ResumeToken::new("token_del"),
        operation_type: ChangeStreamOperation::Delete,
        cluster_time: None,
        ns: Namespace::new("testdb", "users"),
        document_key: Some(DocumentKey {
            id: serde_json::json!("user_42"),
            extra: HashMap::new(),
        }),
        full_document: None,
        full_document_before_change: Some(serde_json::json!({
            "_id": "user_42",
            "name": "Deleted User"
        })),
        update_description: None,
        txn_number: None,
        lsid: None,
    };

    let change = connector.to_change_event(&mongo_event);
    assert!(change.is_some());

    let ce = change.expect("change event exists");
    assert!(ce.is_delete());
    assert!(ce.before.is_some());
    assert!(ce.after.is_none());
}

#[tokio::test]
async fn test_mongodb_connector_skips_invalidate() {
    let config = MongoDbConfig::new("mongodb://localhost:27017").with_database("testdb");
    let offset_store = Arc::new(MemoryOffsetStore::new());

    let connector = MongoDbConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    let mongo_event = MongoChangeEvent {
        id: ResumeToken::new("token_inv"),
        operation_type: ChangeStreamOperation::Invalidate,
        cluster_time: None,
        ns: Namespace::new("testdb", "coll"),
        document_key: None,
        full_document: None,
        full_document_before_change: None,
        update_description: None,
        txn_number: None,
        lsid: None,
    };

    let change = connector.to_change_event(&mongo_event);
    assert!(change.is_none());
}

#[tokio::test]
async fn test_mongodb_connector_process_event() {
    let config = MongoDbConfig::new("mongodb://localhost:27017").with_database("testdb");
    let offset_store = Arc::new(MemoryOffsetStore::new());

    let connector = MongoDbConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);

    let mongo_event = MongoChangeEvent {
        id: ResumeToken::new("token_process"),
        operation_type: ChangeStreamOperation::Insert,
        cluster_time: None,
        ns: Namespace::new("testdb", "items"),
        document_key: Some(DocumentKey {
            id: serde_json::json!("item_1"),
            extra: HashMap::new(),
        }),
        full_document: Some(serde_json::json!({"name": "item"})),
        full_document_before_change: None,
        update_description: None,
        txn_number: None,
        lsid: None,
    };

    let processed = connector
        .process_event(mongo_event, &tx)
        .await
        .expect("Process ok");
    assert!(processed);

    // Resume token should be updated
    assert!(connector.resume_token().await.is_some());

    // Event should be in channel
    let received = rx.try_recv().expect("Should receive event");
    assert!(received.is_insert());
    assert_eq!(received.collection, "testdb.items");
}

#[tokio::test]
async fn test_mongodb_connector_vector_parsing() {
    let config = MongoDbConfig::new("mongodb://localhost:27017")
        .with_database("testdb")
        .with_collection(MongoCollectionConfig::new("vectors").with_vector_field("embedding"));
    let offset_store = Arc::new(MemoryOffsetStore::new());

    let connector = MongoDbConnector::new(config, offset_store)
        .await
        .expect("Connector created");

    let mongo_event = MongoChangeEvent {
        id: ResumeToken::new("token_vec"),
        operation_type: ChangeStreamOperation::Insert,
        cluster_time: None,
        ns: Namespace::new("testdb", "vectors"),
        document_key: Some(DocumentKey {
            id: serde_json::json!("v1"),
            extra: HashMap::new(),
        }),
        full_document: Some(serde_json::json!({
            "_id": "v1",
            "embedding": [1.0, 2.0, 3.0, 4.0],
            "label": "test"
        })),
        full_document_before_change: None,
        update_description: None,
        txn_number: None,
        lsid: None,
    };

    let change = connector.to_change_event(&mongo_event);
    assert!(change.is_some());

    let ce = change.expect("event exists");
    let after = ce.after.expect("after state");

    // The vector field should be extracted
    assert!(after.vector.is_some());
    assert_eq!(
        after.vector.as_ref().expect("vec"),
        &vec![1.0, 2.0, 3.0, 4.0]
    );
    // The label should be in metadata
    assert!(after.metadata.contains_key("label"));
}

// ============================================================================
// Pipeline Integration Tests (Connector -> Decoder -> Sink)
// ============================================================================

#[tokio::test]
async fn test_full_pipeline_mysql_to_webhook() {
    // 1. Create MySQL connector
    let mysql_config = MySqlConfig::new("mysql://localhost/test").with_server_id(500);
    let offset_store = Arc::new(MemoryOffsetStore::new());
    let connector = MySqlConnector::new(mysql_config, offset_store.clone())
        .await
        .expect("MySQL connector created");

    // 2. Create a binlog event (simulated)
    let table_map = TableMapEvent {
        table_id: 10,
        schema: "shop".to_string(),
        table: "products".to_string(),
        columns: vec![],
    };

    let binlog_event = BinlogEvent::WriteRows(RowEvent {
        table_id: 10,
        event_type: RowEventType::Insert,
        table_map: Some(table_map),
        rows: vec![],
    });

    // 3. Convert to ChangeEvent via connector
    let change_event = connector
        .to_change_event(binlog_event)
        .await
        .expect("Should convert to change event");
    assert!(change_event.is_insert());
    assert_eq!(change_event.collection, "shop.products");

    // 4. Send through webhook sink (dry-run)
    let webhook_config = WebhookConfig::new("https://hooks.example.com/cdc");
    let webhook_sink = WebhookSink::new_dry_run(webhook_config);

    let send_result = webhook_sink.send(change_event).await;
    assert!(send_result.is_ok());

    let stats = webhook_sink.stats();
    assert_eq!(stats.events_sent, 1);
    assert!(stats.bytes_sent > 0);
}

#[tokio::test]
async fn test_full_pipeline_mongodb_to_kafka() {
    // 1. Create MongoDB connector
    let mongo_config = MongoDbConfig::new("mongodb://localhost:27017").with_database("analytics");
    let offset_store = Arc::new(MemoryOffsetStore::new());
    let connector = MongoDbConnector::new(mongo_config, offset_store.clone())
        .await
        .expect("MongoDB connector created");

    // 2. Create a MongoDB change event (simulated)
    let mongo_event = MongoChangeEvent {
        id: ResumeToken::new("pipeline_token"),
        operation_type: ChangeStreamOperation::Update,
        cluster_time: None,
        ns: Namespace::new("analytics", "metrics"),
        document_key: Some(DocumentKey {
            id: serde_json::json!("metric_001"),
            extra: HashMap::new(),
        }),
        full_document: Some(serde_json::json!({
            "_id": "metric_001",
            "value": 42.5,
            "timestamp": "2025-01-01T00:00:00Z"
        })),
        full_document_before_change: None,
        update_description: None,
        txn_number: None,
        lsid: None,
    };

    // 3. Convert to ChangeEvent via connector
    let change_event = connector
        .to_change_event(&mongo_event)
        .expect("Should convert to change event");
    assert!(change_event.is_update());
    assert_eq!(change_event.collection, "analytics.metrics");

    // 4. Send through Kafka sink (simulated)
    let kafka_config = KafkaConfig::new(vec!["broker:9092"]).with_topic_pattern("cdc.{collection}");
    let kafka_sink = KafkaSink::new(kafka_config);
    kafka_sink.connect().await.expect("Kafka connect ok");

    let send_result = kafka_sink.send(change_event).await;
    assert!(send_result.is_ok());

    let stats = kafka_sink.stats();
    assert_eq!(stats.events_sent, 1);

    kafka_sink.close().await.expect("Kafka close ok");
}

#[tokio::test]
async fn test_full_pipeline_batch_through_sinks() {
    // Create multiple events simulating a batch from MySQL binlog
    let events: Vec<ChangeEvent> = (0..20)
        .map(|i| {
            let op = match i % 3 {
                0 => Operation::Insert,
                1 => Operation::Update,
                _ => Operation::Delete,
            };
            ChangeEvent::new(
                SourceInfo::mysql("production", "server_1"),
                op,
                format!("db.table_{}", i % 5),
                format!("key_{}", i),
            )
        })
        .collect();

    // Send batch through webhook sink (dry-run)
    let webhook_config = WebhookConfig::new("https://api.example.com/cdc");
    let webhook = WebhookSink::new_dry_run(webhook_config);

    webhook
        .send_batch(events.clone())
        .await
        .expect("Webhook batch ok");
    assert_eq!(webhook.stats().events_sent, 20);

    // Send batch through Kafka sink (simulated)
    let kafka_config = KafkaConfig::new(vec!["localhost:9092"]);
    let kafka = KafkaSink::new(kafka_config);
    kafka.connect().await.expect("Kafka connect ok");

    kafka.send_batch(events).await.expect("Kafka batch ok");
    assert_eq!(kafka.stats().events_sent, 20);

    kafka.close().await.expect("Kafka close ok");
}

// ============================================================================
// Binlog Position and Config Tests
// ============================================================================

#[test]
fn test_binlog_position_parse_format() {
    let pos = BinlogPosition::new("mysql-bin.000005", 12345);
    assert_eq!(pos.format(), "mysql-bin.000005:12345");

    let parsed = BinlogPosition::parse("mysql-bin.000010:99999").expect("Should parse");
    assert_eq!(parsed.filename, "mysql-bin.000010");
    assert_eq!(parsed.position, 99999);

    assert!(BinlogPosition::parse("invalid_format").is_none());
    assert!(BinlogPosition::parse("file:not_a_number").is_none());
    assert!(BinlogPosition::parse("").is_none());
}

#[test]
fn test_mysql_config_validation() {
    // Empty URL should fail
    let config = MySqlConfig::default();
    assert!(config.validate().is_err());

    // Zero server ID should fail
    let config = MySqlConfig::new("mysql://localhost/db").with_server_id(0);
    assert!(config.validate().is_err());

    // Valid config
    let config = MySqlConfig::new("mysql://localhost/db").with_server_id(1);
    assert!(config.validate().is_ok());
}

#[test]
fn test_mongodb_config_validation() {
    // Empty URI should fail
    let config = MongoDbConfig::default();
    assert!(config.validate().is_err());

    // Valid config
    let config = MongoDbConfig::new("mongodb://localhost:27017");
    assert!(config.validate().is_ok());
}

#[test]
fn test_mongodb_should_capture() {
    let config = MongoDbConfig::new("mongodb://localhost")
        .with_collection(MongoCollectionConfig::new("users").with_database("app"))
        .with_collection(MongoCollectionConfig::new("*").with_database("logs"));

    assert!(config.should_capture("app", "users"));
    assert!(!config.should_capture("app", "orders"));
    assert!(config.should_capture("logs", "access"));
    assert!(config.should_capture("logs", "errors"));

    // Empty collections means capture all
    let config_all = MongoDbConfig::new("mongodb://localhost");
    assert!(config_all.should_capture("any_db", "any_collection"));
}

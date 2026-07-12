//! Query-facade tests, extracted from `query/mod.rs` when the facade moved to
//! `proximadb-observability-engine`. They construct the root
//! `ObservabilityStorage` (coerced to the port) and exercise the pub
//! `ObservabilityQueryEngine` methods exposed via the crate re-export.

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;

use crate::observability::ObservabilityStorage;
use crate::observability::query::{LogSearchOptions, ObservabilityQueryEngine};
use crate::proto::proximadb_v1::{LogEntry, Severity};

#[test]
fn test_parse_query() {
    let storage = Arc::new(ObservabilityStorage::new("/tmp/test"));
    let engine = ObservabilityQueryEngine::new(storage);

    let query = "service:api error connection";
    let parsed = engine.parse_query(query);

    assert_eq!(parsed.service_filter, Some("api".to_string()));
    assert!(parsed.text_search.contains(&"error".to_string()));
    assert!(parsed.text_search.contains(&"connection".to_string()));
}

#[test]
fn test_parse_severity() {
    let storage = Arc::new(ObservabilityStorage::new("/tmp/test"));
    let engine = ObservabilityQueryEngine::new(storage);

    assert_eq!(engine.parse_severity("debug"), Severity::Debug);
    assert_eq!(engine.parse_severity("ERROR"), Severity::Error);
    assert_eq!(engine.parse_severity("WARN"), Severity::Warn);
}

#[tokio::test]
async fn test_index_logs_for_search() {
    let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_index"));
    let engine = ObservabilityQueryEngine::new(storage);

    // Create logs to index
    let logs = vec![
        (
            "log_1".to_string(),
            LogEntry {
                timestamp_ns: 1000,
                severity: Severity::Error as i32,
                message: "Database connection timeout occurred".to_string(),
                service: Some("api-gateway".to_string()),
                source: Some("host1".to_string()),
                fields: HashMap::new(),
            },
        ),
        (
            "log_2".to_string(),
            LogEntry {
                timestamp_ns: 2000,
                severity: Severity::Info as i32,
                message: "Request completed successfully".to_string(),
                service: Some("web-server".to_string()),
                source: Some("host2".to_string()),
                fields: HashMap::new(),
            },
        ),
        (
            "log_3".to_string(),
            LogEntry {
                timestamp_ns: 3000,
                severity: Severity::Error as i32,
                message: "Connection refused by database".to_string(),
                service: Some("data-service".to_string()),
                source: Some("host3".to_string()),
                fields: HashMap::new(),
            },
        ),
    ];

    // Index the logs
    let indexed = engine.index_logs_for_search("test_ns", logs).await.unwrap();
    assert_eq!(indexed, 3);

    // Verify index stats
    let stats = engine.log_index_stats("test_ns").await.unwrap();
    assert_eq!(stats.doc_count, 3);
}

#[tokio::test]
async fn test_search_logs_fulltext() {
    let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_search"));
    let engine = ObservabilityQueryEngine::new(storage);

    // Create and index logs
    let logs = vec![
        (
            "log_1".to_string(),
            LogEntry {
                timestamp_ns: 1000,
                severity: Severity::Error as i32,
                message: "Database connection timeout error".to_string(),
                service: Some("api".to_string()),
                source: Some("host1".to_string()),
                fields: HashMap::new(),
            },
        ),
        (
            "log_2".to_string(),
            LogEntry {
                timestamp_ns: 2000,
                severity: Severity::Info as i32,
                message: "Request completed without issues".to_string(),
                service: Some("web".to_string()),
                source: Some("host2".to_string()),
                fields: HashMap::new(),
            },
        ),
        (
            "log_3".to_string(),
            LogEntry {
                timestamp_ns: 3000,
                severity: Severity::Error as i32,
                message: "Connection failed to database".to_string(),
                service: Some("data".to_string()),
                source: Some("host3".to_string()),
                fields: HashMap::new(),
            },
        ),
    ];

    engine
        .index_logs_for_search("search_ns", logs)
        .await
        .unwrap();

    // Search for "connection"
    let options = LogSearchOptions::with_limit(10);
    let results = engine
        .search_logs_fulltext("search_ns", "connection", &options)
        .await
        .unwrap();

    // Should find 2 logs containing "connection"
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn test_search_logs_phrase() {
    let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_phrase"));
    let engine = ObservabilityQueryEngine::new(storage);

    let logs = vec![
        (
            "log_1".to_string(),
            LogEntry {
                timestamp_ns: 1000,
                severity: Severity::Error as i32,
                message: "Connection timeout error in processing".to_string(),
                service: Some("api".to_string()),
                source: Some("host1".to_string()),
                fields: HashMap::new(),
            },
        ),
        (
            "log_2".to_string(),
            LogEntry {
                timestamp_ns: 2000,
                severity: Severity::Error as i32,
                message: "Error timeout connection".to_string(),
                service: Some("api".to_string()),
                source: Some("host2".to_string()),
                fields: HashMap::new(),
            },
        ),
    ];

    engine
        .index_logs_for_search("phrase_ns", logs)
        .await
        .unwrap();

    // Phrase search - exact phrase
    let options = LogSearchOptions::with_limit(10);
    let results = engine
        .search_logs_phrase("phrase_ns", "timeout error", &options)
        .await
        .unwrap();

    // Only log_1 has exact phrase "timeout error"
    assert!(!results.is_empty());
}

#[tokio::test]
async fn test_search_with_filters() {
    let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_filters"));
    let engine = ObservabilityQueryEngine::new(storage);

    let logs = vec![
        (
            "log_1".to_string(),
            LogEntry {
                timestamp_ns: 1000,
                severity: Severity::Error as i32,
                message: "Error in api processing".to_string(),
                service: Some("api".to_string()),
                source: Some("host1".to_string()),
                fields: HashMap::new(),
            },
        ),
        (
            "log_2".to_string(),
            LogEntry {
                timestamp_ns: 2000,
                severity: Severity::Warn as i32,
                message: "Warning in api layer".to_string(),
                service: Some("api".to_string()),
                source: Some("host2".to_string()),
                fields: HashMap::new(),
            },
        ),
        (
            "log_3".to_string(),
            LogEntry {
                timestamp_ns: 3000,
                severity: Severity::Error as i32,
                message: "Error in web service".to_string(),
                service: Some("web".to_string()),
                source: Some("host3".to_string()),
                fields: HashMap::new(),
            },
        ),
    ];

    engine
        .index_logs_for_search("filter_ns", logs)
        .await
        .unwrap();

    // Search with severity filter
    let options = LogSearchOptions::with_limit(10).severity(Severity::Error);
    let results = engine
        .search_logs_fulltext("filter_ns", "error", &options)
        .await
        .unwrap();

    // Should find only Error severity logs
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn test_log_index_stats() {
    let storage = Arc::new(ObservabilityStorage::new("/tmp/test_tantivy_stats"));
    let engine = ObservabilityQueryEngine::new(storage);

    // Initially empty
    let stats = engine.log_index_stats("stats_ns").await.unwrap();
    assert_eq!(stats.namespace, "stats_ns");
    assert_eq!(stats.doc_count, 0);

    // Add some logs
    let logs = vec![(
        "log_1".to_string(),
        LogEntry {
            timestamp_ns: 1000,
            severity: Severity::Info as i32,
            message: "Test message".to_string(),
            service: Some("api".to_string()),
            source: None,
            fields: HashMap::new(),
        },
    )];

    engine
        .index_logs_for_search("stats_ns", logs)
        .await
        .unwrap();

    let stats = engine.log_index_stats("stats_ns").await.unwrap();
    assert_eq!(stats.doc_count, 1);
}

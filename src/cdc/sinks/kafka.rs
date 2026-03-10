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

//! Kafka sink implementation

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::cdc::event::ChangeEvent;

use super::traits::{CdcSink, MessageFormat, SinkResult, SinkStats};

/// Kafka sink configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KafkaConfig {
    /// Bootstrap servers (comma-separated)
    pub bootstrap_servers: Vec<String>,
    /// Topic pattern (can include {collection} placeholder)
    pub topic_pattern: String,
    /// Security protocol
    #[serde(default)]
    pub security_protocol: SecurityProtocol,
    /// SASL mechanism (if using SASL)
    pub sasl_mechanism: Option<String>,
    /// SASL username
    pub sasl_username: Option<String>,
    /// SASL password
    pub sasl_password: Option<String>,
    /// Message key pattern (can include {key} placeholder)
    #[serde(default = "default_key_pattern")]
    pub key_pattern: String,
    /// Acknowledgment level
    #[serde(default)]
    pub acks: KafkaAcks,
    /// Compression type
    #[serde(default)]
    pub compression: KafkaCompression,
    /// Batch size
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Linger time in milliseconds
    #[serde(default = "default_linger_ms")]
    pub linger_ms: u64,
    /// Message format
    #[serde(default)]
    pub format: MessageFormat,
    /// Additional producer properties
    #[serde(default)]
    pub properties: HashMap<String, String>,
}

fn default_key_pattern() -> String {
    "{key}".to_string()
}

fn default_batch_size() -> usize {
    16384
}

fn default_linger_ms() -> u64 {
    5
}

/// Security protocol for Kafka
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SecurityProtocol {
    /// Plain text (no encryption)
    #[default]
    Plaintext,
    /// SSL encryption
    Ssl,
    /// SASL with plain text
    SaslPlaintext,
    /// SASL with SSL
    SaslSsl,
}

/// Kafka acknowledgment levels
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KafkaAcks {
    /// No acknowledgment (fire and forget)
    None,
    /// Leader acknowledgment
    Leader,
    /// All replicas acknowledgment
    #[default]
    All,
}

impl KafkaAcks {
    /// Convert to Kafka string value
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "0",
            Self::Leader => "1",
            Self::All => "all",
        }
    }
}

/// Kafka compression types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KafkaCompression {
    /// No compression
    #[default]
    None,
    /// Gzip compression
    Gzip,
    /// Snappy compression
    Snappy,
    /// LZ4 compression
    Lz4,
    /// Zstd compression
    Zstd,
}

impl KafkaCompression {
    /// Convert to Kafka string value
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Snappy => "snappy",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }
}

impl KafkaConfig {
    /// Create a new Kafka configuration
    pub fn new(bootstrap_servers: Vec<impl Into<String>>) -> Self {
        Self {
            bootstrap_servers: bootstrap_servers.into_iter().map(|s| s.into()).collect(),
            topic_pattern: "proximadb.{collection}".to_string(),
            security_protocol: SecurityProtocol::Plaintext,
            sasl_mechanism: None,
            sasl_username: None,
            sasl_password: None,
            key_pattern: default_key_pattern(),
            acks: KafkaAcks::All,
            compression: KafkaCompression::None,
            batch_size: default_batch_size(),
            linger_ms: default_linger_ms(),
            format: MessageFormat::Json,
            properties: HashMap::new(),
        }
    }

    /// Set the topic pattern
    pub fn with_topic_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.topic_pattern = pattern.into();
        self
    }

    /// Set the key pattern
    pub fn with_key_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.key_pattern = pattern.into();
        self
    }

    /// Set acknowledgment level
    pub fn with_acks(mut self, acks: KafkaAcks) -> Self {
        self.acks = acks;
        self
    }

    /// Set compression
    pub fn with_compression(mut self, compression: KafkaCompression) -> Self {
        self.compression = compression;
        self
    }

    /// Set batch size
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Set linger time
    pub fn with_linger_ms(mut self, ms: u64) -> Self {
        self.linger_ms = ms;
        self
    }

    /// Set message format
    pub fn with_format(mut self, format: MessageFormat) -> Self {
        self.format = format;
        self
    }

    /// Add SASL authentication
    pub fn with_sasl(
        mut self,
        mechanism: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.security_protocol = SecurityProtocol::SaslPlaintext;
        self.sasl_mechanism = Some(mechanism.into());
        self.sasl_username = Some(username.into());
        self.sasl_password = Some(password.into());
        self
    }

    /// Add SASL SSL authentication
    pub fn with_sasl_ssl(
        mut self,
        mechanism: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.security_protocol = SecurityProtocol::SaslSsl;
        self.sasl_mechanism = Some(mechanism.into());
        self.sasl_username = Some(username.into());
        self.sasl_password = Some(password.into());
        self
    }

    /// Set security protocol
    pub fn with_security_protocol(mut self, protocol: SecurityProtocol) -> Self {
        self.security_protocol = protocol;
        self
    }

    /// Add a custom property
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Get bootstrap servers as comma-separated string
    pub fn bootstrap_servers_string(&self) -> String {
        self.bootstrap_servers.join(",")
    }

    /// Resolve topic name for an event
    pub fn resolve_topic(&self, event: &ChangeEvent) -> String {
        self.topic_pattern
            .replace("{collection}", &event.collection)
            .replace("{database}", &event.source.database)
    }

    /// Resolve message key for an event
    pub fn resolve_key(&self, event: &ChangeEvent) -> String {
        self.key_pattern
            .replace("{key}", &event.key)
            .replace("{collection}", &event.collection)
    }
}

/// Kafka sink for CDC events
pub struct KafkaSink {
    /// Configuration
    config: KafkaConfig,
    /// Statistics
    stats: Mutex<SinkStats>,
    /// Buffer for batching
    buffer: Mutex<Vec<ChangeEvent>>,
    /// Connected flag
    connected: Mutex<bool>,
}

impl KafkaSink {
    /// Create a new Kafka sink
    pub fn new(config: KafkaConfig) -> Self {
        Self {
            config,
            stats: Mutex::new(SinkStats::default()),
            buffer: Mutex::new(Vec::new()),
            connected: Mutex::new(false),
        }
    }

    /// Get the configuration
    pub fn config(&self) -> &KafkaConfig {
        &self.config
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        *self
            .connected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Connect to Kafka (simulated)
    pub async fn connect(&self) -> SinkResult<()> {
        // In production, would create rdkafka producer here
        *self
            .connected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        Ok(())
    }

    /// Produce a message to Kafka (simulated)
    async fn produce(&self, topic: &str, key: &str, payload: &[u8]) -> SinkResult<()> {
        // Simulate Kafka produce
        let _ = (topic, key, payload);

        // Update stats
        let mut stats = self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.record_send(payload.len() as u64, 1.0);

        Ok(())
    }

    /// Produce a batch of messages
    async fn produce_batch(&self, messages: Vec<(String, String, Vec<u8>)>) -> SinkResult<()> {
        let total_bytes: u64 = messages.iter().map(|(_, _, p)| p.len() as u64).sum();
        let count = messages.len() as u64;

        // Simulate batch produce
        let _ = messages;

        // Update stats
        let mut stats = self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.record_batch(count, total_bytes, 5.0);

        Ok(())
    }
}

#[async_trait::async_trait]
impl CdcSink for KafkaSink {
    fn name(&self) -> &str {
        "kafka"
    }

    async fn send(&self, event: ChangeEvent) -> SinkResult<()> {
        let topic = self.config.resolve_topic(&event);
        let key = self.config.resolve_key(&event);
        let payload = self.config.format.serialize(&event)?;

        self.produce(&topic, &key, &payload).await
    }

    async fn send_batch(&self, events: Vec<ChangeEvent>) -> SinkResult<()> {
        let messages: Vec<(String, String, Vec<u8>)> = events
            .iter()
            .map(|event| {
                let topic = self.config.resolve_topic(event);
                let key = self.config.resolve_key(event);
                let payload = self.config.format.serialize(event).unwrap_or_default();
                (topic, key, payload)
            })
            .collect();

        self.produce_batch(messages).await
    }

    async fn flush(&self) -> SinkResult<()> {
        // Flush buffer
        let events = {
            let mut buffer = self
                .buffer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *buffer)
        };

        if !events.is_empty() {
            self.send_batch(events).await?;
        }

        Ok(())
    }

    async fn close(&self) -> SinkResult<()> {
        self.flush().await?;
        *self
            .connected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        Ok(())
    }

    fn stats(&self) -> SinkStats {
        self.stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::event::{Operation, SourceInfo};

    fn create_test_event() -> ChangeEvent {
        ChangeEvent::new(
            SourceInfo::postgres("testdb", "public", "test_server"),
            Operation::Insert,
            "public.users",
            "user_123",
        )
    }

    #[test]
    fn test_kafka_config_new() {
        let config = KafkaConfig::new(vec!["localhost:9092"]);
        assert_eq!(config.bootstrap_servers, vec!["localhost:9092"]);
        assert_eq!(config.topic_pattern, "proximadb.{collection}");
    }

    #[test]
    fn test_kafka_config_builder() {
        let config = KafkaConfig::new(vec!["broker1:9092", "broker2:9092"])
            .with_topic_pattern("events.{collection}")
            .with_acks(KafkaAcks::Leader)
            .with_compression(KafkaCompression::Lz4)
            .with_batch_size(32768)
            .with_linger_ms(10);

        assert_eq!(config.topic_pattern, "events.{collection}");
        assert!(matches!(config.acks, KafkaAcks::Leader));
        assert!(matches!(config.compression, KafkaCompression::Lz4));
        assert_eq!(config.batch_size, 32768);
        assert_eq!(config.linger_ms, 10);
    }

    #[test]
    fn test_kafka_config_sasl() {
        let config =
            KafkaConfig::new(vec!["localhost:9092"]).with_sasl("PLAIN", "user", "password");

        assert!(matches!(
            config.security_protocol,
            SecurityProtocol::SaslPlaintext
        ));
        assert_eq!(config.sasl_mechanism, Some("PLAIN".to_string()));
        assert_eq!(config.sasl_username, Some("user".to_string()));
    }

    #[test]
    fn test_resolve_topic() {
        let config =
            KafkaConfig::new(vec!["localhost:9092"]).with_topic_pattern("cdc.{collection}");

        let event = create_test_event();
        let topic = config.resolve_topic(&event);

        assert_eq!(topic, "cdc.public.users");
    }

    #[test]
    fn test_resolve_key() {
        let config =
            KafkaConfig::new(vec!["localhost:9092"]).with_key_pattern("{collection}:{key}");

        let event = create_test_event();
        let key = config.resolve_key(&event);

        assert_eq!(key, "public.users:user_123");
    }

    #[test]
    fn test_bootstrap_servers_string() {
        let config = KafkaConfig::new(vec!["broker1:9092", "broker2:9092", "broker3:9092"]);
        assert_eq!(
            config.bootstrap_servers_string(),
            "broker1:9092,broker2:9092,broker3:9092"
        );
    }

    #[test]
    fn test_kafka_acks_as_str() {
        assert_eq!(KafkaAcks::None.as_str(), "0");
        assert_eq!(KafkaAcks::Leader.as_str(), "1");
        assert_eq!(KafkaAcks::All.as_str(), "all");
    }

    #[test]
    fn test_kafka_compression_as_str() {
        assert_eq!(KafkaCompression::None.as_str(), "none");
        assert_eq!(KafkaCompression::Gzip.as_str(), "gzip");
        assert_eq!(KafkaCompression::Snappy.as_str(), "snappy");
        assert_eq!(KafkaCompression::Lz4.as_str(), "lz4");
        assert_eq!(KafkaCompression::Zstd.as_str(), "zstd");
    }

    #[tokio::test]
    async fn test_kafka_sink_creation() {
        let config = KafkaConfig::new(vec!["localhost:9092"]);
        let sink = KafkaSink::new(config);

        assert_eq!(sink.name(), "kafka");
        assert!(!sink.is_connected());
    }

    #[tokio::test]
    async fn test_kafka_sink_connect() {
        let config = KafkaConfig::new(vec!["localhost:9092"]);
        let sink = KafkaSink::new(config);

        sink.connect().await.unwrap();
        assert!(sink.is_connected());
    }

    #[tokio::test]
    async fn test_kafka_sink_send() {
        let config = KafkaConfig::new(vec!["localhost:9092"]);
        let sink = KafkaSink::new(config);
        sink.connect().await.unwrap();

        let event = create_test_event();
        sink.send(event).await.unwrap();

        let stats = sink.stats();
        assert_eq!(stats.events_sent, 1);
    }

    #[tokio::test]
    async fn test_kafka_sink_send_batch() {
        let config = KafkaConfig::new(vec!["localhost:9092"]);
        let sink = KafkaSink::new(config);
        sink.connect().await.unwrap();

        let events = vec![
            create_test_event(),
            create_test_event(),
            create_test_event(),
        ];
        sink.send_batch(events).await.unwrap();

        let stats = sink.stats();
        assert_eq!(stats.events_sent, 3);
        assert_eq!(stats.batches_sent, 1);
    }

    #[tokio::test]
    async fn test_kafka_sink_flush_and_close() {
        let config = KafkaConfig::new(vec!["localhost:9092"]);
        let sink = KafkaSink::new(config);
        sink.connect().await.unwrap();

        sink.flush().await.unwrap();
        sink.close().await.unwrap();

        assert!(!sink.is_connected());
    }

    #[test]
    fn test_kafka_config_with_property() {
        let config = KafkaConfig::new(vec!["localhost:9092"])
            .with_property("client.id", "my-producer")
            .with_property("request.timeout.ms", "30000");

        assert_eq!(
            config.properties.get("client.id"),
            Some(&"my-producer".to_string())
        );
        assert_eq!(
            config.properties.get("request.timeout.ms"),
            Some(&"30000".to_string())
        );
    }
}

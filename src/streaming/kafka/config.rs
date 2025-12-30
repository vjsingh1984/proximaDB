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

//! Kafka consumer configuration types

use std::time::Duration;
use serde::{Deserialize, Serialize};

/// Kafka consumer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KafkaConsumerConfig {
    /// Kafka broker addresses
    pub brokers: Vec<String>,
    /// Topic to consume from
    pub topic: String,
    /// Consumer group configuration
    pub group: ConsumerGroupConfig,
    /// Message deserialization format
    pub format: DeserializationFormat,
    /// Commit strategy
    pub commit_strategy: CommitStrategy,
    /// Dead letter queue configuration
    pub dlq: Option<DlqConfig>,
    /// Maximum messages to buffer
    pub max_buffer_size: usize,
    /// Polling timeout
    pub poll_timeout: Duration,
    /// Whether to start from earliest offset
    pub start_from_earliest: bool,
    /// Security configuration
    pub security: Option<KafkaSecurityConfig>,
    /// Target collection (can be overridden per message)
    pub default_collection: Option<String>,
    /// Number of consumer threads
    pub num_threads: usize,
    /// Enable message batching
    pub batch_size: usize,
    /// Batch timeout before flushing
    pub batch_timeout: Duration,
}

impl Default for KafkaConsumerConfig {
    fn default() -> Self {
        Self {
            brokers: vec!["localhost:9092".to_string()],
            topic: "vectors".to_string(),
            group: ConsumerGroupConfig::default(),
            format: DeserializationFormat::Json,
            commit_strategy: CommitStrategy::AtLeastOnce,
            dlq: None,
            max_buffer_size: 10_000,
            poll_timeout: Duration::from_millis(100),
            start_from_earliest: false,
            security: None,
            default_collection: None,
            num_threads: 1,
            batch_size: 100,
            batch_timeout: Duration::from_millis(50),
        }
    }
}

impl KafkaConsumerConfig {
    /// Create configuration for local development
    pub fn local(topic: &str, group_id: &str) -> Self {
        Self {
            topic: topic.to_string(),
            group: ConsumerGroupConfig {
                group_id: group_id.to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Create configuration with brokers
    pub fn with_brokers(brokers: Vec<String>, topic: &str, group_id: &str) -> Self {
        Self {
            brokers,
            topic: topic.to_string(),
            group: ConsumerGroupConfig {
                group_id: group_id.to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.brokers.is_empty() {
            return Err(ConfigError::InvalidConfig("No brokers specified".to_string()));
        }
        if self.topic.is_empty() {
            return Err(ConfigError::InvalidConfig("No topic specified".to_string()));
        }
        if self.group.group_id.is_empty() {
            return Err(ConfigError::InvalidConfig("No group ID specified".to_string()));
        }
        if self.batch_size == 0 {
            return Err(ConfigError::InvalidConfig("Batch size must be > 0".to_string()));
        }
        Ok(())
    }
}

/// Consumer group configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerGroupConfig {
    /// Consumer group ID
    pub group_id: String,
    /// Session timeout
    pub session_timeout: Duration,
    /// Heartbeat interval
    pub heartbeat_interval: Duration,
    /// Maximum poll interval
    pub max_poll_interval: Duration,
    /// Auto offset reset policy
    pub auto_offset_reset: AutoOffsetReset,
    /// Enable auto commit
    pub enable_auto_commit: bool,
    /// Auto commit interval (if enabled)
    pub auto_commit_interval: Duration,
}

impl Default for ConsumerGroupConfig {
    fn default() -> Self {
        Self {
            group_id: "proximadb-consumer".to_string(),
            session_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(3),
            max_poll_interval: Duration::from_secs(300),
            auto_offset_reset: AutoOffsetReset::Latest,
            enable_auto_commit: false,
            auto_commit_interval: Duration::from_secs(5),
        }
    }
}

/// Auto offset reset policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoOffsetReset {
    /// Start from earliest available offset
    Earliest,
    /// Start from latest offset
    Latest,
    /// Throw error if no offset is found
    Error,
}

/// Message deserialization format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeserializationFormat {
    /// JSON format
    Json,
    /// Apache Avro format (requires schema registry)
    Avro,
    /// Protocol Buffers format
    Protobuf,
    /// Raw bytes (vector only)
    Raw,
}

impl Default for DeserializationFormat {
    fn default() -> Self {
        Self::Json
    }
}

/// Commit strategy for offset management
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitStrategy {
    /// Commit after processing (may duplicate on failure)
    AtLeastOnce,
    /// Commit before processing (may lose on failure)
    AtMostOnce,
    /// Transactional commits (requires Kafka transactions)
    ExactlyOnce,
    /// Manual commits (application controlled)
    Manual,
}

impl Default for CommitStrategy {
    fn default() -> Self {
        Self::AtLeastOnce
    }
}

/// Dead letter queue configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqConfig {
    /// DLQ topic name
    pub topic: String,
    /// Maximum retries before sending to DLQ
    pub max_retries: u32,
    /// Retry delay
    pub retry_delay: Duration,
    /// Include original message in DLQ
    pub include_original: bool,
    /// Include error details in DLQ
    pub include_error: bool,
}

impl Default for DlqConfig {
    fn default() -> Self {
        Self {
            topic: "vectors-dlq".to_string(),
            max_retries: 3,
            retry_delay: Duration::from_secs(1),
            include_original: true,
            include_error: true,
        }
    }
}

/// Kafka security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KafkaSecurityConfig {
    /// Security protocol
    pub protocol: SecurityProtocol,
    /// SASL mechanism (if using SASL)
    pub sasl_mechanism: Option<SaslMechanism>,
    /// SASL username
    pub sasl_username: Option<String>,
    /// SASL password
    pub sasl_password: Option<String>,
    /// SSL CA certificate path
    pub ssl_ca_location: Option<String>,
    /// SSL certificate path
    pub ssl_certificate_location: Option<String>,
    /// SSL key path
    pub ssl_key_location: Option<String>,
    /// SSL key password
    pub ssl_key_password: Option<String>,
}

impl Default for KafkaSecurityConfig {
    fn default() -> Self {
        Self {
            protocol: SecurityProtocol::Plaintext,
            sasl_mechanism: None,
            sasl_username: None,
            sasl_password: None,
            ssl_ca_location: None,
            ssl_certificate_location: None,
            ssl_key_location: None,
            ssl_key_password: None,
        }
    }
}

/// Kafka security protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SecurityProtocol {
    /// No security
    Plaintext,
    /// SSL/TLS encryption
    Ssl,
    /// SASL authentication without encryption
    SaslPlaintext,
    /// SASL authentication with SSL/TLS
    SaslSsl,
}

/// SASL authentication mechanism
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SaslMechanism {
    /// Plain username/password
    Plain,
    /// SCRAM-SHA-256
    ScramSha256,
    /// SCRAM-SHA-512
    ScramSha512,
    /// OAuth bearer token
    OAuthBearer,
    /// AWS IAM
    AwsMskIam,
}

/// Configuration error
#[derive(Debug, Clone)]
pub enum ConfigError {
    /// Invalid configuration value
    InvalidConfig(String),
    /// Missing required field
    MissingField(String),
    /// Security configuration error
    SecurityError(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "Invalid configuration: {}", msg),
            Self::MissingField(field) => write!(f, "Missing required field: {}", field),
            Self::SecurityError(msg) => write!(f, "Security configuration error: {}", msg),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = KafkaConsumerConfig::default();
        assert_eq!(config.brokers, vec!["localhost:9092".to_string()]);
        assert_eq!(config.topic, "vectors");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_local_config() {
        let config = KafkaConsumerConfig::local("my-topic", "my-group");
        assert_eq!(config.topic, "my-topic");
        assert_eq!(config.group.group_id, "my-group");
    }

    #[test]
    fn test_config_validation_no_brokers() {
        let mut config = KafkaConsumerConfig::default();
        config.brokers = vec![];
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_no_topic() {
        let mut config = KafkaConsumerConfig::default();
        config.topic = String::new();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_consumer_group_defaults() {
        let group = ConsumerGroupConfig::default();
        assert_eq!(group.group_id, "proximadb-consumer");
        assert!(!group.enable_auto_commit);
    }

    #[test]
    fn test_dlq_defaults() {
        let dlq = DlqConfig::default();
        assert_eq!(dlq.topic, "vectors-dlq");
        assert_eq!(dlq.max_retries, 3);
    }

    #[test]
    fn test_config_serialization() {
        let config = KafkaConsumerConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: KafkaConsumerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.topic, config.topic);
    }
}

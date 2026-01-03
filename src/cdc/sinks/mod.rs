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

//! CDC Sinks
//!
//! This module provides sink implementations for delivering CDC events
//! to various destinations:
//!
//! - **Kafka**: High-throughput event streaming
//! - **Webhook**: HTTP-based event delivery
//! - **File**: Local file output for testing/debugging
//!
//! ## Example
//!
//! ```rust,ignore
//! use proximadb::cdc::sinks::{KafkaSink, KafkaConfig, CdcSink};
//!
//! let config = KafkaConfig::new(vec!["localhost:9092"])
//!     .with_topic_pattern("proximadb.{collection}");
//!
//! let sink = KafkaSink::new(config).await?;
//! sink.send(event).await?;
//! ```

mod kafka;
mod traits;
mod webhook;

pub use kafka::{KafkaAcks, KafkaCompression, KafkaConfig, KafkaSink};
pub use traits::RetryConfig;
pub use traits::{CdcSink, SinkConfig, SinkError, SinkResult, SinkStats};
pub use webhook::{HttpMethod, WebhookConfig, WebhookSink};

use crate::cdc::error::{CdcError, CdcResult};
use crate::cdc::event::ChangeEvent;

/// Sink factory for creating sinks from configuration
pub struct SinkFactory;

impl SinkFactory {
    /// Create a sink from configuration
    pub fn create(config: &SinkConfig) -> CdcResult<Box<dyn CdcSink>> {
        match config.sink_type.as_str() {
            "kafka" => {
                let kafka_config = config
                    .kafka
                    .as_ref()
                    .ok_or_else(|| CdcError::Configuration("Kafka config required".to_string()))?
                    .clone();
                Ok(Box::new(KafkaSink::new(kafka_config)))
            }
            "webhook" => {
                let webhook_config = config
                    .webhook
                    .as_ref()
                    .ok_or_else(|| CdcError::Configuration("Webhook config required".to_string()))?
                    .clone();
                Ok(Box::new(WebhookSink::new(webhook_config)))
            }
            other => Err(CdcError::Configuration(format!(
                "Unknown sink type: {}",
                other
            ))),
        }
    }
}

/// Multi-sink that writes to multiple destinations
pub struct MultiSink {
    sinks: Vec<Box<dyn CdcSink>>,
    mode: MultiSinkMode,
}

/// Mode for multi-sink delivery
#[derive(Debug, Clone, Copy, Default)]
pub enum MultiSinkMode {
    /// All sinks must succeed
    #[default]
    All,
    /// At least one sink must succeed
    AtLeastOne,
    /// Best effort - continue even if some fail
    BestEffort,
}

impl MultiSink {
    /// Create a new multi-sink
    pub fn new(sinks: Vec<Box<dyn CdcSink>>) -> Self {
        Self {
            sinks,
            mode: MultiSinkMode::All,
        }
    }

    /// Create with specific mode
    pub fn with_mode(mut self, mode: MultiSinkMode) -> Self {
        self.mode = mode;
        self
    }

    /// Add a sink
    pub fn add_sink(&mut self, sink: Box<dyn CdcSink>) {
        self.sinks.push(sink);
    }

    /// Get the number of sinks
    pub fn len(&self) -> usize {
        self.sinks.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

#[async_trait::async_trait]
impl CdcSink for MultiSink {
    fn name(&self) -> &str {
        "multi_sink"
    }

    async fn send(&self, event: ChangeEvent) -> SinkResult<()> {
        let mut successes = 0;
        let mut errors = Vec::new();

        for sink in &self.sinks {
            match sink.send(event.clone()).await {
                Ok(()) => successes += 1,
                Err(e) => errors.push((sink.name().to_string(), e)),
            }
        }

        match self.mode {
            MultiSinkMode::All => {
                if errors.is_empty() {
                    Ok(())
                } else {
                    let error_msg = errors
                        .iter()
                        .map(|(name, e)| format!("{}: {}", name, e))
                        .collect::<Vec<_>>()
                        .join("; ");
                    Err(SinkError::Send(error_msg))
                }
            }
            MultiSinkMode::AtLeastOne => {
                if successes > 0 {
                    Ok(())
                } else {
                    let error_msg = errors
                        .iter()
                        .map(|(name, e)| format!("{}: {}", name, e))
                        .collect::<Vec<_>>()
                        .join("; ");
                    Err(SinkError::Send(error_msg))
                }
            }
            MultiSinkMode::BestEffort => Ok(()),
        }
    }

    async fn send_batch(&self, events: Vec<ChangeEvent>) -> SinkResult<()> {
        let mut successes = 0;
        let mut errors = Vec::new();

        for sink in &self.sinks {
            match sink.send_batch(events.clone()).await {
                Ok(()) => successes += 1,
                Err(e) => errors.push((sink.name().to_string(), e)),
            }
        }

        match self.mode {
            MultiSinkMode::All => {
                if errors.is_empty() {
                    Ok(())
                } else {
                    let error_msg = errors
                        .iter()
                        .map(|(name, e)| format!("{}: {}", name, e))
                        .collect::<Vec<_>>()
                        .join("; ");
                    Err(SinkError::Send(error_msg))
                }
            }
            MultiSinkMode::AtLeastOne => {
                if successes > 0 {
                    Ok(())
                } else {
                    let error_msg = errors
                        .iter()
                        .map(|(name, e)| format!("{}: {}", name, e))
                        .collect::<Vec<_>>()
                        .join("; ");
                    Err(SinkError::Send(error_msg))
                }
            }
            MultiSinkMode::BestEffort => Ok(()),
        }
    }

    async fn flush(&self) -> SinkResult<()> {
        for sink in &self.sinks {
            sink.flush().await?;
        }
        Ok(())
    }

    async fn close(&self) -> SinkResult<()> {
        for sink in &self.sinks {
            sink.close().await?;
        }
        Ok(())
    }

    fn stats(&self) -> SinkStats {
        let mut combined = SinkStats::default();
        for sink in &self.sinks {
            let stats = sink.stats();
            combined.events_sent += stats.events_sent;
            combined.bytes_sent += stats.bytes_sent;
            combined.errors += stats.errors;
            combined.retries += stats.retries;
        }
        combined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockSink {
        name: String,
        should_fail: bool,
        stats: std::sync::Mutex<SinkStats>,
    }

    impl MockSink {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: false,
                stats: std::sync::Mutex::new(SinkStats::default()),
            }
        }

        fn failing(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: true,
                stats: std::sync::Mutex::new(SinkStats::default()),
            }
        }
    }

    #[async_trait::async_trait]
    impl CdcSink for MockSink {
        fn name(&self) -> &str {
            &self.name
        }

        async fn send(&self, _event: ChangeEvent) -> SinkResult<()> {
            if self.should_fail {
                Err(SinkError::Send("Mock failure".to_string()))
            } else {
                let mut stats = self.stats.lock().unwrap();
                stats.events_sent += 1;
                Ok(())
            }
        }

        async fn send_batch(&self, events: Vec<ChangeEvent>) -> SinkResult<()> {
            if self.should_fail {
                Err(SinkError::Send("Mock failure".to_string()))
            } else {
                let mut stats = self.stats.lock().unwrap();
                stats.events_sent += events.len() as u64;
                Ok(())
            }
        }

        async fn flush(&self) -> SinkResult<()> {
            Ok(())
        }

        async fn close(&self) -> SinkResult<()> {
            Ok(())
        }

        fn stats(&self) -> SinkStats {
            self.stats.lock().unwrap().clone()
        }
    }

    fn create_test_event() -> ChangeEvent {
        use crate::cdc::event::{Operation, SourceInfo};
        ChangeEvent::new(
            SourceInfo::postgres("testdb", "public", "test_server"),
            Operation::Insert,
            "public.users",
            "user_1",
        )
    }

    #[tokio::test]
    async fn test_multi_sink_all_success() {
        let multi = MultiSink::new(vec![
            Box::new(MockSink::new("sink1")),
            Box::new(MockSink::new("sink2")),
        ])
        .with_mode(MultiSinkMode::All);

        let event = create_test_event();
        let result = multi.send(event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multi_sink_all_fail() {
        let multi = MultiSink::new(vec![
            Box::new(MockSink::new("sink1")),
            Box::new(MockSink::failing("sink2")),
        ])
        .with_mode(MultiSinkMode::All);

        let event = create_test_event();
        let result = multi.send(event).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multi_sink_at_least_one() {
        let multi = MultiSink::new(vec![
            Box::new(MockSink::new("sink1")),
            Box::new(MockSink::failing("sink2")),
        ])
        .with_mode(MultiSinkMode::AtLeastOne);

        let event = create_test_event();
        let result = multi.send(event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multi_sink_best_effort() {
        let multi = MultiSink::new(vec![
            Box::new(MockSink::failing("sink1")),
            Box::new(MockSink::failing("sink2")),
        ])
        .with_mode(MultiSinkMode::BestEffort);

        let event = create_test_event();
        let result = multi.send(event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multi_sink_stats() {
        let multi = MultiSink::new(vec![
            Box::new(MockSink::new("sink1")),
            Box::new(MockSink::new("sink2")),
        ]);

        let event = create_test_event();
        multi.send(event).await.unwrap();

        let stats = multi.stats();
        assert_eq!(stats.events_sent, 2); // Sent to both sinks
    }

    #[tokio::test]
    async fn test_multi_sink_batch() {
        let multi = MultiSink::new(vec![
            Box::new(MockSink::new("sink1")),
            Box::new(MockSink::new("sink2")),
        ]);

        let events = vec![create_test_event(), create_test_event()];
        multi.send_batch(events).await.unwrap();

        let stats = multi.stats();
        assert_eq!(stats.events_sent, 4); // 2 events x 2 sinks
    }
}

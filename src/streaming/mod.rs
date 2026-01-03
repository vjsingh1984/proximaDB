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

//! # Real-time Streaming Module
//!
//! This module provides real-time vector streaming capabilities for ProximaDB,
//! enabling continuous vector ingestion and live query updates.
//!
//! ## Features
//!
//! - **High-throughput ingestion** via gRPC bidirectional streaming
//! - **Lock-free ring buffers** for efficient stream buffering
//! - **Watermark-based backpressure** for reliable operation under load
//! - **Live query subscriptions** with push notifications
//! - **Multiple protocol support** (gRPC, WebSocket, Kafka)
//!
//! ## Architecture
//!
//! ```text
//! Stream Ingress Layer
//! ├── gRPC Streaming
//! ├── WebSocket Handler
//! └── Kafka Consumer
//!         │
//!         ▼
//! Stream Coordinator (backpressure, ordering)
//!         │
//!         ▼
//! Ring Buffer (per stream, lock-free)
//!         │
//!         ▼
//! Stream Processor (embed, index, WAL)
//!         │
//!         ▼
//! Subscription Manager (live queries)
//! ```
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use proximadb::streaming::{StreamCoordinator, StreamConfig, SessionConfig};
//!
//! // Create coordinator with default config
//! let coordinator = StreamCoordinator::new(StreamConfig::default());
//!
//! // Create a streaming session
//! let session_id = coordinator.create_session(
//!     "my_collection".to_string(),
//!     SessionConfig::default(),
//! ).await?;
//!
//! // Push records (with backpressure handling)
//! let result = coordinator.push_records(&session_id, records).await?;
//! if result.backpressure != BackpressureLevel::None {
//!     // Slow down based on backpressure signal
//!     tokio::time::sleep(result.suggested_delay()).await;
//! }
//! ```

/// Error types for streaming operations
mod error;
pub use error::{StreamError, StreamResult};

/// Lock-free ring buffer for stream buffering
mod ring_buffer;
pub use ring_buffer::{BackpressureLevel, RingBuffer};

/// Watermark-based backpressure configuration
mod watermarks;
pub use watermarks::Watermarks;

/// Stream configuration types
mod config;
pub use config::{DeliverySemantics, OrderingMode, SessionConfig, StreamConfig};

/// Stream session management
mod session;
pub use session::{AckMessage, SessionState, SessionStats, StreamId, StreamSession};

/// Stream coordinator for managing multiple streams
mod coordinator;
pub use coordinator::{
    CoordinatorStats, FlushRetryConfig, FlushStats, PushResult, StreamCoordinator,
};

/// Prometheus metrics for streaming
mod metrics;
pub use metrics::StreamMetrics;

/// Rate limiting for stream ingestion
mod rate_limiter;
pub use rate_limiter::RateLimiter;

/// Integrated streaming service (coordinator + subscriptions)
mod integrated_service;
pub use integrated_service::{
    FlushAndNotifyResult, IntegratedServiceConfig, IntegratedServiceStats,
    IntegratedStreamingService, PushAndNotifyResult,
};

/// Live query subscription system
pub mod subscriptions;
pub use subscriptions::{
    EvaluationResult, QueryEvaluator, QueryFingerprint, QueryUpdate, ResultChange, ScoreChange,
    ScoredResult, Subscription, SubscriptionConfig, SubscriptionHandle, SubscriptionId,
    SubscriptionManager, SubscriptionState, UpdateType,
};

/// Kafka consumer integration
pub mod kafka;
pub use kafka::{
    CommitStrategy, ConsumerGroupConfig, ConsumerHandle, ConsumerStatus, DeserializationError,
    DeserializationFormat, DlqConfig, KafkaConsumerConfig, KafkaVectorConsumer,
    MessageDeserializer, VectorMessage,
};

#[cfg(test)]
mod tests;

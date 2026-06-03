// Streaming gRPC backend — `StreamingPort` implementation (session management).
//
// The tonic `StreamingService` wire adapter lives in
// `crates/platform/proximadb-api/src/grpc/v1/streaming.rs`. That adapter serves
// the unary session RPCs by delegating to the injected `StreamingPort` and
// leaves the server-streaming RPCs (`stream_insert`, `subscribe_query`,
// `batch_stream`) explicitly `unimplemented` by design.
//
// This file holds the canonical session-management logic behind that port.
// TD-105 Phase B lifted the session logic out of the former (dead, never-served)
// tonic `impl StreamingService` block and deleted that block — including its
// unreachable server-streaming implementations — so this type is now purely a
// port backend.

use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use crate::proto::proximadb_streaming_v1::{
    BackpressureLevel as ProtoBackpressureLevel, CloseSessionRequest, CloseSessionResponse,
    CreateSessionRequest, CreateSessionResponse, GetSessionStatusRequest, GetSessionStatusResponse,
    SessionConfig as ProtoSessionConfig, SessionState as ProtoSessionState,
    SessionStats as ProtoSessionStats,
};
use crate::streaming::{
    BackpressureLevel, SessionConfig, SessionState, StreamConfig, StreamCoordinator, StreamId,
};

/// Streaming session port backend — implements [`proximadb_runtime::StreamingPort`].
pub struct StreamingServiceImpl {
    /// Stream coordinator for managing sessions
    coordinator: Arc<StreamCoordinator>,
}

impl StreamingServiceImpl {
    /// Create a new StreamingServiceImpl with default configuration
    pub fn new() -> Self {
        Self {
            coordinator: Arc::new(StreamCoordinator::new(StreamConfig::default())),
        }
    }

    /// Create a new StreamingServiceImpl with custom configuration
    pub fn with_config(config: StreamConfig) -> Self {
        Self {
            coordinator: Arc::new(StreamCoordinator::new(config)),
        }
    }

    /// Create a new StreamingServiceImpl with an existing coordinator
    pub fn with_coordinator(coordinator: Arc<StreamCoordinator>) -> Self {
        Self { coordinator }
    }

    /// Get reference to the coordinator
    pub fn coordinator(&self) -> &Arc<StreamCoordinator> {
        &self.coordinator
    }
}

impl Default for StreamingServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert internal backpressure level to proto
fn backpressure_to_proto(level: BackpressureLevel) -> i32 {
    match level {
        BackpressureLevel::None => ProtoBackpressureLevel::None as i32,
        BackpressureLevel::Low => ProtoBackpressureLevel::Low as i32,
        BackpressureLevel::Medium => ProtoBackpressureLevel::Medium as i32,
        BackpressureLevel::High => ProtoBackpressureLevel::High as i32,
        BackpressureLevel::Critical => ProtoBackpressureLevel::Critical as i32,
    }
}

/// Convert internal session state to proto
fn session_state_to_proto(state: SessionState) -> i32 {
    match state {
        SessionState::Active => ProtoSessionState::Active as i32,
        SessionState::Paused => ProtoSessionState::Paused as i32,
        SessionState::Draining => ProtoSessionState::Draining as i32,
        SessionState::Closed => ProtoSessionState::Closed as i32,
        SessionState::Error => ProtoSessionState::Error as i32,
    }
}

/// Convert proto session config to internal config
fn proto_to_session_config(proto: Option<ProtoSessionConfig>) -> SessionConfig {
    match proto {
        Some(cfg) => SessionConfig {
            buffer_size: if cfg.buffer_size > 0 {
                Some(cfg.buffer_size as usize)
            } else {
                None
            },
            rate_limit: if cfg.rate_limit > 0 {
                Some(cfg.rate_limit)
            } else {
                None
            },
            flush_interval: if cfg.flush_interval_ms > 0 {
                Some(Duration::from_millis(cfg.flush_interval_ms as u64))
            } else {
                None
            },
            ..Default::default()
        },
        None => SessionConfig::default(),
    }
}

#[async_trait::async_trait]
impl proximadb_runtime::StreamingPort for StreamingServiceImpl {
    async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> anyhow::Result<CreateSessionResponse> {
        let config = proto_to_session_config(request.config);

        match self
            .coordinator
            .create_session(request.collection.clone(), config)
            .await
        {
            Ok(session_id) => {
                let info = self.coordinator.get_session_info(&session_id);
                let buffer_size = info.as_ref().map_or(0, |s| s.buffer_capacity as u32);

                info!(
                    session_id = %session_id,
                    collection = %request.collection,
                    buffer_size = buffer_size,
                    "Created explicit streaming session"
                );

                Ok(CreateSessionResponse {
                    session_id: session_id.to_string(),
                    buffer_size,
                    expires_in_seconds: self.coordinator.config().session_timeout.as_secs() as u32,
                })
            }
            Err(e) => Err(anyhow::anyhow!("Failed to create session: {}", e)),
        }
    }

    async fn close_session(
        &self,
        request: CloseSessionRequest,
    ) -> anyhow::Result<CloseSessionResponse> {
        let session_id = StreamId::from_string(request.session_id.clone());

        // Get stats before closing
        let stats = self.coordinator.get_session_info(&session_id);

        // Drain if requested
        let drain_complete = if request.drain {
            // Drain all remaining records
            loop {
                match self.coordinator.drain_records(&session_id, 1000) {
                    Ok(records) if records.is_empty() => break true,
                    Ok(_) => continue,
                    Err(_) => break false,
                }
            }
        } else {
            true
        };

        // Close the session
        self.coordinator.close_session(&session_id);

        info!(session_id = %request.session_id, drain_complete = drain_complete, "Closed streaming session");

        Ok(CloseSessionResponse {
            stats: stats.map(|s| ProtoSessionStats {
                records_received: s.records_received,
                records_processed: s.records_processed,
                records_dropped: 0, // Not tracked per session
                buffer_length: s.buffer_len as u32,
                buffer_capacity: s.buffer_capacity as u32,
                age_seconds: s.age_secs,
                idle_seconds: s.idle_secs,
                avg_latency_us: 0, // Deferred: Track per session
            }),
            drain_complete,
        })
    }

    async fn get_session_status(
        &self,
        request: GetSessionStatusRequest,
    ) -> anyhow::Result<GetSessionStatusResponse> {
        let session_id = StreamId::from_string(request.session_id.clone());

        match self.coordinator.get_session_info(&session_id) {
            Some(info) => Ok(GetSessionStatusResponse {
                session_id: request.session_id,
                collection: info.collection,
                state: session_state_to_proto(info.state),
                stats: Some(ProtoSessionStats {
                    records_received: info.records_received,
                    records_processed: info.records_processed,
                    records_dropped: 0,
                    buffer_length: info.buffer_len as u32,
                    buffer_capacity: info.buffer_capacity as u32,
                    age_seconds: info.age_secs,
                    idle_seconds: info.idle_secs,
                    avg_latency_us: 0,
                }),
                backpressure: backpressure_to_proto(BackpressureLevel::None), // Deferred: Get from session
            }),
            None => Err(anyhow::anyhow!("Session not found: {}", request.session_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_runtime::StreamingPort;

    #[tokio::test]
    async fn test_create_session() {
        let service = StreamingServiceImpl::new();

        let response = service
            .create_session(CreateSessionRequest {
                collection: "test_collection".to_string(),
                config: None,
                name: "test_session".to_string(),
            })
            .await
            .unwrap();

        assert!(!response.session_id.is_empty());
        assert!(response.buffer_size > 0);
        assert!(response.expires_in_seconds > 0);
    }

    #[tokio::test]
    async fn test_get_session_status() {
        let service = StreamingServiceImpl::new();

        // Create a session first
        let create_resp = service
            .create_session(CreateSessionRequest {
                collection: "test_collection".to_string(),
                config: None,
                name: "".to_string(),
            })
            .await
            .unwrap();
        let session_id = create_resp.session_id;

        // Get status
        let status = service
            .get_session_status(GetSessionStatusRequest {
                session_id: session_id.clone(),
            })
            .await
            .unwrap();

        assert_eq!(status.session_id, session_id);
        assert_eq!(status.collection, "test_collection");
        assert_eq!(status.state, ProtoSessionState::Active as i32);
    }

    #[test]
    fn test_streaming_config_defaults() {
        // Verify default StreamConfig values
        let config = StreamConfig::default();
        assert_eq!(config.max_streams, 1000);
        assert_eq!(config.default_buffer_size, 10_000);
        assert_eq!(config.global_rate_limit, 1_000_000);
        assert_eq!(config.flush_interval, Duration::from_millis(100));
        assert_eq!(config.session_timeout, Duration::from_secs(300));

        // Verify default BackpressureConfig values
        let bp = &config.backpressure;
        assert!((bp.low_watermark - 0.25).abs() < f32::EPSILON);
        assert!((bp.high_watermark - 0.75).abs() < f32::EPSILON);
        assert!((bp.critical_watermark - 0.90).abs() < f32::EPSILON);
        assert_eq!(bp.min_delay_ms, 10);
        assert_eq!(bp.max_delay_ms, 1000);
        assert!((bp.backoff_multiplier - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_streaming_message_serialization() {
        // Verify proto StreamInsertRequest round-trips through prost encoding
        use crate::proto::proximadb_streaming_v1::{BackpressureSignal, StreamInsertRequest};
        use prost::Message;

        let request = StreamInsertRequest {
            collection: "my_collection".to_string(),
            vectors: vec![],
            sequence: 42,
            partition_key: "pk_1".to_string(),
            session_id: "sess_abc".to_string(),
        };

        // Encode to bytes
        let mut buf = Vec::new();
        request.encode(&mut buf).expect("Failed to encode");
        assert!(!buf.is_empty());

        // Decode back
        let decoded = StreamInsertRequest::decode(buf.as_slice()).expect("Failed to decode");
        assert_eq!(decoded.collection, "my_collection");
        assert_eq!(decoded.sequence, 42);
        assert_eq!(decoded.partition_key, "pk_1");
        assert_eq!(decoded.session_id, "sess_abc");
        assert!(decoded.vectors.is_empty());

        // Verify BackpressureSignal serialization
        let signal = BackpressureSignal {
            level: ProtoBackpressureLevel::Medium as i32,
            suggested_delay_ms: 100,
            buffer_percent: 75,
            drain_time_ms: 500,
        };

        let mut buf2 = Vec::new();
        signal.encode(&mut buf2).expect("Failed to encode signal");
        let decoded_signal =
            BackpressureSignal::decode(buf2.as_slice()).expect("Failed to decode signal");
        assert_eq!(decoded_signal.level, ProtoBackpressureLevel::Medium as i32);
        assert_eq!(decoded_signal.suggested_delay_ms, 100);
        assert_eq!(decoded_signal.buffer_percent, 75);
        assert_eq!(decoded_signal.drain_time_ms, 500);
    }

    #[test]
    fn test_subscription_creation() {
        // Verify SubscriptionConfig construction and defaults
        use crate::streaming::subscriptions::SubscriptionConfig;

        let default_config = SubscriptionConfig::default();
        assert_eq!(default_config.top_k, 10);
        assert!((default_config.score_threshold - 0.0).abs() < f32::EPSILON);
        assert!(default_config.include_initial);
        assert_eq!(default_config.debounce_ms, 50);
        assert_eq!(default_config.max_delay_ms, 200);
        assert!(default_config.track_positions);

        // Verify custom config
        let custom_config = SubscriptionConfig {
            top_k: 50,
            score_threshold: 0.8,
            include_initial: false,
            debounce_ms: 100,
            max_delay_ms: 500,
            track_positions: false,
        };
        assert_eq!(custom_config.top_k, 50);
        assert!((custom_config.score_threshold - 0.8).abs() < f32::EPSILON);
        assert!(!custom_config.include_initial);
        assert_eq!(custom_config.debounce_ms, 100);
        assert_eq!(custom_config.max_delay_ms, 500);
        assert!(!custom_config.track_positions);
    }

    #[test]
    fn test_backpressure_config() {
        // Verify backpressure level to proto conversion
        assert_eq!(
            backpressure_to_proto(BackpressureLevel::None),
            ProtoBackpressureLevel::None as i32
        );
        assert_eq!(
            backpressure_to_proto(BackpressureLevel::Low),
            ProtoBackpressureLevel::Low as i32
        );
        assert_eq!(
            backpressure_to_proto(BackpressureLevel::Medium),
            ProtoBackpressureLevel::Medium as i32
        );
        assert_eq!(
            backpressure_to_proto(BackpressureLevel::High),
            ProtoBackpressureLevel::High as i32
        );
        assert_eq!(
            backpressure_to_proto(BackpressureLevel::Critical),
            ProtoBackpressureLevel::Critical as i32
        );

        // Verify session state to proto conversion
        assert_eq!(
            session_state_to_proto(SessionState::Active),
            ProtoSessionState::Active as i32
        );
        assert_eq!(
            session_state_to_proto(SessionState::Paused),
            ProtoSessionState::Paused as i32
        );
        assert_eq!(
            session_state_to_proto(SessionState::Draining),
            ProtoSessionState::Draining as i32
        );
        assert_eq!(
            session_state_to_proto(SessionState::Closed),
            ProtoSessionState::Closed as i32
        );
        assert_eq!(
            session_state_to_proto(SessionState::Error),
            ProtoSessionState::Error as i32
        );

        // Verify proto_to_session_config with None input yields defaults
        let default_session = proto_to_session_config(None);
        assert!(default_session.buffer_size.is_none());
        assert!(default_session.rate_limit.is_none());
        assert!(default_session.flush_interval.is_none());

        // Verify proto_to_session_config with custom values
        let proto_config = ProtoSessionConfig {
            buffer_size: 5000,
            rate_limit: 100_000,
            ordering: 0,
            delivery: 0,
            flush_interval_ms: 200,
            timeout_seconds: 0,
        };
        let session = proto_to_session_config(Some(proto_config));
        assert_eq!(session.buffer_size, Some(5000));
        assert_eq!(session.rate_limit, Some(100_000));
        assert_eq!(session.flush_interval, Some(Duration::from_millis(200)));
    }

    #[tokio::test]
    async fn test_close_session() {
        let service = StreamingServiceImpl::new();

        // Create a session first
        let create_resp = service
            .create_session(CreateSessionRequest {
                collection: "test_collection".to_string(),
                config: None,
                name: "".to_string(),
            })
            .await
            .unwrap();
        let session_id = create_resp.session_id;

        // Close the session
        let close_resp = service
            .close_session(CloseSessionRequest {
                session_id: session_id.clone(),
                drain: false,
            })
            .await
            .unwrap();
        assert!(close_resp.drain_complete);

        // Verify session is gone
        let status_result = service
            .get_session_status(GetSessionStatusRequest { session_id })
            .await;
        assert!(status_result.is_err());
    }
}

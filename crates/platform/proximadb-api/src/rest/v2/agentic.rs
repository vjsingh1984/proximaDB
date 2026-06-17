//! Agentic REST v2 request/response contracts.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Long-term memory item stored under a namespace/key pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMemoryItem {
    pub namespace: Vec<String>,
    pub key: String,
    pub value: Value,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
    pub score: Option<f32>,
}

/// Request body for `POST /api/v2/stores/{store}/items`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMemoryPutRequest {
    pub namespace: Vec<String>,
    pub key: String,
    pub value: Value,
    pub index: Option<Value>,
}

/// Query parameters/body for memory lookup and semantic search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMemorySearchRequest {
    pub namespace: Vec<String>,
    pub query: Option<String>,
    pub filter: Option<Value>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMemorySearchResponse {
    pub items: Vec<AgentMemoryItem>,
}

/// Request body for `POST /api/v2/checkpoints/{thread_id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCheckpointRequest {
    pub thread_id: String,
    pub checkpoint_ns: Option<String>,
    pub checkpoint_id: Option<String>,
    pub parent_checkpoint_id: Option<String>,
    pub checkpoint: Value,
    pub metadata: Value,
    pub writes: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCheckpointResponse {
    pub thread_id: String,
    pub checkpoint_ns: String,
    pub checkpoint_id: String,
    pub parent_checkpoint_id: Option<String>,
    pub checkpoint: Value,
    pub metadata: Value,
    pub writes: Vec<Value>,
}

/// Append-only event record for agent workflow audit/replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEventRecord {
    pub stream_id: String,
    pub version: u64,
    pub event_type: String,
    pub data: Value,
    pub metadata: Value,
    pub event_id: String,
    pub global_position: Option<u64>,
    pub created_at_ms: Option<i64>,
}

/// Request body for `POST /api/v2/events/{stream}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEventAppendRequest {
    pub stream_id: String,
    pub event_type: String,
    pub data: Value,
    pub metadata: Option<Value>,
    pub expected_version: Option<u64>,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEventAppendResponse {
    pub event: AgentEventRecord,
}

/// Query parameters/body for `GET /api/v2/events/{stream}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEventReplayRequest {
    pub stream_id: String,
    pub after_version: Option<u64>,
    pub after_position: Option<u64>,
    pub limit: Option<u32>,
}

/// Storage/service boundary used by REST v2 agentic handlers.
#[async_trait]
pub trait AgenticRestBackend: Send + Sync + 'static {
    async fn put_memory(
        &self,
        store: String,
        request: AgentMemoryPutRequest,
    ) -> Result<AgentMemoryItem, AgenticApiError>;

    async fn search_memory(
        &self,
        store: String,
        request: AgentMemorySearchRequest,
    ) -> Result<AgentMemorySearchResponse, AgenticApiError>;

    async fn put_checkpoint(
        &self,
        request: AgentCheckpointRequest,
    ) -> Result<AgentCheckpointResponse, AgenticApiError>;

    async fn append_event(
        &self,
        request: AgentEventAppendRequest,
    ) -> Result<AgentEventAppendResponse, AgenticApiError>;

    async fn replay_events(
        &self,
        request: AgentEventReplayRequest,
    ) -> Result<Vec<AgentEventRecord>, AgenticApiError>;
}

/// HTTP error envelope for agentic REST v2 handlers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgenticErrorBody {
    pub error: String,
    pub message: String,
}

/// Handler error with an explicit status code.
#[derive(Debug, Clone, PartialEq)]
pub struct AgenticApiError {
    pub status: StatusCode,
    pub error: String,
    pub message: String,
}

impl AgenticApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: "bad_request".to_string(),
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            error: "conflict".to_string(),
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: "internal".to_string(),
            message: message.into(),
        }
    }
}

impl IntoResponse for AgenticApiError {
    fn into_response(self) -> Response {
        // Canonical error envelope shared with `RestError`:
        // `{ "error": { "type", "message", "code", "request_id"? } }`.
        let mut error_obj = serde_json::json!({
            "type": self.error,
            "message": self.message,
            "code": self.status.as_u16(),
        });
        if let Some(rid) = crate::rest::errors::current_request_id() {
            error_obj["request_id"] = serde_json::json!(rid);
        }
        (self.status, Json(serde_json::json!({ "error": error_obj }))).into_response()
    }
}

pub type AgenticState<B> = State<Arc<B>>;
pub type AgenticResponse<T> = Result<Json<T>, AgenticApiError>;

/// `POST /api/v2/stores/{store}/items`
pub async fn put_memory<B: AgenticRestBackend>(
    State(backend): AgenticState<B>,
    Path(store): Path<String>,
    Json(request): Json<AgentMemoryPutRequest>,
) -> AgenticResponse<AgentMemoryItem> {
    validate_namespace(&request.namespace)?;
    if request.key.is_empty() {
        return Err(AgenticApiError::bad_request("memory key must not be empty"));
    }
    backend.put_memory(store, request).await.map(Json)
}

/// `POST /api/v2/stores/{store}/search`
pub async fn search_memory<B: AgenticRestBackend>(
    State(backend): AgenticState<B>,
    Path(store): Path<String>,
    Json(request): Json<AgentMemorySearchRequest>,
) -> AgenticResponse<AgentMemorySearchResponse> {
    validate_namespace(&request.namespace)?;
    backend.search_memory(store, request).await.map(Json)
}

/// `POST /api/v2/checkpoints/{thread_id}`
pub async fn put_checkpoint<B: AgenticRestBackend>(
    State(backend): AgenticState<B>,
    Path(thread_id): Path<String>,
    Json(mut request): Json<AgentCheckpointRequest>,
) -> AgenticResponse<AgentCheckpointResponse> {
    if request.thread_id.is_empty() {
        request.thread_id = thread_id;
    } else if request.thread_id != thread_id {
        return Err(AgenticApiError::bad_request(
            "checkpoint thread_id must match path",
        ));
    }
    backend.put_checkpoint(request).await.map(Json)
}

/// `POST /api/v2/events/{stream}`
pub async fn append_event<B: AgenticRestBackend>(
    State(backend): AgenticState<B>,
    Path(stream_id): Path<String>,
    Json(mut request): Json<AgentEventAppendRequest>,
) -> AgenticResponse<AgentEventAppendResponse> {
    if request.stream_id.is_empty() {
        request.stream_id = stream_id;
    } else if request.stream_id != stream_id {
        return Err(AgenticApiError::bad_request(
            "event stream_id must match path",
        ));
    }
    backend.append_event(request).await.map(Json)
}

/// `POST /api/v2/events/{stream}/replay`
pub async fn replay_events<B: AgenticRestBackend>(
    State(backend): AgenticState<B>,
    Path(stream_id): Path<String>,
    Json(mut request): Json<AgentEventReplayRequest>,
) -> AgenticResponse<Vec<AgentEventRecord>> {
    if request.stream_id.is_empty() {
        request.stream_id = stream_id;
    } else if request.stream_id != stream_id {
        return Err(AgenticApiError::bad_request(
            "event stream_id must match path",
        ));
    }
    backend.replay_events(request).await.map(Json)
}

fn validate_namespace(namespace: &[String]) -> Result<(), AgenticApiError> {
    if namespace.is_empty() {
        return Err(AgenticApiError::bad_request(
            "memory namespace must not be empty",
        ));
    }
    if namespace.iter().any(|part| part.is_empty()) {
        return Err(AgenticApiError::bad_request(
            "memory namespace parts must not be empty",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, State};

    #[test]
    fn agentic_rest_contracts_round_trip_json() {
        let put = AgentMemoryPutRequest {
            namespace: vec!["tenant".to_string(), "user".to_string()],
            key: "profile".to_string(),
            value: serde_json::json!({"role": "planner"}),
            index: Some(serde_json::json!(["role"])),
        };

        let encoded = serde_json::to_value(&put).unwrap();
        let decoded: AgentMemoryPutRequest = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, put);

        let append = AgentEventAppendRequest {
            stream_id: "thread-1".to_string(),
            event_type: "CheckpointCreated".to_string(),
            data: serde_json::json!({"checkpoint_id": "cp-1"}),
            metadata: Some(serde_json::json!({"source": "agent"})),
            expected_version: Some(0),
            event_id: None,
        };

        let encoded = serde_json::to_string(&append).unwrap();
        assert!(encoded.contains("CheckpointCreated"));
        let decoded: AgentEventAppendRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, append);
    }

    #[derive(Default)]
    struct MockBackend;

    #[async_trait]
    impl AgenticRestBackend for MockBackend {
        async fn put_memory(
            &self,
            store: String,
            request: AgentMemoryPutRequest,
        ) -> Result<AgentMemoryItem, AgenticApiError> {
            Ok(AgentMemoryItem {
                namespace: request.namespace,
                key: format!("{}:{}", store, request.key),
                value: request.value,
                created_at_ms: Some(1),
                updated_at_ms: Some(1),
                score: None,
            })
        }

        async fn search_memory(
            &self,
            store: String,
            request: AgentMemorySearchRequest,
        ) -> Result<AgentMemorySearchResponse, AgenticApiError> {
            Ok(AgentMemorySearchResponse {
                items: vec![AgentMemoryItem {
                    namespace: request.namespace,
                    key: store,
                    value: serde_json::json!({"matched": request.query}),
                    created_at_ms: None,
                    updated_at_ms: None,
                    score: Some(1.0),
                }],
            })
        }

        async fn put_checkpoint(
            &self,
            request: AgentCheckpointRequest,
        ) -> Result<AgentCheckpointResponse, AgenticApiError> {
            Ok(AgentCheckpointResponse {
                thread_id: request.thread_id,
                checkpoint_ns: request.checkpoint_ns.unwrap_or_default(),
                checkpoint_id: request.checkpoint_id.unwrap_or_else(|| "cp-1".to_string()),
                parent_checkpoint_id: request.parent_checkpoint_id,
                checkpoint: request.checkpoint,
                metadata: request.metadata,
                writes: request.writes,
            })
        }

        async fn append_event(
            &self,
            request: AgentEventAppendRequest,
        ) -> Result<AgentEventAppendResponse, AgenticApiError> {
            Ok(AgentEventAppendResponse {
                event: AgentEventRecord {
                    stream_id: request.stream_id,
                    version: request.expected_version.unwrap_or(0) + 1,
                    event_type: request.event_type,
                    data: request.data,
                    metadata: request.metadata.unwrap_or_else(|| serde_json::json!({})),
                    event_id: request.event_id.unwrap_or_else(|| "evt-1".to_string()),
                    global_position: Some(10),
                    created_at_ms: Some(1),
                },
            })
        }

        async fn replay_events(
            &self,
            request: AgentEventReplayRequest,
        ) -> Result<Vec<AgentEventRecord>, AgenticApiError> {
            Ok(vec![AgentEventRecord {
                stream_id: request.stream_id,
                version: request.after_version.unwrap_or(0) + 1,
                event_type: "Started".to_string(),
                data: serde_json::json!({}),
                metadata: serde_json::json!({}),
                event_id: "evt-1".to_string(),
                global_position: request.after_position.or(Some(1)),
                created_at_ms: Some(1),
            }])
        }
    }

    #[tokio::test]
    async fn agentic_rest_handlers_validate_path_and_delegate() {
        let backend = Arc::new(MockBackend);
        let memory = put_memory(
            State(backend.clone()),
            Path("default".to_string()),
            Json(AgentMemoryPutRequest {
                namespace: vec!["tenant".to_string()],
                key: "profile".to_string(),
                value: serde_json::json!({"role": "planner"}),
                index: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(memory.key, "default:profile");

        let checkpoint = put_checkpoint(
            State(backend.clone()),
            Path("thread-1".to_string()),
            Json(AgentCheckpointRequest {
                thread_id: String::new(),
                checkpoint_ns: None,
                checkpoint_id: None,
                parent_checkpoint_id: None,
                checkpoint: serde_json::json!({"id": "cp-1"}),
                metadata: serde_json::json!({}),
                writes: vec![],
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(checkpoint.thread_id, "thread-1");

        let event = append_event(
            State(backend.clone()),
            Path("stream-1".to_string()),
            Json(AgentEventAppendRequest {
                stream_id: String::new(),
                event_type: "Started".to_string(),
                data: serde_json::json!({}),
                metadata: None,
                expected_version: Some(0),
                event_id: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(event.event.stream_id, "stream-1");
        assert_eq!(event.event.version, 1);
    }

    #[tokio::test]
    async fn agentic_rest_handlers_reject_mismatched_path_ids() {
        let backend = Arc::new(MockBackend);
        let err = append_event(
            State(backend),
            Path("path-stream".to_string()),
            Json(AgentEventAppendRequest {
                stream_id: "body-stream".to_string(),
                event_type: "Started".to_string(),
                data: serde_json::json!({}),
                metadata: None,
                expected_version: None,
                event_id: None,
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.error, "bad_request");
    }
}

//! Agentic gRPC v2 service contracts.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tonic::Status;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryPutServiceRequest {
    pub store: String,
    pub namespace: Vec<String>,
    pub key: String,
    pub value: Value,
    pub index: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchServiceRequest {
    pub store: String,
    pub namespace: Vec<String>,
    pub query: Option<String>,
    pub filter: Option<Value>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchServiceResponse {
    pub items: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointServiceRequest {
    pub thread_id: String,
    pub checkpoint_ns: String,
    pub checkpoint_id: Option<String>,
    pub parent_checkpoint_id: Option<String>,
    pub checkpoint: Value,
    pub metadata: Value,
    pub writes: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointServiceResponse {
    pub checkpoint_id: String,
    pub config: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventAppendServiceRequest {
    pub stream_id: String,
    pub event_type: String,
    pub data: Value,
    pub metadata: Value,
    pub expected_version: Option<u64>,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventAppendServiceResponse {
    pub event_id: String,
    pub stream_id: String,
    pub version: u64,
    pub global_position: Option<u64>,
}

/// Backend boundary for native gRPC v2 agentic services.
#[async_trait]
pub trait AgenticGrpcBackend: Send + Sync + 'static {
    async fn put_memory(
        &self,
        request: MemoryPutServiceRequest,
    ) -> Result<MemoryPutServiceResponse, Status>;

    async fn search_memory(
        &self,
        request: MemorySearchServiceRequest,
    ) -> Result<MemorySearchServiceResponse, Status>;

    async fn put_checkpoint(
        &self,
        request: CheckpointServiceRequest,
    ) -> Result<CheckpointServiceResponse, Status>;

    async fn append_event(
        &self,
        request: EventAppendServiceRequest,
    ) -> Result<EventAppendServiceResponse, Status>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryPutServiceResponse {
    pub store: String,
    pub namespace: Vec<String>,
    pub key: String,
    pub value: Value,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
}

/// gRPC service wrapper for long-term memory operations.
#[derive(Clone)]
pub struct AgentMemoryService<B> {
    backend: Arc<B>,
}

/// gRPC service wrapper for checkpoint operations.
#[derive(Clone)]
pub struct AgentCheckpointService<B> {
    backend: Arc<B>,
}

/// gRPC service wrapper for event stream operations.
#[derive(Clone)]
pub struct AgentEventService<B> {
    backend: Arc<B>,
}

impl<B: AgenticGrpcBackend> AgentMemoryService<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }

    pub async fn put_memory(
        &self,
        request: MemoryPutServiceRequest,
    ) -> Result<MemoryPutServiceResponse, Status> {
        validate_namespace(&request.namespace)?;
        if request.key.is_empty() {
            return Err(Status::invalid_argument("memory key must not be empty"));
        }
        self.backend.put_memory(request).await
    }

    pub async fn search_memory(
        &self,
        request: MemorySearchServiceRequest,
    ) -> Result<MemorySearchServiceResponse, Status> {
        validate_namespace(&request.namespace)?;
        self.backend.search_memory(request).await
    }
}

impl<B: AgenticGrpcBackend> AgentCheckpointService<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }

    pub async fn put_checkpoint(
        &self,
        request: CheckpointServiceRequest,
    ) -> Result<CheckpointServiceResponse, Status> {
        if request.thread_id.is_empty() {
            return Err(Status::invalid_argument("thread_id must not be empty"));
        }
        self.backend.put_checkpoint(request).await
    }
}

impl<B: AgenticGrpcBackend> AgentEventService<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }

    pub async fn append_event(
        &self,
        request: EventAppendServiceRequest,
    ) -> Result<EventAppendServiceResponse, Status> {
        if request.stream_id.is_empty() {
            return Err(Status::invalid_argument("stream_id must not be empty"));
        }
        if request.event_type.is_empty() {
            return Err(Status::invalid_argument("event_type must not be empty"));
        }
        self.backend.append_event(request).await
    }
}

fn validate_namespace(namespace: &[String]) -> Result<(), Status> {
    if namespace.is_empty() {
        return Err(Status::invalid_argument(
            "memory namespace must not be empty",
        ));
    }
    if namespace.iter().any(|part| part.is_empty()) {
        return Err(Status::invalid_argument(
            "memory namespace parts must not be empty",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agentic_grpc_contracts_are_serializable_until_proto_codegen_lands() {
        let request = MemoryPutServiceRequest {
            store: "default".to_string(),
            namespace: vec!["tenant".to_string()],
            key: "profile".to_string(),
            value: serde_json::json!({"role": "planner"}),
            index: None,
        };

        let encoded = serde_json::to_value(&request).unwrap();
        let decoded: MemoryPutServiceRequest = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, request);
    }

    #[derive(Default)]
    struct MockBackend;

    #[async_trait]
    impl AgenticGrpcBackend for MockBackend {
        async fn put_memory(
            &self,
            request: MemoryPutServiceRequest,
        ) -> Result<MemoryPutServiceResponse, Status> {
            Ok(MemoryPutServiceResponse {
                store: request.store,
                namespace: request.namespace,
                key: request.key,
                value: request.value,
                created_at_ms: Some(1),
                updated_at_ms: Some(1),
            })
        }

        async fn search_memory(
            &self,
            request: MemorySearchServiceRequest,
        ) -> Result<MemorySearchServiceResponse, Status> {
            Ok(MemorySearchServiceResponse {
                items: vec![serde_json::json!({
                    "store": request.store,
                    "namespace": request.namespace,
                    "query": request.query,
                })],
            })
        }

        async fn put_checkpoint(
            &self,
            request: CheckpointServiceRequest,
        ) -> Result<CheckpointServiceResponse, Status> {
            Ok(CheckpointServiceResponse {
                checkpoint_id: request.checkpoint_id.unwrap_or_else(|| "cp-1".to_string()),
                config: serde_json::json!({
                    "configurable": {
                        "thread_id": request.thread_id,
                        "checkpoint_ns": request.checkpoint_ns,
                    }
                }),
            })
        }

        async fn append_event(
            &self,
            request: EventAppendServiceRequest,
        ) -> Result<EventAppendServiceResponse, Status> {
            Ok(EventAppendServiceResponse {
                event_id: request.event_id.unwrap_or_else(|| "evt-1".to_string()),
                stream_id: request.stream_id,
                version: request.expected_version.unwrap_or(0) + 1,
                global_position: Some(1),
            })
        }
    }

    #[tokio::test]
    async fn agentic_grpc_services_validate_and_delegate() {
        let backend = Arc::new(MockBackend);
        let memory = AgentMemoryService::new(backend.clone());
        let checkpoints = AgentCheckpointService::new(backend.clone());
        let events = AgentEventService::new(backend);

        let put = memory
            .put_memory(MemoryPutServiceRequest {
                store: "default".to_string(),
                namespace: vec!["tenant".to_string()],
                key: "profile".to_string(),
                value: serde_json::json!({"role": "planner"}),
                index: None,
            })
            .await
            .unwrap();
        assert_eq!(put.key, "profile");

        let checkpoint = checkpoints
            .put_checkpoint(CheckpointServiceRequest {
                thread_id: "thread-1".to_string(),
                checkpoint_ns: String::new(),
                checkpoint_id: None,
                parent_checkpoint_id: None,
                checkpoint: serde_json::json!({"id": "cp-1"}),
                metadata: serde_json::json!({}),
                writes: vec![],
            })
            .await
            .unwrap();
        assert_eq!(checkpoint.checkpoint_id, "cp-1");

        let event = events
            .append_event(EventAppendServiceRequest {
                stream_id: "stream-1".to_string(),
                event_type: "Started".to_string(),
                data: serde_json::json!({}),
                metadata: serde_json::json!({}),
                expected_version: Some(0),
                event_id: None,
            })
            .await
            .unwrap();
        assert_eq!(event.version, 1);
    }

    #[tokio::test]
    async fn agentic_grpc_services_reject_invalid_requests() {
        let service = AgentMemoryService::new(Arc::new(MockBackend));
        let err = service
            .put_memory(MemoryPutServiceRequest {
                store: "default".to_string(),
                namespace: vec![],
                key: "profile".to_string(),
                value: serde_json::json!({}),
                index: None,
            })
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }
}

//! Agentic gRPC v2 service contract placeholders.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// gRPC service marker for long-term memory operations.
pub struct AgentMemoryService;

/// gRPC service marker for checkpoint operations.
pub struct AgentCheckpointService;

/// gRPC service marker for event stream operations.
pub struct AgentEventService;

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

impl Default for AgentMemoryService {
    fn default() -> Self {
        Self
    }
}

impl Default for AgentCheckpointService {
    fn default() -> Self {
        Self
    }
}

impl Default for AgentEventService {
    fn default() -> Self {
        Self
    }
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

        let _memory = AgentMemoryService;
        let _checkpoints = AgentCheckpointService;
        let _events = AgentEventService;
    }
}

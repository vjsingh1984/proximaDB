//! Agentic REST v2 request/response contracts.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}

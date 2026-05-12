//! # gRPC API v2 Contracts
//!
//! Version 2 gRPC service contract placeholders for multimodal and agentic APIs.

pub mod agentic;

pub use agentic::{
    AgentCheckpointService, AgentEventService, AgentMemoryService, CheckpointServiceRequest,
    CheckpointServiceResponse, EventAppendServiceRequest, EventAppendServiceResponse,
    MemoryPutServiceRequest, MemorySearchServiceRequest, MemorySearchServiceResponse,
};

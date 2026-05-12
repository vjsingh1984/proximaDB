//! # gRPC API v2 Contracts
//!
//! Version 2 gRPC service contracts for multimodal and agentic APIs.

pub mod agentic;

pub use agentic::{
    AgentCheckpointService, AgentEventService, AgentMemoryService, AgenticGrpcBackend,
    CheckpointServiceRequest, CheckpointServiceResponse, EventAppendServiceRequest,
    EventAppendServiceResponse, MemoryPutServiceRequest, MemoryPutServiceResponse,
    MemorySearchServiceRequest, MemorySearchServiceResponse,
};

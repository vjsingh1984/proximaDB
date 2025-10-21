//! LLM Integration Module
//!
//! This module provides comprehensive LLM provider integration for ProximaDB's AI capabilities.
//! Implements the design specification from task_1_ai_implementation_design.adoc

pub mod engine;
pub mod metrics;
pub mod providers;
pub mod types;

pub use engine::LLMIntegrationEngine;
pub use metrics::LLMMetrics;
pub use types::{LLMConfig, LLMError, LLMProvider, LLMRequest, LLMResponse};

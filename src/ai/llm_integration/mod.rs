//! LLM Integration Module
//!
//! This module provides comprehensive LLM provider integration for ProximaDB's AI capabilities.
//! Implements the design specification from task_1_ai_implementation_design.adoc

pub mod engine;
pub mod providers;
pub mod metrics;
pub mod types;

pub use engine::LLMIntegrationEngine;
pub use types::{LLMRequest, LLMResponse, LLMError, LLMProvider, LLMConfig};
pub use metrics::LLMMetrics;
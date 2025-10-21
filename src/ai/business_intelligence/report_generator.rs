//! Report Generator
//!
//! Generates formatted business intelligence reports.

use crate::ai::llm_integration::LLMIntegrationEngine;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Report generator for business intelligence
pub struct ReportGenerator {
    llm_engine: Arc<LLMIntegrationEngine>,
}

/// Executive report structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutiveReport {
    pub title: String,
    pub content: String,
    pub format: ReportFormat,
    pub generated_at: chrono::DateTime<chrono::Utc>,
}

/// Report output formats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReportFormat {
    Html,
    Markdown,
    Json,
    PlainText,
}

impl ReportGenerator {
    pub async fn new(llm_engine: Arc<LLMIntegrationEngine>) -> Result<Self> {
        Ok(Self { llm_engine })
    }
}

impl Default for ReportFormat {
    fn default() -> Self {
        ReportFormat::Html
    }
}

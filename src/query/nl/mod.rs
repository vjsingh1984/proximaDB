//! # AV-SQL (TD-048) — 3-Agent Decomposition.
//!
//! Implementation of the multi-agent natural language query system
//! from arXiv:2604.07041.
//!
//! AV-SQL decomposes complex Text-to-SQL tasks into three specialized agents:
//! 1. **Rewriter Agent**: Clarifies and normalizes the input query.
//! 2. **ViewGenerator Agent**: Proposes a minimal schema subset (views) for the query.
//! 3. **Composer Agent**: Generates the final AQL or SQL query based on the views.

use crate::ai::llm_integration::LLMIntegrationEngine;
use crate::core::error::ProximaDBError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, ProximaDBError>;

// ---------------------------------------------------------------------------
// Agent Traits
// ---------------------------------------------------------------------------

#[async_trait]
pub trait AgentRewriter: Send + Sync {
    /// Rewrite a raw natural language query into a normalized form.
    async fn rewrite(&self, query: &str) -> Result<String>;
}

#[async_trait]
pub trait AgentViewGenerator: Send + Sync {
    /// Propose a set of views or schema subsets relevant to the query.
    async fn generate_views(&self, normalized_query: &str) -> Result<Vec<String>>;
}

#[async_trait]
pub trait AgentComposer: Send + Sync {
    /// Compose the final query (SQL or AQL) using the query and views.
    async fn compose(&self, normalized_query: &str, views: &[String]) -> Result<String>;
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Coordinates the 3-agent AV-SQL flow.
pub struct AvSqlEngine {
    rewriter: Arc<dyn AgentRewriter>,
    view_generator: Arc<dyn AgentViewGenerator>,
    composer: Arc<dyn AgentComposer>,
}

impl AvSqlEngine {
    pub fn new(
        rewriter: Arc<dyn AgentRewriter>,
        view_generator: Arc<dyn AgentViewGenerator>,
        composer: Arc<dyn AgentComposer>,
    ) -> Self {
        Self {
            rewriter,
            view_generator,
            composer,
        }
    }

    /// Execute the full 3-agent flow to translate text to a query.
    pub async fn translate(&self, text: &str) -> Result<AvSqlResult> {
        // 1. Rewrite
        let normalized = self.rewriter.rewrite(text).await?;

        // 2. View Generation
        let views = self.view_generator.generate_views(&normalized).await?;

        // 3. Composition
        let final_query = self.composer.compose(&normalized, &views).await?;

        Ok(AvSqlResult {
            normalized_query: normalized,
            views,
            final_query,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AvSqlResult {
    pub normalized_query: String,
    pub views: Vec<String>,
    pub final_query: String,
}

// ---------------------------------------------------------------------------
// LLM-backed Implementations (Skeletons)
// ---------------------------------------------------------------------------

pub struct LlmRewriter {
    llm: Arc<LLMIntegrationEngine>,
}

impl LlmRewriter {
    pub fn new(llm: Arc<LLMIntegrationEngine>) -> Self {
        Self { llm }
    }
}

#[async_trait]
impl AgentRewriter for LlmRewriter {
    async fn rewrite(&self, query: &str) -> Result<String> {
        let prompt = format!(
            "Rewrite this query for clarity and normalization: {}",
            query
        );
        let resp = self
            .llm
            .query_with_fallback(&prompt)
            .await
            .map_err(|e| ProximaDBError::Internal(e.to_string()))?;
        Ok(resp.content)
    }
}

pub struct LlmViewGenerator {
    llm: Arc<LLMIntegrationEngine>,
}

impl LlmViewGenerator {
    pub fn new(llm: Arc<LLMIntegrationEngine>) -> Self {
        Self { llm }
    }
}

#[async_trait]
impl AgentViewGenerator for LlmViewGenerator {
    async fn generate_views(&self, query: &str) -> Result<Vec<String>> {
        let prompt = format!(
            "Identify relevant schema tables or collections for: {}",
            query
        );
        let resp = self
            .llm
            .query_with_fallback(&prompt)
            .await
            .map_err(|e| ProximaDBError::Internal(e.to_string()))?;
        Ok(vec![resp.content]) // Simplified
    }
}

pub struct LlmComposer {
    llm: Arc<LLMIntegrationEngine>,
}

impl LlmComposer {
    pub fn new(llm: Arc<LLMIntegrationEngine>) -> Self {
        Self { llm }
    }
}

#[async_trait]
impl AgentComposer for LlmComposer {
    async fn compose(&self, query: &str, views: &[String]) -> Result<String> {
        let prompt = format!(
            "Compose an AQL query for '{}' using these views: {:?}",
            query, views
        );
        let resp = self
            .llm
            .query_with_fallback(&prompt)
            .await
            .map_err(|e| ProximaDBError::Internal(e.to_string()))?;
        Ok(resp.content)
    }
}

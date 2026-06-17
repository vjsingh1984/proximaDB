//! Agent-memory WRITE surface (TD-101, P2) — extraction + consolidation.
//!
//! Per the Convergence Gate (CLAUDE.md), this module introduces NO new storage
//! path or LLM abstraction. It converges on:
//!
//! * `LLMIntegrationEngine` (`src/ai/llm_integration/`) — reused for the
//!   extraction + consolidation completions (the same engine AV-SQL uses);
//! * `VectorOperationsService` — reused for top-k retrieval
//!   (`unified_search_v1`) and writes (`insert_batch_with_tenant_context`,
//!   `delete_records_with_tenant_context`);
//! * `ProximaRecord` — the canonical record a memory is stored as.
//!
//! See `ADR-022-agent-memory-layer` + `AGENT_MEMORY_LAYER_HLD_2026_05_30`.
//!
//! Architecture mirrors AV-SQL (`src/query/nl/mod.rs`): the orchestrator depends
//! on trait objects (`ExtractionAgent`, `ConsolidationAgent`, `MemoryStore`,
//! `MemoryEmbedder`) so the control flow is deterministically unit-testable with
//! mocks — the `LLMIntegrationEngine` itself has no trait seam, so the mock seam
//! lives at the agent layer. The error-prone LLM-response parsing is factored
//! into pure functions tested directly.
//!
//! Slice 1 lands the orchestration core + adapters + unit tests. The real
//! `MemoryEmbedder` model adapter, the `POST /api/v1/memory/ingest` REST route,
//! and consolidation-decision audit persistence are deferred (TD-101 sub-slice).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use proximadb_data_model::MemoryType;
use proximadb_embedding::{EmbedBatch, EmbedRecord, EmbeddingService, IngestMode};
use proximadb_records::{EmbeddingCell, ProximaRecord, ProximaTreeNode, ProximaValue};

use crate::ai::llm_integration::LLMIntegrationEngine;
use crate::core::search::{ComparisonOperator, FilterExpression};
use crate::services::VectorOperationsService;
use crate::services::audit_sink::{AuditEventSink, AuditRecord, EventLogAuditSink};
use crate::storage::engines::eventlog::EventLogEngine;

/// How many similar existing memories the consolidation step retrieves.
const CONSOLIDATION_TOP_K: usize = 5;
/// Embedding model tag stamped on written memory records.
const MEMORY_MODEL_ID: &str = "agent-memory";
const MEMORY_MODALITY: &str = "text";

// ---------------------------------------------------------------------------
// Value types
// ---------------------------------------------------------------------------

/// One turn pair (user + assistant) the extraction step distills facts from.
#[derive(Debug, Clone)]
pub struct MessagePair {
    pub user: String,
    pub assistant: String,
}

/// Scope a memory write is performed under.
#[derive(Debug, Clone)]
pub struct MemoryWriteScope {
    pub collection: String,
    pub tenant_id: String,
    pub actor: String,
    pub session_id: String,
}

/// A salient fact distilled from a message pair.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedFact {
    pub text: String,
    pub memory_type: MemoryType,
}

/// An existing memory surfaced during consolidation retrieval.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHit {
    pub id: String,
    pub text: String,
    pub score: f64,
    pub memory_type: Option<MemoryType>,
}

/// The consolidation decision for a single extracted fact.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsolidationAction {
    Add,
    Update { id: String },
    Delete { id: String },
    Noop,
}

/// Record of what the engine applied for one fact (for the response / audit).
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedAction {
    pub kind: &'static str,
    pub memory_id: Option<String>,
    pub fact_text: String,
}

// ---------------------------------------------------------------------------
// Trait seams (mockable)
// ---------------------------------------------------------------------------

#[async_trait]
pub trait ExtractionAgent: Send + Sync {
    async fn extract(&self, pair: &MessagePair) -> Result<Vec<ExtractedFact>>;
}

#[async_trait]
pub trait ConsolidationAgent: Send + Sync {
    async fn decide(
        &self,
        fact: &ExtractedFact,
        similar: &[MemoryHit],
    ) -> Result<ConsolidationAction>;
}

#[async_trait]
pub trait MemoryEmbedder: Send + Sync {
    /// Embed `text` for `tenant_id` (tenant drives per-tenant embed routing).
    async fn embed(&self, text: &str, tenant_id: &str) -> Result<Vec<f32>>;
}

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn retrieve_similar(
        &self,
        scope: &MemoryWriteScope,
        fact: &ExtractedFact,
        k: usize,
    ) -> Result<Vec<MemoryHit>>;
    async fn add(&self, scope: &MemoryWriteScope, fact: &ExtractedFact) -> Result<String>;
    async fn update(&self, scope: &MemoryWriteScope, id: &str, fact: &ExtractedFact) -> Result<()>;
    async fn delete(&self, scope: &MemoryWriteScope, id: &str) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Consolidation audit (the "why did this memory change?" trail)
// ---------------------------------------------------------------------------

/// A durable record of one consolidation decision. Captures the decision AND
/// the candidate set it was made against, so the memory mutation is explainable
/// after the fact (ADR-022 auditable memory). Serializable so it can be the
/// EventLog `Event.data` payload verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsolidationAuditEvent {
    pub collection: String,
    pub tenant_id: String,
    pub session_id: String,
    pub actor: String,
    pub fact_text: String,
    pub memory_type: Option<MemoryType>,
    /// "add" | "update" | "delete" | "noop".
    pub action: String,
    /// The memory id added/updated/deleted (None for noop).
    pub memory_id: Option<String>,
    /// Ids of the similar memories the decision was made against.
    pub similar_ids: Vec<String>,
    pub similar_count: usize,
}

/// Sink for consolidation-decision audit records. A trait seam (like the other
/// agent-memory ports) so tests can assert emission without a real EventLog.
#[async_trait]
pub trait ConsolidationAuditSink: Send + Sync {
    async fn emit(&self, event: &ConsolidationAuditEvent) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Drives extract → retrieve → consolidate → apply for an agent turn.
pub struct MemoryWriteEngine {
    extractor: Arc<dyn ExtractionAgent>,
    consolidator: Arc<dyn ConsolidationAgent>,
    store: Arc<dyn MemoryStore>,
    /// Optional durable audit of consolidation decisions. When `None`, ingest
    /// behaves exactly as before (decisions are still returned to the caller).
    audit_sink: Option<Arc<dyn ConsolidationAuditSink>>,
}

impl MemoryWriteEngine {
    pub fn new(
        extractor: Arc<dyn ExtractionAgent>,
        consolidator: Arc<dyn ConsolidationAgent>,
        store: Arc<dyn MemoryStore>,
    ) -> Self {
        Self {
            extractor,
            consolidator,
            store,
            audit_sink: None,
        }
    }

    /// Attach a sink that persists every consolidation decision (ADR-022
    /// auditable memory). Additive: omitting it leaves ingest unchanged.
    pub fn with_audit_sink(mut self, sink: Arc<dyn ConsolidationAuditSink>) -> Self {
        self.audit_sink = Some(sink);
        self
    }

    /// Ingest one message pair: extract facts, then for each fact retrieve
    /// similar memories, decide an action, and apply it.
    pub async fn ingest(
        &self,
        scope: &MemoryWriteScope,
        pair: &MessagePair,
    ) -> Result<Vec<AppliedAction>> {
        // Fail-closed: a memory write MUST be tenant-scoped. Never extract,
        // store, or audit under an empty tenant — that would write/retrieve
        // unscoped and risk cross-tenant leakage. The tenant is authoritative
        // (derived from the authenticated request context at the route), not a
        // caller-supplied body field.
        if scope.tenant_id.trim().is_empty() {
            return Err(anyhow!(
                "agent-memory write requires a non-empty tenant scope"
            ));
        }
        let facts = self.extractor.extract(pair).await?;
        let mut applied = Vec::with_capacity(facts.len());

        for fact in &facts {
            let similar = self
                .store
                .retrieve_similar(scope, fact, CONSOLIDATION_TOP_K)
                .await?;
            let action = self.consolidator.decide(fact, &similar).await?;
            let applied_action = match action {
                ConsolidationAction::Add => {
                    let id = self.store.add(scope, fact).await?;
                    AppliedAction {
                        kind: "add",
                        memory_id: Some(id),
                        fact_text: fact.text.clone(),
                    }
                }
                ConsolidationAction::Update { id } => {
                    self.store.update(scope, &id, fact).await?;
                    AppliedAction {
                        kind: "update",
                        memory_id: Some(id),
                        fact_text: fact.text.clone(),
                    }
                }
                ConsolidationAction::Delete { id } => {
                    self.store.delete(scope, &id).await?;
                    AppliedAction {
                        kind: "delete",
                        memory_id: Some(id),
                        fact_text: fact.text.clone(),
                    }
                }
                ConsolidationAction::Noop => AppliedAction {
                    kind: "noop",
                    memory_id: None,
                    fact_text: fact.text.clone(),
                },
            };

            // Persist the consolidation decision (non-fatal: an audit failure
            // never fails the write — mirrors the AQL audit path).
            if let Some(sink) = &self.audit_sink {
                let event = ConsolidationAuditEvent {
                    collection: scope.collection.clone(),
                    tenant_id: scope.tenant_id.clone(),
                    session_id: scope.session_id.clone(),
                    actor: scope.actor.clone(),
                    fact_text: fact.text.clone(),
                    memory_type: Some(fact.memory_type),
                    action: applied_action.kind.to_string(),
                    memory_id: applied_action.memory_id.clone(),
                    similar_ids: similar.iter().map(|h| h.id.clone()).collect(),
                    similar_count: similar.len(),
                };
                if let Err(e) = sink.emit(&event).await {
                    tracing::warn!("failed to persist consolidation audit: {e}");
                }
            }

            applied.push(applied_action);
        }

        Ok(applied)
    }
}

// ---------------------------------------------------------------------------
// Pure prompt-build + response-parse (unit-tested directly)
// ---------------------------------------------------------------------------

/// Canonical lowercase tag for a `MemoryType` (matches serde rename).
fn memory_type_tag(m: MemoryType) -> String {
    serde_json::to_value(m)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{m:?}").to_lowercase())
}

/// Parse a `MemoryType` from a lowercase tag; defaults to `Fact` when unknown.
fn memory_type_from_tag(tag: &str) -> MemoryType {
    serde_json::from_value(serde_json::Value::String(tag.trim().to_lowercase()))
        .unwrap_or(MemoryType::Fact)
}

/// Slice out the first balanced JSON value of the given open/close delimiters.
/// Tolerant of code fences / prose around the JSON the LLM emits.
fn slice_json(content: &str, open: char, close: char) -> Option<&str> {
    let start = content.find(open)?;
    let end = content.rfind(close)?;
    if end <= start {
        return None;
    }
    Some(&content[start..=end])
}

pub fn build_extraction_prompt(pair: &MessagePair) -> String {
    format!(
        "Extract durable, salient memories from this exchange. Return ONLY a JSON array of objects \
         with keys \"text\" (the memory, third-person, self-contained) and \"type\" (one of: fact, \
         preference, decision, commitment, goal, event, instruction, relationship, context, learning, \
         observation, error, artifact). Omit small talk.\n\nUser: {}\nAssistant: {}",
        pair.user, pair.assistant
    )
}

pub fn parse_extraction_response(content: &str) -> Vec<ExtractedFact> {
    let Some(json) = slice_json(content, '[', ']') else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|v| {
            let text = v.get("text")?.as_str()?.trim().to_string();
            if text.is_empty() {
                return None;
            }
            let memory_type = v
                .get("type")
                .and_then(|t| t.as_str())
                .map(memory_type_from_tag)
                .unwrap_or(MemoryType::Fact);
            Some(ExtractedFact { text, memory_type })
        })
        .collect()
}

pub fn build_consolidation_prompt(fact: &ExtractedFact, similar: &[MemoryHit]) -> String {
    let mut existing = String::new();
    for hit in similar {
        existing.push_str(&format!("- [{}] {}\n", hit.id, hit.text));
    }
    if existing.is_empty() {
        existing.push_str("(none)\n");
    }
    format!(
        "A new candidate memory was extracted. Decide how to reconcile it with existing memories. \
         Return ONLY a JSON object with key \"action\" (one of ADD, UPDATE, DELETE, NOOP) and, for \
         UPDATE/DELETE, key \"id\" naming the existing memory to act on.\n\n\
         Candidate ({}): {}\n\nExisting memories:\n{}",
        memory_type_tag(fact.memory_type),
        fact.text,
        existing
    )
}

pub fn parse_consolidation_response(content: &str) -> ConsolidationAction {
    let Some(json) = slice_json(content, '{', '}') else {
        return ConsolidationAction::Noop;
    };
    let Ok(obj) = serde_json::from_str::<serde_json::Value>(json) else {
        return ConsolidationAction::Noop;
    };
    let action = obj
        .get("action")
        .and_then(|a| a.as_str())
        .unwrap_or("NOOP")
        .trim()
        .to_ascii_uppercase();
    let id = obj
        .get("id")
        .and_then(|i| i.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    match action.as_str() {
        "ADD" => ConsolidationAction::Add,
        "UPDATE" => match id {
            Some(id) => ConsolidationAction::Update { id },
            None => ConsolidationAction::Noop,
        },
        "DELETE" => match id {
            Some(id) => ConsolidationAction::Delete { id },
            None => ConsolidationAction::Noop,
        },
        _ => ConsolidationAction::Noop,
    }
}

/// Build the canonical `ProximaRecord` for a memory. Pure (no I/O) so the field
/// mapping is unit-testable.
pub fn build_memory_record(
    scope: &MemoryWriteScope,
    fact: &ExtractedFact,
    embedding: Vec<f32>,
    oid: String,
) -> ProximaRecord {
    let dim = embedding.len() as u32;
    let mut props = HashMap::new();
    props.insert(
        "session_id".to_string(),
        ProximaTreeNode::Value(ProximaValue::String(scope.session_id.clone())),
    );
    // Mirror tenant_id into props so it is searchable through the flat
    // props-derived metadata filter path (TD-103). `ProximaRecord.tenant_id`
    // (set below) remains the RLS authority; this copy is search-time scoping.
    props.insert(
        "tenant_id".to_string(),
        ProximaTreeNode::Value(ProximaValue::String(scope.tenant_id.clone())),
    );
    props.insert(
        "text".to_string(),
        ProximaTreeNode::Value(ProximaValue::String(fact.text.clone())),
    );

    ProximaRecord {
        oid,
        tenant_id: scope.tenant_id.clone(),
        actor: Some(scope.actor.clone()),
        memory_type: Some(fact.memory_type),
        origin: Some("agent_memory".to_string()),
        method: Some("memory_write_surface".to_string()),
        props,
        embeddings: vec![EmbeddingCell::new_fp32(
            MEMORY_MODEL_ID,
            MEMORY_MODALITY,
            dim,
            embedding,
        )],
        ..ProximaRecord::default()
    }
}

// ---------------------------------------------------------------------------
// LLM-backed agents (real adapters; reuse LLMIntegrationEngine)
// ---------------------------------------------------------------------------

pub struct LlmExtractionAgent {
    llm: Arc<LLMIntegrationEngine>,
}

impl LlmExtractionAgent {
    pub fn new(llm: Arc<LLMIntegrationEngine>) -> Self {
        Self { llm }
    }
}

#[async_trait]
impl ExtractionAgent for LlmExtractionAgent {
    async fn extract(&self, pair: &MessagePair) -> Result<Vec<ExtractedFact>> {
        let prompt = build_extraction_prompt(pair);
        let resp = self
            .llm
            .query_with_fallback(&prompt)
            .await
            .map_err(|e| anyhow!("extraction LLM call failed: {e}"))?;
        Ok(parse_extraction_response(&resp.content))
    }
}

pub struct LlmConsolidationAgent {
    llm: Arc<LLMIntegrationEngine>,
}

impl LlmConsolidationAgent {
    pub fn new(llm: Arc<LLMIntegrationEngine>) -> Self {
        Self { llm }
    }
}

#[async_trait]
impl ConsolidationAgent for LlmConsolidationAgent {
    async fn decide(
        &self,
        fact: &ExtractedFact,
        similar: &[MemoryHit],
    ) -> Result<ConsolidationAction> {
        let prompt = build_consolidation_prompt(fact, similar);
        let resp = self
            .llm
            .query_with_fallback(&prompt)
            .await
            .map_err(|e| anyhow!("consolidation LLM call failed: {e}"))?;
        Ok(parse_consolidation_response(&resp.content))
    }
}

// ---------------------------------------------------------------------------
// Real store adapter (reuse VectorOperationsService)
// ---------------------------------------------------------------------------

/// Scoped tenant/session filter pushed into retrieval (mirrors the read
/// surface's `MemoryAqlSource::build_scope_filter`).
///
/// Field names are the FLAT metadata keys produced by flattening a record's
/// props (`proxima_tree_to_value_map`): a top-level prop `session_id` is the
/// metadata key `"session_id"`, NOT `"props.session_id"` (TD-103). `tenant_id`
/// is mirrored into props by `build_memory_record` so it resolves here too.
fn scope_filter(scope: &MemoryWriteScope) -> Option<FilterExpression> {
    let parts = vec![
        FilterExpression::Comparison {
            field: "tenant_id".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String(scope.tenant_id.clone()),
        },
        FilterExpression::Comparison {
            field: "session_id".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String(scope.session_id.clone()),
        },
    ];
    Some(FilterExpression::And(parts))
}

/// `MemoryStore` backed by the canonical vector write/search path. Embedding is
/// pluggable via `MemoryEmbedder` (no model wired in this slice).
pub struct VectorMemoryStore {
    vector_ops: Arc<VectorOperationsService>,
    embedder: Arc<dyn MemoryEmbedder>,
}

impl VectorMemoryStore {
    pub fn new(
        vector_ops: Arc<VectorOperationsService>,
        embedder: Arc<dyn MemoryEmbedder>,
    ) -> Self {
        Self {
            vector_ops,
            embedder,
        }
    }
}

#[async_trait]
impl MemoryStore for VectorMemoryStore {
    async fn retrieve_similar(
        &self,
        scope: &MemoryWriteScope,
        fact: &ExtractedFact,
        k: usize,
    ) -> Result<Vec<MemoryHit>> {
        let vector = self.embedder.embed(&fact.text, &scope.tenant_id).await?;
        let results = self
            .vector_ops
            .unified_search_v1(&scope.collection, vector, k, scope_filter(scope), None)
            .await
            .map_err(|e| anyhow!("memory retrieve failed: {e}"))?;

        let mut hits = Vec::new();
        if let Some(batch) = results.first() {
            for res in &batch.results {
                let text = res
                    .metadata
                    .get("text")
                    .and_then(|v| v.value.as_ref())
                    .and_then(sql_value_as_string)
                    .unwrap_or_default();
                let memory_type = res
                    .metadata
                    .get("memory_type")
                    .and_then(|v| v.value.as_ref())
                    .and_then(sql_value_as_string)
                    .map(|s| memory_type_from_tag(&s));
                hits.push(MemoryHit {
                    id: res.id.clone(),
                    text,
                    score: res.score,
                    memory_type,
                });
            }
        }
        Ok(hits)
    }

    async fn add(&self, scope: &MemoryWriteScope, fact: &ExtractedFact) -> Result<String> {
        let oid = uuid::Uuid::new_v4().to_string();
        self.write_record(scope, fact, oid.clone()).await?;
        Ok(oid)
    }

    async fn update(&self, scope: &MemoryWriteScope, id: &str, fact: &ExtractedFact) -> Result<()> {
        // Upsert by oid — `insert_batch_with_tenant_context` overwrites on match.
        self.write_record(scope, fact, id.to_string()).await
    }

    async fn delete(&self, scope: &MemoryWriteScope, id: &str) -> Result<()> {
        self.vector_ops
            .delete_records_with_tenant_context(&scope.collection, vec![id.to_string()], None)
            .await
            .map_err(|e| anyhow!("memory delete failed: {e}"))?;
        Ok(())
    }
}

impl VectorMemoryStore {
    async fn write_record(
        &self,
        scope: &MemoryWriteScope,
        fact: &ExtractedFact,
        oid: String,
    ) -> Result<()> {
        let embedding = self.embedder.embed(&fact.text, &scope.tenant_id).await?;
        let record = build_memory_record(scope, fact, embedding, oid);
        self.vector_ops
            .insert_batch_with_tenant_context(&scope.collection, vec![record], None)
            .await
            .map_err(|e| anyhow!("memory write failed: {e}"))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Real embedder adapter (reuse the in-process EmbeddingService)
// ---------------------------------------------------------------------------

/// Build a single-record `EmbedBatch` for `text` under `tenant_id`. Pure, so
/// the field mapping is unit-testable without the embedding singleton.
pub fn build_embed_batch(text: &str, tenant_id: &str) -> EmbedBatch {
    EmbedBatch {
        records: vec![EmbedRecord {
            id: "agent-memory".to_string(),
            text: text.to_string(),
            tenant_id: tenant_id.to_string(),
        }],
        mode: IngestMode::Async,
    }
}

/// `MemoryEmbedder` backed by the in-process `EmbeddingService` singleton
/// (`crates/modalities/proximadb-embedding`), the same engine the v2/Flight
/// ingest path uses (`ProximaFlightService::embed_text_only_records`).
#[derive(Default)]
pub struct EmbeddingServiceEmbedder;

#[async_trait]
impl MemoryEmbedder for EmbeddingServiceEmbedder {
    async fn embed(&self, text: &str, tenant_id: &str) -> Result<Vec<f32>> {
        let service = EmbeddingService::try_global()
            .ok_or_else(|| anyhow!("embedding service not initialized"))?;
        let result = service
            .embed_sync(build_embed_batch(text, tenant_id))
            .await
            .map_err(|e| anyhow!("memory embed failed: {e}"))?;
        result
            .vectors
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("embedder returned no vector"))
    }
}

// ---------------------------------------------------------------------------
// Real audit sink adapter (reuse EventLogEngine)
// ---------------------------------------------------------------------------

/// The per-session entity-id prefix for consolidation audit events:
/// `memory-consolidation:{tenant}:{session}:`. The trailing `:` makes it a true
/// session boundary — `consolidation_entity_prefix("acme", "s1")` does NOT
/// match a decision under session `s1b`. This is the SINGLE source of the
/// scheme: the write sink builds its entity-id as `{prefix}{memory_id|noop}`
/// and the read reader scans by `{prefix}`, so the two can never drift.
pub fn consolidation_entity_prefix(tenant_id: &str, session_id: &str) -> String {
    format!("memory-consolidation:{tenant_id}:{session_id}:")
}

/// `ConsolidationAuditSink` that maps each consolidation decision to a
/// domain-neutral [`AuditRecord`] and emits it through the shared
/// [`AuditEventSink`] foundation. The decision's tenant/session/memory scope
/// becomes the entity-id; the full `ConsolidationAuditEvent` is the payload, so
/// it is queryable + recoverable alongside every other audit event. The
/// `Event{}` construction itself lives once, in `EventLogAuditSink` — this
/// adapter holds only the domain → `AuditRecord` mapping.
pub struct EventLogConsolidationAuditSink {
    sink: Arc<dyn AuditEventSink>,
}

impl EventLogConsolidationAuditSink {
    /// Wrap any `AuditEventSink` (production: `EventLogAuditSink`).
    pub fn new(sink: Arc<dyn AuditEventSink>) -> Self {
        Self { sink }
    }

    /// Convenience: build directly over the shared `EventLogEngine`.
    pub fn from_event_log(log: Arc<EventLogEngine>) -> Self {
        Self {
            sink: Arc::new(EventLogAuditSink::new(log)),
        }
    }
}

#[async_trait]
impl ConsolidationAuditSink for EventLogConsolidationAuditSink {
    async fn emit(&self, event: &ConsolidationAuditEvent) -> Result<()> {
        let entity_id = format!(
            "{}{}",
            consolidation_entity_prefix(&event.tenant_id, &event.session_id),
            event.memory_id.as_deref().unwrap_or("noop"),
        );
        let data = serde_json::to_value(event)
            .map_err(|e| anyhow!("consolidation audit serialize failed: {e}"))?;
        let record = AuditRecord::new(entity_id, "MemoryConsolidationDecision", data)
            .with_causation(event.session_id.clone())
            .with_metadata_kv(
                "collection",
                serde_json::Value::String(event.collection.clone()),
            );
        self.sink
            .emit(record)
            .await
            .map_err(|e| anyhow!("consolidation audit append failed: {e}"))
    }
}

/// Read side of the consolidation audit trail: lists the ADD/UPDATE/DELETE/NOOP
/// decisions made for one `(tenant, session)`, closing the auditable-memory
/// loop (write → persist → read). Reuses the foundational
/// [`EventLogEngine::read_events_by_entity_prefix`] prefix scan over the SAME
/// entity-id scheme the sink writes (via [`consolidation_entity_prefix`]).
/// Deserialize is per-domain by design (the AQL read side does the same for its
/// own scheme) — a malformed/foreign payload under the prefix is skipped, not
/// fatal, so one bad record can't deny the whole session's history.
pub struct ConsolidationAuditReader {
    log: Arc<EventLogEngine>,
}

impl ConsolidationAuditReader {
    pub fn new(log: Arc<EventLogEngine>) -> Self {
        Self { log }
    }

    /// List a session's consolidation decisions in append (sequence) order.
    /// The `(tenant_id, session_id)` pair is the trust boundary: the caller MUST
    /// pass the authenticated tenant (never a client-supplied one) so the prefix
    /// scopes structurally to that tenant's events only.
    pub async fn list_session_decisions(
        &self,
        tenant_id: &str,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<ConsolidationAuditEvent>> {
        let prefix = consolidation_entity_prefix(tenant_id, session_id);
        let events = self
            .log
            .read_events_by_entity_prefix(&prefix, 0, limit)
            .await
            .map_err(|e| anyhow!("consolidation audit read failed: {e}"))?;
        let mut decisions = Vec::with_capacity(events.len());
        for event in events {
            match serde_json::from_value::<ConsolidationAuditEvent>(event.data) {
                Ok(decision) => decisions.push(decision),
                Err(e) => tracing::warn!(
                    sequence = event.sequence,
                    entity_id = %event.entity_id,
                    "skipping malformed consolidation audit payload: {e}"
                ),
            }
        }
        Ok(decisions)
    }
}

/// Best-effort extraction of a string from a proto `SqlValue`.
fn sql_value_as_string(val: &crate::proto::proximadb_v1::sql_value::Value) -> Option<String> {
    use crate::proto::proximadb_v1::sql_value::Value as V;
    match val {
        V::StringValue(s) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn scope() -> MemoryWriteScope {
        MemoryWriteScope {
            collection: "mem".to_string(),
            tenant_id: "acme".to_string(),
            actor: "assistant-1".to_string(),
            session_id: "sess-1".to_string(),
        }
    }

    fn fact(text: &str, mt: MemoryType) -> ExtractedFact {
        ExtractedFact {
            text: text.to_string(),
            memory_type: mt,
        }
    }

    struct MockExtractor(Vec<ExtractedFact>);
    #[async_trait]
    impl ExtractionAgent for MockExtractor {
        async fn extract(&self, _pair: &MessagePair) -> Result<Vec<ExtractedFact>> {
            Ok(self.0.clone())
        }
    }

    /// Returns a queued action per call, in order.
    struct MockConsolidator(Mutex<std::collections::VecDeque<ConsolidationAction>>);
    #[async_trait]
    impl ConsolidationAgent for MockConsolidator {
        async fn decide(
            &self,
            _fact: &ExtractedFact,
            _similar: &[MemoryHit],
        ) -> Result<ConsolidationAction> {
            Ok(self
                .0
                .lock()
                .map_err(|_| anyhow!("poisoned"))?
                .pop_front()
                .unwrap_or(ConsolidationAction::Noop))
        }
    }

    #[derive(Default)]
    struct RecordingStore {
        retrieves: Mutex<usize>,
        adds: Mutex<usize>,
        updates: Mutex<Vec<String>>,
        deletes: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl MemoryStore for RecordingStore {
        async fn retrieve_similar(
            &self,
            _scope: &MemoryWriteScope,
            _fact: &ExtractedFact,
            _k: usize,
        ) -> Result<Vec<MemoryHit>> {
            *self.retrieves.lock().map_err(|_| anyhow!("poisoned"))? += 1;
            Ok(Vec::new())
        }
        async fn add(&self, _scope: &MemoryWriteScope, _fact: &ExtractedFact) -> Result<String> {
            let mut a = self.adds.lock().map_err(|_| anyhow!("poisoned"))?;
            *a += 1;
            Ok(format!("new-{a}"))
        }
        async fn update(
            &self,
            _scope: &MemoryWriteScope,
            id: &str,
            _fact: &ExtractedFact,
        ) -> Result<()> {
            self.updates
                .lock()
                .map_err(|_| anyhow!("poisoned"))?
                .push(id.to_string());
            Ok(())
        }
        async fn delete(&self, _scope: &MemoryWriteScope, id: &str) -> Result<()> {
            self.deletes
                .lock()
                .map_err(|_| anyhow!("poisoned"))?
                .push(id.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn ingest_applies_action_per_fact_in_order() {
        let extractor = Arc::new(MockExtractor(vec![
            fact("user prefers dark mode", MemoryType::Preference),
            fact("user lives in NYC", MemoryType::Fact),
            fact("stale note", MemoryType::Observation),
        ]));
        let mut actions = std::collections::VecDeque::new();
        actions.push_back(ConsolidationAction::Add);
        actions.push_back(ConsolidationAction::Update {
            id: "m-42".to_string(),
        });
        actions.push_back(ConsolidationAction::Delete {
            id: "m-7".to_string(),
        });
        let consolidator = Arc::new(MockConsolidator(Mutex::new(actions)));
        let store = Arc::new(RecordingStore::default());

        let engine = MemoryWriteEngine::new(extractor, consolidator, store.clone());
        let applied = engine
            .ingest(
                &scope(),
                &MessagePair {
                    user: "hi".to_string(),
                    assistant: "hello".to_string(),
                },
            )
            .await
            .expect("ingest");

        assert_eq!(applied.len(), 3);
        assert_eq!(applied[0].kind, "add");
        assert_eq!(applied[1].kind, "update");
        assert_eq!(applied[1].memory_id.as_deref(), Some("m-42"));
        assert_eq!(applied[2].kind, "delete");
        assert_eq!(*store.retrieves.lock().unwrap(), 3, "retrieve per fact");
        assert_eq!(*store.adds.lock().unwrap(), 1);
        assert_eq!(store.updates.lock().unwrap().as_slice(), ["m-42"]);
        assert_eq!(store.deletes.lock().unwrap().as_slice(), ["m-7"]);
    }

    #[test]
    fn parse_extraction_valid_json() {
        let content = "Here you go:\n[{\"text\":\"user likes tea\",\"type\":\"preference\"},\
                       {\"text\":\"meeting at 3pm\",\"type\":\"event\"}]";
        let facts = parse_extraction_response(content);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].memory_type, MemoryType::Preference);
        assert_eq!(facts[1].text, "meeting at 3pm");
        assert_eq!(facts[1].memory_type, MemoryType::Event);
    }

    #[test]
    fn parse_extraction_unknown_type_defaults_fact_and_garbage_empty() {
        let facts = parse_extraction_response("[{\"text\":\"x\",\"type\":\"nonsense\"}]");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].memory_type, MemoryType::Fact);
        assert!(parse_extraction_response("not json at all").is_empty());
    }

    #[test]
    fn parse_consolidation_all_actions() {
        assert_eq!(
            parse_consolidation_response("{\"action\":\"ADD\"}"),
            ConsolidationAction::Add
        );
        assert_eq!(
            parse_consolidation_response("ok: {\"action\":\"update\",\"id\":\"m1\"}"),
            ConsolidationAction::Update {
                id: "m1".to_string()
            }
        );
        assert_eq!(
            parse_consolidation_response("{\"action\":\"DELETE\",\"id\":\"m2\"}"),
            ConsolidationAction::Delete {
                id: "m2".to_string()
            }
        );
        assert_eq!(
            parse_consolidation_response("{\"action\":\"NOOP\"}"),
            ConsolidationAction::Noop
        );
        // UPDATE without id degrades to Noop; garbage degrades to Noop.
        assert_eq!(
            parse_consolidation_response("{\"action\":\"UPDATE\"}"),
            ConsolidationAction::Noop
        );
        assert_eq!(
            parse_consolidation_response("garbage"),
            ConsolidationAction::Noop
        );
    }

    #[test]
    fn build_embed_batch_maps_text_and_tenant() {
        let batch = build_embed_batch("user likes tea", "acme");
        assert_eq!(batch.records.len(), 1);
        assert_eq!(batch.records[0].text, "user likes tea");
        assert_eq!(batch.records[0].tenant_id, "acme");
        assert!(matches!(batch.mode, IngestMode::Async));
    }

    #[test]
    fn build_memory_record_sets_scope_and_fields() {
        let rec = build_memory_record(
            &scope(),
            &fact("user lives in NYC", MemoryType::Fact),
            vec![0.1, 0.2, 0.3],
            "oid-1".to_string(),
        );
        assert_eq!(rec.oid, "oid-1");
        assert_eq!(rec.tenant_id, "acme");
        assert_eq!(rec.actor.as_deref(), Some("assistant-1"));
        assert_eq!(rec.memory_type, Some(MemoryType::Fact));
        assert_eq!(rec.embeddings.len(), 1);
        assert_eq!(rec.embeddings[0].dim, 3);
        match rec.props.get("session_id") {
            Some(ProximaTreeNode::Value(ProximaValue::String(s))) => assert_eq!(s, "sess-1"),
            other => panic!("session_id not set: {other:?}"),
        }
        // TD-103: tenant_id mirrored into props for search-time scoping.
        match rec.props.get("tenant_id") {
            Some(ProximaTreeNode::Value(ProximaValue::String(s))) => assert_eq!(s, "acme"),
            other => panic!("tenant_id not mirrored into props: {other:?}"),
        }
    }

    /// TD-103 regression: the scope filter must actually MATCH a record built by
    /// `build_memory_record`, evaluated against the record's real props tree via
    /// the canonical `evaluate_filter_proxima`. This is the end-to-end path the
    /// mock-based orchestrator tests bypassed — it FAILS on the old
    /// `"props.session_id"` field name and passes after the fix.
    #[test]
    fn scope_filter_matches_record_props_end_to_end() {
        use crate::core::search::sql_value_filter::evaluate_filter_proxima;

        let rec = build_memory_record(
            &scope(),
            &fact("user lives in NYC", MemoryType::Fact),
            vec![0.1, 0.2, 0.3],
            "oid-1".to_string(),
        );
        let filter = scope_filter(&scope()).expect("scope filter built");

        // Matching tenant+session → record is retained.
        assert!(
            evaluate_filter_proxima(&filter, &rec.props),
            "scope filter should match the record it scopes"
        );

        // Different session → filtered out.
        let other_session = MemoryWriteScope {
            session_id: "sess-OTHER".to_string(),
            ..scope()
        };
        assert!(
            !evaluate_filter_proxima(&scope_filter(&other_session).unwrap(), &rec.props),
            "different session must not match"
        );

        // Different tenant → filtered out.
        let other_tenant = MemoryWriteScope {
            tenant_id: "other-tenant".to_string(),
            ..scope()
        };
        assert!(
            !evaluate_filter_proxima(&scope_filter(&other_tenant).unwrap(), &rec.props),
            "different tenant must not match"
        );
    }

    // ── consolidation audit persistence ──────────────────────────────────

    /// Store whose `retrieve_similar` returns a fixed candidate set, so the
    /// audit event's `similar_ids`/`similar_count` are non-trivial.
    struct HitStore(Vec<MemoryHit>);
    #[async_trait]
    impl MemoryStore for HitStore {
        async fn retrieve_similar(
            &self,
            _scope: &MemoryWriteScope,
            _fact: &ExtractedFact,
            _k: usize,
        ) -> Result<Vec<MemoryHit>> {
            Ok(self.0.clone())
        }
        async fn add(&self, _scope: &MemoryWriteScope, _fact: &ExtractedFact) -> Result<String> {
            Ok("new-1".to_string())
        }
        async fn update(
            &self,
            _scope: &MemoryWriteScope,
            _id: &str,
            _fact: &ExtractedFact,
        ) -> Result<()> {
            Ok(())
        }
        async fn delete(&self, _scope: &MemoryWriteScope, _id: &str) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingAuditSink {
        emitted: Mutex<Vec<ConsolidationAuditEvent>>,
    }
    #[async_trait]
    impl ConsolidationAuditSink for RecordingAuditSink {
        async fn emit(&self, event: &ConsolidationAuditEvent) -> Result<()> {
            self.emitted
                .lock()
                .map_err(|_| anyhow!("poisoned"))?
                .push(event.clone());
            Ok(())
        }
    }

    struct FailingAuditSink;
    #[async_trait]
    impl ConsolidationAuditSink for FailingAuditSink {
        async fn emit(&self, _event: &ConsolidationAuditEvent) -> Result<()> {
            Err(anyhow!("synthetic audit failure"))
        }
    }

    fn hit(id: &str) -> MemoryHit {
        MemoryHit {
            id: id.to_string(),
            text: format!("text-{id}"),
            score: 0.9,
            memory_type: None,
        }
    }

    #[tokio::test]
    async fn audit_emits_one_event_per_consolidation_decision() {
        let extractor = Arc::new(MockExtractor(vec![
            fact("user prefers dark mode", MemoryType::Preference),
            fact("user lives in NYC", MemoryType::Fact),
            fact("stale note", MemoryType::Observation),
        ]));
        let mut actions = std::collections::VecDeque::new();
        actions.push_back(ConsolidationAction::Add);
        actions.push_back(ConsolidationAction::Update {
            id: "m-42".to_string(),
        });
        actions.push_back(ConsolidationAction::Delete {
            id: "m-7".to_string(),
        });
        let consolidator = Arc::new(MockConsolidator(Mutex::new(actions)));
        let store = Arc::new(HitStore(vec![hit("h1"), hit("h2")]));
        let audit = Arc::new(RecordingAuditSink::default());

        let engine =
            MemoryWriteEngine::new(extractor, consolidator, store).with_audit_sink(audit.clone());
        engine
            .ingest(
                &scope(),
                &MessagePair {
                    user: "hi".to_string(),
                    assistant: "hello".to_string(),
                },
            )
            .await
            .expect("ingest");

        let events = audit.emitted.lock().unwrap();
        assert_eq!(events.len(), 3, "one audit event per decision");
        assert_eq!(events[0].action, "add");
        assert_eq!(events[1].action, "update");
        assert_eq!(events[1].memory_id.as_deref(), Some("m-42"));
        assert_eq!(events[2].action, "delete");
        assert_eq!(events[2].memory_id.as_deref(), Some("m-7"));
        // The decision carries the candidate set it was made against (the "why").
        assert_eq!(events[0].similar_count, 2);
        assert_eq!(
            events[0].similar_ids,
            vec!["h1".to_string(), "h2".to_string()]
        );
        assert_eq!(events[0].tenant_id, "acme");
        assert_eq!(events[0].session_id, "sess-1");
    }

    #[tokio::test]
    async fn ingest_without_audit_sink_is_unchanged() {
        let extractor = Arc::new(MockExtractor(vec![fact("x", MemoryType::Fact)]));
        let consolidator = Arc::new(MockConsolidator(Mutex::new(
            [ConsolidationAction::Add].into_iter().collect(),
        )));
        let store = Arc::new(HitStore(Vec::new()));
        // Plain `new` — no audit sink.
        let engine = MemoryWriteEngine::new(extractor, consolidator, store);
        let applied = engine
            .ingest(
                &scope(),
                &MessagePair {
                    user: "u".to_string(),
                    assistant: "a".to_string(),
                },
            )
            .await
            .expect("ingest");
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].kind, "add");
    }

    #[tokio::test]
    async fn audit_emit_failure_does_not_fail_ingest() {
        let extractor = Arc::new(MockExtractor(vec![fact("x", MemoryType::Fact)]));
        let consolidator = Arc::new(MockConsolidator(Mutex::new(
            [ConsolidationAction::Add].into_iter().collect(),
        )));
        let store = Arc::new(HitStore(Vec::new()));
        let engine = MemoryWriteEngine::new(extractor, consolidator, store)
            .with_audit_sink(Arc::new(FailingAuditSink));
        // The audit sink errors, but ingest must still succeed (non-fatal).
        let applied = engine
            .ingest(
                &scope(),
                &MessagePair {
                    user: "u".to_string(),
                    assistant: "a".to_string(),
                },
            )
            .await
            .expect("ingest must not fail when audit sink errors");
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].kind, "add");
    }

    #[test]
    fn consolidation_audit_event_round_trips_json() {
        let ev = ConsolidationAuditEvent {
            collection: "mem".to_string(),
            tenant_id: "acme".to_string(),
            session_id: "sess-1".to_string(),
            actor: "assistant-1".to_string(),
            fact_text: "user likes tea".to_string(),
            memory_type: Some(MemoryType::Preference),
            action: "add".to_string(),
            memory_id: Some("m1".to_string()),
            similar_ids: vec!["h1".to_string()],
            similar_count: 1,
        };
        let json = serde_json::to_value(&ev).expect("serialize");
        let back: ConsolidationAuditEvent = serde_json::from_value(json).expect("deserialize");
        assert_eq!(ev, back);
    }

    // ── fail-closed tenant scoping (security) ─────────────────────────────

    /// Extractor that records whether it was invoked, to prove the fail-closed
    /// guard runs BEFORE any extraction/store work.
    #[derive(Default)]
    struct CountingExtractor {
        calls: Mutex<usize>,
    }
    #[async_trait]
    impl ExtractionAgent for CountingExtractor {
        async fn extract(&self, _pair: &MessagePair) -> Result<Vec<ExtractedFact>> {
            *self.calls.lock().map_err(|_| anyhow!("poisoned"))? += 1;
            Ok(vec![fact("x", MemoryType::Fact)])
        }
    }

    #[tokio::test]
    async fn empty_tenant_is_rejected_fail_closed() {
        let extractor = Arc::new(CountingExtractor::default());
        let consolidator = Arc::new(MockConsolidator(Mutex::new(
            [ConsolidationAction::Add].into_iter().collect(),
        )));
        let store = Arc::new(HitStore(Vec::new()));
        let engine = MemoryWriteEngine::new(extractor.clone(), consolidator, store);

        let empty_tenant = MemoryWriteScope {
            tenant_id: "   ".to_string(), // whitespace = empty
            ..scope()
        };
        let result = engine
            .ingest(
                &empty_tenant,
                &MessagePair {
                    user: "u".to_string(),
                    assistant: "a".to_string(),
                },
            )
            .await;

        assert!(
            result.is_err(),
            "empty tenant must be rejected, not run unscoped"
        );
        assert_eq!(
            *extractor.calls.lock().unwrap(),
            0,
            "extraction must NOT run under an empty tenant (fail-closed before any work)"
        );
    }

    #[test]
    fn prefix_is_a_true_session_boundary() {
        let p = consolidation_entity_prefix("acme", "s1");
        assert_eq!(p, "memory-consolidation:acme:s1:");
        // The sink's entity-id is prefix + memory_id, so a real decision id
        // starts with the prefix...
        assert!(format!("{p}m42").starts_with(&p));
        // ...but a different session (s1b) does NOT, thanks to the trailing ':'.
        let other = consolidation_entity_prefix("acme", "s1b");
        assert!(!format!("{other}m9").starts_with(&p));
        // ...and neither does a different tenant.
        let other_tenant = consolidation_entity_prefix("evil", "s1");
        assert!(!format!("{other_tenant}m42").starts_with(&p));
    }

    async fn temp_event_log(name: &str) -> Arc<EventLogEngine> {
        let base_dir = std::path::PathBuf::from(format!("/tmp/test_memaudit_{name}"));
        let _ = std::fs::remove_dir_all(&base_dir);
        std::fs::create_dir_all(&base_dir).expect("create dir");
        let cfg = crate::storage::engines::eventlog::EventLogConfig {
            base_dir,
            ..Default::default()
        };
        let local = crate::storage::persistence::filesystem::local::LocalFileSystem::new(
            crate::storage::persistence::filesystem::local::LocalConfig::default(),
        )
        .await
        .expect("local fs");
        let fs = Arc::new(
            crate::storage::persistence::filesystem::UnifiedCachingFilesystem::new(
                Arc::new(local),
                format!("memaudit_{name}"),
                "eventlog".to_string(),
            ),
        );
        Arc::new(EventLogEngine::new(cfg, fs).expect("eventlog engine"))
    }

    fn decision(
        tenant: &str,
        session: &str,
        action: &str,
        memory_id: Option<&str>,
    ) -> ConsolidationAuditEvent {
        ConsolidationAuditEvent {
            collection: "mem".to_string(),
            tenant_id: tenant.to_string(),
            session_id: session.to_string(),
            actor: "assistant-1".to_string(),
            fact_text: format!("fact for {action}"),
            memory_type: None,
            action: action.to_string(),
            memory_id: memory_id.map(str::to_string),
            similar_ids: vec![],
            similar_count: 0,
        }
    }

    #[tokio::test]
    async fn sink_then_reader_round_trips_one_session_only() {
        let log = temp_event_log("roundtrip").await;
        let sink = EventLogConsolidationAuditSink::from_event_log(log.clone());

        // Two decisions in (acme, s1); decoys in a sibling session, sibling
        // tenant, and a session whose name is a prefix-superstring of s1.
        sink.emit(&decision("acme", "s1", "add", Some("m1")))
            .await
            .unwrap();
        sink.emit(&decision("acme", "s1", "update", Some("m2")))
            .await
            .unwrap();
        sink.emit(&decision("acme", "s1b", "add", Some("m9")))
            .await
            .unwrap();
        sink.emit(&decision("evil", "s1", "add", Some("m1")))
            .await
            .unwrap();
        sink.emit(&decision("acme", "s1", "noop", None))
            .await
            .unwrap();

        let reader = ConsolidationAuditReader::new(log);
        let got = reader
            .list_session_decisions("acme", "s1", 100)
            .await
            .unwrap();

        // Exactly the three (acme, s1) decisions — not s1b, not the evil tenant.
        assert_eq!(got.len(), 3, "got: {got:?}");
        assert!(
            got.iter()
                .all(|d| d.tenant_id == "acme" && d.session_id == "s1")
        );
        let actions: Vec<&str> = got.iter().map(|d| d.action.as_str()).collect();
        assert_eq!(
            actions,
            vec!["add", "update", "noop"],
            "append order preserved"
        );

        // limit is honored.
        let one = reader
            .list_session_decisions("acme", "s1", 1)
            .await
            .unwrap();
        assert_eq!(one.len(), 1);

        // Unknown session → empty, not error.
        let none = reader
            .list_session_decisions("acme", "nope", 100)
            .await
            .unwrap();
        assert!(none.is_empty());

        let _ = std::fs::remove_dir_all("/tmp/test_memaudit_roundtrip");
    }
}

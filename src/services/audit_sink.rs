//! Shared audit-event emission foundation.
//!
//! ProximaDB persists structured audit records (AQL query trails, agent-memory
//! consolidation decisions, …) to the one process-wide `EventLogEngine`. Before
//! this module each emitter hand-rolled the `Event{}` struct and called
//! `append_event` itself — two shapes for the same operation. `AuditEventSink`
//! is the single convergence point: callers build a domain-neutral
//! [`AuditRecord`] and emit it; exactly one place ([`EventLogAuditSink`]) owns
//! the `Event` construction (sequence/timestamp defaults + `append_event`).
//!
//! Per the Convergence Gate (CLAUDE.md): this introduces NO new storage path —
//! it converges existing emitters onto the shared `EventLogEngine`. Domain
//! specifics (entity-id scheme, event-type string, causation, metadata) stay
//! caller-supplied on the `AuditRecord`; the sink abstracts the *mechanism*,
//! not the *content*. A typed event-type registry is intentionally deferred
//! (premature at the current handful of emitters).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::storage::engines::eventlog::{Event, EventLogEngine};

/// A domain-neutral audit record to be persisted. `entity_id`/`event_type` and
/// the optional causation/metadata are chosen by each caller; `data` is the
/// caller's serialized domain payload (e.g. an AQL `AuditTrail` or a memory
/// `ConsolidationAuditEvent`).
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub entity_id: String,
    pub event_type: String,
    pub data: serde_json::Value,
    pub causation_id: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl AuditRecord {
    /// Minimal record: entity + type + payload, no causation/metadata.
    pub fn new(
        entity_id: impl Into<String>,
        event_type: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            entity_id: entity_id.into(),
            event_type: event_type.into(),
            data,
            causation_id: None,
            metadata: HashMap::new(),
        }
    }

    /// Set the causation/correlation id (e.g. a session id).
    pub fn with_causation(mut self, causation_id: impl Into<String>) -> Self {
        self.causation_id = Some(causation_id.into());
        self
    }

    /// Add one metadata key/value.
    pub fn with_metadata_kv(
        mut self,
        key: impl Into<String>,
        value: serde_json::Value,
    ) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// Sink that persists [`AuditRecord`]s. The mockable seam every audit emitter
/// shares; the production impl is [`EventLogAuditSink`].
#[async_trait]
pub trait AuditEventSink: Send + Sync {
    async fn emit(&self, record: AuditRecord) -> Result<()>;
}

/// The single production `AuditEventSink`: constructs the canonical [`Event`]
/// (assigning sequence/timestamp) and appends it to the shared `EventLogEngine`.
/// This is the one place audit-event construction lives.
pub struct EventLogAuditSink {
    log: Arc<EventLogEngine>,
}

impl EventLogAuditSink {
    pub fn new(log: Arc<EventLogEngine>) -> Self {
        Self { log }
    }
}

#[async_trait]
impl AuditEventSink for EventLogAuditSink {
    async fn emit(&self, record: AuditRecord) -> Result<()> {
        let event = Event {
            sequence: 0, // assigned by append_event
            entity_id: record.entity_id,
            event_type: record.event_type,
            data: record.data,
            timestamp: chrono::Utc::now(),
            causation_id: record.causation_id,
            metadata: record.metadata,
        };
        self.log.append_event(event).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Recording sink used to assert callers build the right `AuditRecord`
    /// without a real EventLogEngine.
    #[derive(Default)]
    pub struct RecordingAuditSink {
        pub records: Mutex<Vec<AuditRecord>>,
    }

    #[async_trait]
    impl AuditEventSink for RecordingAuditSink {
        async fn emit(&self, record: AuditRecord) -> Result<()> {
            self.records
                .lock()
                .map_err(|_| anyhow::anyhow!("poisoned"))?
                .push(record);
            Ok(())
        }
    }

    #[test]
    fn audit_record_new_defaults_causation_and_metadata() {
        let r = AuditRecord::new("query:1", "AqlQueryExecuted", serde_json::json!({"x": 1}));
        assert_eq!(r.entity_id, "query:1");
        assert_eq!(r.event_type, "AqlQueryExecuted");
        assert_eq!(r.data, serde_json::json!({"x": 1}));
        assert!(r.causation_id.is_none());
        assert!(r.metadata.is_empty());
    }

    #[test]
    fn audit_record_builders_set_fields() {
        let r = AuditRecord::new("memory-consolidation:t:s:m1", "MemoryConsolidationDecision", serde_json::json!({}))
            .with_causation("s")
            .with_metadata_kv("collection", serde_json::json!("mem"));
        assert_eq!(r.causation_id.as_deref(), Some("s"));
        assert_eq!(r.metadata.get("collection"), Some(&serde_json::json!("mem")));
    }

    #[tokio::test]
    async fn recording_sink_captures_emitted_record() {
        let sink = RecordingAuditSink::default();
        sink.emit(AuditRecord::new("e", "T", serde_json::json!({"k": "v"})))
            .await
            .expect("emit");
        let recs = sink.records.lock().unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].entity_id, "e");
        assert_eq!(recs[0].event_type, "T");
    }
}

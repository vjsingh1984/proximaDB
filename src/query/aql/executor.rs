//! # AQL Query Executor
//!
//! Coordinates execution of AQL queries across multiple data models,
//! ensuring a structured audit trail is captured for every operation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::query::aql::{
    AqlFrom, AqlQuery, AqlResult, AqlSource, AqlValue, AuditContext, AuditFrame, AuditOp,
    AuditOutcome, AuditTrail, JoinType, Result,
};

use crate::storage::engines::eventlog::{Event, EventLogEngine};

pub struct AqlExecutor {
    sources: HashMap<String, Arc<dyn AqlSource>>,
    event_log: Option<Arc<EventLogEngine>>,
}

impl Default for AqlExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl AqlExecutor {
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
            event_log: None,
        }
    }

    /// Attach an event log engine for persistent audit trails.
    pub fn with_event_log(mut self, event_log: Arc<EventLogEngine>) -> Self {
        self.event_log = Some(event_log);
        self
    }

    /// Register a data source with the executor.
    pub fn register_source(&mut self, name: String, source: Arc<dyn AqlSource>) {
        self.sources.insert(name, source);
    }

    /// Execute an AQL query and return results along with an audit trail.
    pub async fn execute(&self, query: AqlQuery) -> Result<(AqlResult, AuditTrail)> {
        let mut ctx = AuditContext::new();
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let result = self.execute_internal(&query, &query.from, &mut ctx).await;

        let finished_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let (aql_result, outcome) = match result {
            Ok(r) => (r, AuditOutcome::Success),
            Err(e) => {
                // Generate a dummy result for failure case
                (
                    AqlResult {
                        rows: Vec::new(),
                        frame_id: 0,
                    },
                    AuditOutcome::Failure {
                        reason: e.to_string(),
                    },
                )
            }
        };

        let trail = AuditTrail {
            query_id: ctx.query_id,
            started_at_ms,
            finished_at_ms,
            plan: query.clone(),
            frames: ctx.frames,
            outcome,
        };

        // Persist the audit trail if event log is available (TD-050 Phase 5)
        if let Some(log) = &self.event_log
            && let Err(e) = self.persist_audit_trail(log.as_ref(), &trail).await
        {
            tracing::warn!("Failed to persist audit trail: {}", e);
        }

        Ok((aql_result, trail))
    }

    /// Retrieve an audit trail from the event log (TD-050 Phase 4).
    pub async fn get_audit_trail(&self, query_id: &str) -> Result<AuditTrail> {
        let log = self.event_log.as_ref().ok_or_else(|| {
            crate::core::error::ProximaDBError::InvalidInput("Event log not configured".to_string())
        })?;

        let entity_id = format!("query:{}", query_id);

        // Read the latest event for this query ID
        let events = log.read_events(&entity_id, 0, 1).await?;
        let event = events.first().ok_or_else(|| {
            crate::core::error::ProximaDBError::Storage(crate::core::error::StorageError::NotFound(
                format!("Audit trail for query '{}'", query_id),
            ))
        })?;

        let trail: AuditTrail = serde_json::from_value(event.data.clone()).map_err(|e| {
            crate::core::error::ProximaDBError::Internal(format!(
                "Audit trail deserialization failed: {}",
                e
            ))
        })?;

        Ok(trail)
    }

    async fn persist_audit_trail(&self, log: &EventLogEngine, trail: &AuditTrail) -> Result<()> {
        let event = Event {
            sequence: 0,
            entity_id: format!("query:{}", trail.query_id),
            event_type: "AqlQueryExecuted".to_string(),
            data: serde_json::to_value(trail).map_err(|e| {
                crate::core::error::ProximaDBError::Internal(format!(
                    "Audit trail serialization failed: {}",
                    e
                ))
            })?,
            timestamp: chrono::Utc::now(),
            causation_id: None,
            metadata: HashMap::new(),
        };

        log.append_event(event).await?;
        Ok(())
    }

    async fn execute_internal(
        &self,
        full_query: &AqlQuery,
        from: &AqlFrom,
        ctx: &mut AuditContext,
    ) -> Result<AqlResult> {
        match from {
            AqlFrom::Source { name, .. } => {
                let source = self.sources.get(name).ok_or_else(|| {
                    crate::core::error::ProximaDBError::InvalidInput(format!(
                        "Source '{}' not found",
                        name
                    ))
                })?;
                source.execute(full_query, ctx).await
            }
            AqlFrom::Join {
                left,
                right,
                on,
                join_type,
            } => {
                let start = Instant::now();

                // Execute left and right sides recursively
                let left_res = Box::pin(self.execute_internal(full_query, left, ctx)).await?;
                let right_res = Box::pin(self.execute_internal(full_query, right, ctx)).await?;

                self.join_results(left_res, right_res, on, *join_type, ctx, start)
            }
            AqlFrom::MultiSource { sources } => {
                Box::pin(self.execute_multi_source(full_query, sources, ctx)).await
            }
        }
    }

    async fn execute_multi_source(
        &self,
        full_query: &AqlQuery,
        sources: &[crate::query::aql::AqlSourceSpec],
        ctx: &mut AuditContext,
    ) -> Result<AqlResult> {
        if sources.is_empty() {
            return Err(crate::core::error::ProximaDBError::InvalidInput(
                "MultiSource query requires at least one source".to_string(),
            ));
        }

        let mut source_results = Vec::with_capacity(sources.len());
        for source in sources {
            let source_from = AqlFrom::Source {
                name: source.name.clone(),
                alias: source.alias.clone(),
            };
            let result = Box::pin(self.execute_internal(full_query, &source_from, ctx)).await?;
            source_results.push((source, result));
        }

        let mut iter = source_results.into_iter();
        let (_, mut combined) = iter.next().ok_or_else(|| {
            crate::core::error::ProximaDBError::InvalidInput(
                "MultiSource query requires at least one source".to_string(),
            )
        })?;

        for (source, next_result) in iter {
            let dependency = source.dependencies.first();
            let join_type = dependency.map_or(JoinType::Inner, |dep| dep.join_type);
            let join_field = dependency
                .map(|dep| dep.on_field.clone())
                .unwrap_or_else(|| "id".to_string());
            let predicate = crate::query::aql::AqlPredicate::Equals {
                field: join_field,
                value: AqlValue::Null,
            };

            combined = self.join_results(
                combined,
                next_result,
                &predicate,
                join_type,
                ctx,
                Instant::now(),
            )?;
        }

        Ok(combined)
    }

    fn join_results(
        &self,
        left_res: AqlResult,
        right_res: AqlResult,
        on: &crate::query::aql::AqlPredicate,
        join_type: JoinType,
        ctx: &mut AuditContext,
        start: Instant,
    ) -> Result<AqlResult> {
        // Perform the join (simplified implementation).
        let joined_rows = self.perform_join(&left_res.rows, &right_res.rows, on, join_type)?;

        let wall_time_us = start.elapsed().as_micros() as u64;
        let frame = AuditFrame {
            frame_id: 0,
            source: crate::query::aql::DataModel::Relational,
            op: AuditOp::Join {
                join_type,
                left_frame: left_res.frame_id,
                right_frame: right_res.frame_id,
            },
            filters_pushed: Vec::new(),
            filters_post: Vec::new(),
            records_scanned: (left_res.rows.len() + right_res.rows.len()) as u64,
            records_returned: joined_rows.len() as u64,
            wall_time_us,
            error: None,
            redaction_count: 0,
        };

        let frame_id = ctx.push_frame(frame);
        Ok(AqlResult {
            rows: joined_rows,
            frame_id,
        })
    }

    fn perform_join(
        &self,
        left_rows: &[HashMap<String, AqlValue>],
        right_rows: &[HashMap<String, AqlValue>],
        _on: &crate::query::aql::AqlPredicate,
        _join_type: JoinType,
    ) -> Result<Vec<HashMap<String, AqlValue>>> {
        // Cross-product as a placeholder for now.
        // In a real implementation, we'd evaluate the 'on' predicate.
        let mut result = Vec::new();
        for left in left_rows {
            for right in right_rows {
                let mut joined = left.clone();
                for (k, v) in right {
                    joined.insert(format!("right.{}", k), v.clone());
                }
                result.push(joined);
            }
        }
        Ok(result)
    }
}

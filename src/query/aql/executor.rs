//! # AQL Query Executor
//!
//! Coordinates execution of AQL queries across multiple data models,
//! ensuring a structured audit trail is captured for every operation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::query::aql::{
    AqlFrom, AqlQuery, AqlResult, AqlSource, AqlValue, AuditContext, AuditFrame, AuditOp,
    AuditOutcome, AuditTrail, JoinType, Result,
};

pub struct AqlExecutor {
    sources: HashMap<String, Arc<dyn AqlSource>>,
}

impl AqlExecutor {
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
        }
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
            .unwrap()
            .as_millis() as i64;

        let result = self.execute_internal(&query, &query.from, &mut ctx).await;

        let finished_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
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

        Ok((aql_result, trail))
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

                // Perform the join (simplified implementation)
                let joined_rows =
                    self.perform_join(&left_res.rows, &right_res.rows, on, *join_type)?;

                let wall_time_us = start.elapsed().as_micros() as u64;

                // Emit join audit frame
                let frame = AuditFrame {
                    frame_id: 0,
                    source: crate::query::aql::DataModel::Relational, // Joins are relational ops
                    op: AuditOp::Join {
                        join_type: *join_type,
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
        }
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

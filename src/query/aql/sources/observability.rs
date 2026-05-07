//! AQL Source implementation for Observability data model.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::observability::{LogQueryParams, ObservabilityService};
use crate::proto::proximadb_v1::sql_value::Value as SqlValueData;
use crate::query::aql::{
    AqlFrom, AqlQuery, AqlResult, AqlSource, AqlValue, AuditContext, AuditFrame, AuditOp,
    DataModel, Result,
};

pub struct ObservabilityAqlSource {
    obs_svc: Arc<ObservabilityService>,
}

impl ObservabilityAqlSource {
    pub fn new(obs_svc: Arc<ObservabilityService>) -> Self {
        Self { obs_svc }
    }

    fn extract_obs_params(&self, query: &AqlQuery) -> String {
        if let AqlFrom::Source { name, .. } = &query.from {
            name.clone()
        } else {
            "default".to_string()
        }
    }
}

#[async_trait]
impl AqlSource for ObservabilityAqlSource {
    fn model(&self) -> DataModel {
        DataModel::Observability
    }

    async fn execute(&self, query: &AqlQuery, ctx: &mut AuditContext) -> Result<AqlResult> {
        let namespace = self.extract_obs_params(query);
        let start = Instant::now();

        // Perform observability query (logs for now)
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let params = LogQueryParams {
            start_time_ns: now_ns - (3600 * 1_000_000_000), // Last hour default
            end_time_ns: now_ns,
            query: None,
            severities: Vec::new(),
            services: Vec::new(),
            sources: Vec::new(),
            limit: 100,
            cursor: None,
        };

        let log_result = self
            .obs_svc
            .query_logs(&namespace, params)
            .await
            .map_err(|e| {
                crate::core::error::ProximaDBError::Storage(
                    crate::core::error::StorageError::SstEngine(e.to_string()),
                )
            })?;

        let wall_time_us = start.elapsed().as_micros() as u64;

        // Convert to AQL rows
        let mut rows = Vec::new();
        for log in log_result.logs {
            let mut row = HashMap::new();
            row.insert("timestamp".to_string(), AqlValue::Int(log.timestamp_ns));
            row.insert("severity".to_string(), AqlValue::Int(log.severity.into()));
            row.insert("message".to_string(), AqlValue::String(log.message.clone()));
            if let Some(svc) = log.service {
                row.insert("service".to_string(), AqlValue::String(svc));
            }

            for (k, v) in log.fields {
                if let Some(val) = v.value {
                    let aql_val = match val {
                        SqlValueData::StringValue(s) => AqlValue::String(s),
                        SqlValueData::Int64Value(i) => AqlValue::Int(i),
                        SqlValueData::NumberValue(f) => AqlValue::Float(f),
                        SqlValueData::BoolValue(b) => AqlValue::Bool(b),
                        _ => AqlValue::Null,
                    };
                    row.insert(k, aql_val);
                }
            }
            rows.push(row);
        }

        // Emit audit frame
        let frame = AuditFrame {
            frame_id: 0,
            source: self.model(),
            op: AuditOp::DocumentQuery {
                // Reuse DocumentQuery for now or add LogQuery to AuditOp
                collection: namespace.clone(),
            },
            filters_pushed: Vec::new(),
            filters_post: Vec::new(),
            records_scanned: rows.len() as u64,
            records_returned: rows.len() as u64,
            wall_time_us,
            error: None,
            redaction_count: 0,
        };

        let frame_id = ctx.push_frame(frame);

        Ok(AqlResult { rows, frame_id })
    }
}

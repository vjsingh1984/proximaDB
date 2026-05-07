//! AQL Source implementation for Document data model.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::proto::proximadb_v1::sql_value::Value as SqlValueData;
use crate::query::aql::{
    AqlFrom, AqlQuery, AqlResult, AqlSource, AqlValue, AuditContext, AuditFrame, AuditOp,
    DataModel, Result,
};
use crate::storage::document::{DocumentQueryParams, DocumentService};

pub struct DocumentAqlSource {
    doc_svc: Arc<DocumentService>,
}

impl DocumentAqlSource {
    pub fn new(doc_svc: Arc<DocumentService>) -> Self {
        Self { doc_svc }
    }

    fn extract_doc_params(&self, query: &AqlQuery) -> String {
        if let AqlFrom::Source { name, .. } = &query.from {
            name.clone()
        } else {
            "default".to_string()
        }
    }
}

#[async_trait]
impl AqlSource for DocumentAqlSource {
    fn model(&self) -> DataModel {
        DataModel::Document
    }

    async fn execute(&self, query: &AqlQuery, ctx: &mut AuditContext) -> Result<AqlResult> {
        let collection = self.extract_doc_params(query);
        let start = Instant::now();

        // Perform document query
        let params = DocumentQueryParams {
            limit: 100,
            ..Default::default()
        };

        let query_result = self
            .doc_svc
            .query_documents(&collection, params)
            .await
            .map_err(|e| {
                crate::core::error::ProximaDBError::Storage(
                    crate::core::error::StorageError::SstEngine(e.to_string()),
                )
            })?;

        let wall_time_us = start.elapsed().as_micros() as u64;

        // Convert to AQL rows
        let mut rows = Vec::new();
        for doc in query_result.documents {
            let mut row = HashMap::new();
            row.insert("_id".to_string(), AqlValue::String(doc.id.clone()));

            for (k, v) in doc.document.fields {
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
                collection: collection.clone(),
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

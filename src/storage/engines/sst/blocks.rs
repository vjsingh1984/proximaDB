/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! SST Engine Record Types (Compatibility Layer)
//!
//! ## Status: Deprecated for Production Use
//!
//! Production code uses `ProximaRecord` and ProximaBlocks directly. This module
//! is kept only for test compatibility. See `blocks_archive.rs` for full legacy
//! type history.
//!
//! ## Migration Note (TD-001)
//! Block types (ProximaDataBlock, ProximaBlockMetadata, etc.) have been migrated to:
//! - `storage::engines::core::formats::proximablocks`
//!
//! ## Future Work
//! Migrate tests in `tests/sst/` to use canonical storage blocks directly, then
//! archive this module.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::search::sql_value_filter::proxima_tree_to_json_map;
use proximadb_records::ProximaRecord;

/// SST record representation with LSM-tree metadata
///
/// ## Deprecation Notice
/// Production code has been optimized to use `ProximaRecord`/ProximaBlocks
/// directly. This type is only used in tests and will be archived once tests
/// are updated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstRecord {
    /// Unique identifier
    pub id: String,
    /// Vector data
    pub vector: Option<Vec<f32>>,
    /// Metadata as JSON value
    pub metadata: Option<serde_json::Value>,
    /// Sequence number for ordering
    pub sequence_number: u64,
    /// LSM tree level
    pub level: u8,
    /// Deletion marker
    pub is_tombstone: bool,
    /// Timestamp
    pub timestamp: u64,
}

impl SstRecord {
    /// Create from the canonical record envelope.
    pub fn from_proxima_record(record: ProximaRecord, sequence_number: u64, level: u8) -> Self {
        let metadata_json = if !record.props.is_empty() {
            Some(serde_json::Value::Object(
                proxima_tree_to_json_map(&record.props)
                    .into_iter()
                    .collect(),
            ))
        } else {
            None
        };

        let vector = record
            .embeddings
            .first()
            .map(|embedding| embedding.values.clone());

        SstRecord {
            id: record.oid,
            vector,
            metadata: metadata_json,
            sequence_number,
            level,
            is_tombstone: false,
            timestamp: (record.created_at_ns / 1_000_000).max(0) as u64,
        }
    }

    /// Create a tombstone record for deletion
    pub fn tombstone(id: String, sequence_number: u64, level: u8) -> Self {
        SstRecord {
            id,
            vector: None,
            metadata: None,
            sequence_number,
            level,
            is_tombstone: true,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
        }
    }

    /// Convert to OptimizedSearchRecord
    pub fn to_optimized_search_result(
        &self,
        score: f32,
    ) -> crate::core::search::results::OptimizedSearchRecord {
        use proximadb_data_model::ProximaValue;
        let metadata = self
            .metadata
            .as_ref()
            .map(|json_value| {
                if let serde_json::Value::Object(map) = json_value {
                    map.iter()
                        .map(|(k, v)| {
                            let pv = match v {
                                serde_json::Value::String(s) => ProximaValue::String(s.clone()),
                                serde_json::Value::Number(n) => {
                                    ProximaValue::Float64(n.as_f64().unwrap_or(0.0))
                                }
                                serde_json::Value::Bool(b) => ProximaValue::Boolean(*b),
                                serde_json::Value::Null => ProximaValue::Null,
                                other => ProximaValue::Json(serde_json::json!(other)),
                            };
                            (k.clone(), pv)
                        })
                        .collect()
                } else {
                    std::collections::HashMap::new()
                }
            })
            .unwrap_or_default();
        crate::core::search::results::OptimizedSearchRecord {
            id: self.id.clone(),
            score,
            vector: self.vector.as_ref().map(|v| Arc::new(v.clone())),
            metadata,
            timestamp: Some(self.timestamp as i64),
            version: Some(self.sequence_number as u32),
            ..Default::default()
        }
    }

    /// Serialize record to bytes
    pub fn serialize(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Deserialize record from bytes
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(data)?)
    }
}

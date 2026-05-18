// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! ProximaRecord ↔ PAX column projection.
//!
//! Defines the canonical mapping from `ProximaRecord` fields to PAX column
//! stripes (ADR-ICE-001, ADR-010). Every collection uses this fixed column
//! layout as the leading stripe set; user-defined columns follow starting at
//! `FIRST_USER_COLUMN_ID`.
//!
//! The same mapping governs the Iceberg REST `load table` column schema
//! (see `src/catalog/iceberg_rest_service.rs`), ensuring cross-engine
//! consistency: Spark/Trino/DuckDB see the same columns as PAX block readers.

use proximadb_records::ProximaRecord;
use serde::{Deserialize, Serialize};

use crate::stripe::ColumnRole;

/// Canonical column IDs for ProximaRecord fields in PAX blocks.
/// These are stable and must not change between format versions.
pub mod col_id {
    pub const OID:          i32 = 0;
    pub const TENANT_ID:    i32 = 1;
    pub const CREATED_AT:   i32 = 2;
    pub const UPDATED_AT:   i32 = 3;
    pub const VALID_FROM:   i32 = 4;
    pub const VALID_TO:     i32 = 5;
    pub const ACTOR:        i32 = 6;
    pub const ORIGIN:       i32 = 7;
    pub const PROPS:        i32 = 8;
    pub const LABELS:       i32 = 9;
    pub const EDGE_SRC:     i32 = 10;
    pub const EDGE_TGT:     i32 = 11;
    pub const EDGE_TYPE:    i32 = 12;
    pub const EDGE_WEIGHT:  i32 = 13;
    /// First column ID for embedding stripes (embedding_0, embedding_1, …).
    pub const EMBED_BASE:   i32 = 20;
    /// First column ID for user-defined columns from CatalogTableSchema.
    pub const USER_BASE:    i32 = 100;
}

/// Descriptor for a single column stripe in a PAX block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDescriptor {
    pub column_id:   i32,
    pub name:        String,
    pub role:        ColumnRole,
    pub nullable:    bool,
}

/// Returns the canonical fixed column descriptors for every ProximaDB collection.
///
/// User-defined columns from `CatalogTableSchema` are appended after these.
/// Embedding columns are generated dynamically based on the registered models.
pub fn canonical_columns() -> Vec<ColumnDescriptor> {
    vec![
        ColumnDescriptor { column_id: col_id::OID,        name: "id".into(),         role: ColumnRole::Identity,   nullable: false },
        ColumnDescriptor { column_id: col_id::TENANT_ID,  name: "tenant_id".into(),  role: ColumnRole::Tenant,     nullable: false },
        ColumnDescriptor { column_id: col_id::CREATED_AT, name: "created_at".into(), role: ColumnRole::Timestamp,  nullable: false },
        ColumnDescriptor { column_id: col_id::UPDATED_AT, name: "updated_at".into(), role: ColumnRole::Timestamp,  nullable: false },
        ColumnDescriptor { column_id: col_id::VALID_FROM, name: "valid_from".into(), role: ColumnRole::Temporal,   nullable: true  },
        ColumnDescriptor { column_id: col_id::VALID_TO,   name: "valid_to".into(),   role: ColumnRole::Temporal,   nullable: true  },
        ColumnDescriptor { column_id: col_id::ACTOR,      name: "actor".into(),      role: ColumnRole::Provenance, nullable: true  },
        ColumnDescriptor { column_id: col_id::ORIGIN,     name: "origin".into(),     role: ColumnRole::Provenance, nullable: true  },
        ColumnDescriptor { column_id: col_id::PROPS,      name: "props".into(),      role: ColumnRole::Props,      nullable: true  },
        ColumnDescriptor { column_id: col_id::LABELS,     name: "labels".into(),     role: ColumnRole::Props,      nullable: true  },
        ColumnDescriptor { column_id: col_id::EDGE_SRC,   name: "edge_source_id".into(), role: ColumnRole::Edge,  nullable: true  },
        ColumnDescriptor { column_id: col_id::EDGE_TGT,   name: "edge_target_id".into(), role: ColumnRole::Edge,  nullable: true  },
        ColumnDescriptor { column_id: col_id::EDGE_TYPE,  name: "edge_type".into(),  role: ColumnRole::Edge,       nullable: true  },
        ColumnDescriptor { column_id: col_id::EDGE_WEIGHT,name: "edge_weight".into(),role: ColumnRole::Edge,       nullable: true  },
    ]
}

/// Flat row representation extracted from a `ProximaRecord` for column encoding.
///
/// Each field corresponds directly to one column stripe. Absent optional values
/// contribute a null entry to the null bitmap.
#[derive(Debug, Clone)]
pub struct FlatRow {
    /// Record identifier (oid). Never null.
    pub oid:           String,
    /// Tenant identifier. Never null; engine-level RLS.
    pub tenant_id:     String,
    pub created_at_ns: i64,
    pub updated_at_ns: i64,
    pub valid_from_ns: Option<i64>,
    pub valid_to_ns:   Option<i64>,
    pub actor:         Option<String>,
    pub origin:        Option<String>,
    /// msgpack-serialised props tree bytes.
    pub props_bytes:   Option<Vec<u8>>,
    /// msgpack-serialised label list bytes.
    pub labels_bytes:  Option<Vec<u8>>,
    pub edge_src:      Option<String>,
    pub edge_tgt:      Option<String>,
    pub edge_type:     Option<String>,
    pub edge_weight:   Option<f64>,
    /// One entry per embedding (model order from record).
    pub embeddings:    Vec<Vec<f32>>,
}

impl FlatRow {
    /// Extract a `FlatRow` from a `ProximaRecord`.
    ///
    /// `props` and `labels` are serialised to msgpack bytes so they can be
    /// stored opaquely in the Props column stripe.
    pub fn from_record(record: &ProximaRecord) -> anyhow::Result<Self> {
        let props_bytes = if record.props.is_empty() {
            None
        } else {
            Some(rmp_serde::to_vec_named(&record.props)?)
        };

        let labels_vec: Vec<&str> = record.labels.iter().map(|s| s.as_str()).collect();
        let labels_bytes = if labels_vec.is_empty() {
            None
        } else {
            Some(rmp_serde::to_vec_named(&labels_vec)?)
        };

        let (edge_src, edge_tgt, edge_type, edge_weight) = match &record.edge {
            Some(e) => (
                Some(e.source_id.clone()),
                Some(e.target_id.clone()),
                Some(e.edge_type.clone()),
                e.weight,
            ),
            None => (None, None, None, None),
        };

        let embeddings: Vec<Vec<f32>> = record
            .embeddings
            .iter()
            .map(|emb| emb.values.clone())
            .collect();

        Ok(FlatRow {
            oid:           record.oid.clone(),
            tenant_id:     record.tenant_id.clone(),
            created_at_ns: record.created_at_ns,
            updated_at_ns: record.updated_at_ns,
            valid_from_ns: record.valid_from_ns,
            valid_to_ns:   record.valid_to_ns,
            actor:         record.actor.clone(),
            origin:        record.origin.clone(),
            props_bytes,
            labels_bytes,
            edge_src,
            edge_tgt,
            edge_type,
            edge_weight,
            embeddings,
        })
    }

    /// Reconstruct a `ProximaRecord` from a decoded `FlatRow`.
    ///
    /// Embedding model IDs are provided externally (from the collection schema).
    pub fn into_record(self, embedding_model_ids: &[String]) -> anyhow::Result<ProximaRecord> {
        use proximadb_records::{EmbeddingCell, EdgeShape, LabelSet};
        use std::collections::HashMap;

        let props = match self.props_bytes {
            Some(b) => rmp_serde::from_slice(&b)?,
            None => HashMap::new(),
        };

        let labels: LabelSet = match self.labels_bytes {
            Some(b) => {
                let v: Vec<String> = rmp_serde::from_slice(&b)?;
                v.into()
            }
            None => LabelSet::new(),
        };

        let edge = match (self.edge_src, self.edge_tgt, self.edge_type) {
            (Some(src), Some(tgt), Some(etype)) => Some(EdgeShape {
                source_id: src,
                target_id: tgt,
                edge_type:  etype,
                weight:     self.edge_weight,
            }),
            _ => None,
        };

        let embeddings: Vec<EmbeddingCell> = self
            .embeddings
            .into_iter()
            .enumerate()
            .map(|(i, values)| {
                let dim = values.len() as u32;
                EmbeddingCell {
                    model_id: embedding_model_ids
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("model_{i}")),
                    modality: "dense".into(),
                    values,
                    dim,
                }
            })
            .collect();

        Ok(ProximaRecord {
            oid:           self.oid,
            tenant_id:     self.tenant_id,
            created_at_ns: self.created_at_ns,
            updated_at_ns: self.updated_at_ns,
            valid_from_ns: self.valid_from_ns,
            valid_to_ns:   self.valid_to_ns,
            actor:         self.actor,
            origin:        self.origin,
            props,
            labels,
            edge,
            embeddings,
            ..Default::default()
        })
    }
}

/// Encode a string column value as raw bytes for the stripe.
pub fn encode_str_col(values: &[Option<&str>]) -> (Vec<u8>, u32) {
    // Variable-length layout: 4B length prefix per value; null = 0xFFFF_FFFF.
    let mut buf = Vec::new();
    let mut null_count = 0u32;
    for v in values {
        match v {
            Some(s) => {
                let bytes = s.as_bytes();
                buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(bytes);
            }
            None => {
                buf.extend_from_slice(&u32::MAX.to_le_bytes());
                null_count += 1;
            }
        }
    }
    (buf, null_count)
}

/// Encode an i64 column (delta-encoded for temporal columns).
pub fn encode_i64_col(values: &[Option<i64>]) -> (Vec<u8>, u32) {
    let mut buf = Vec::new();
    let mut null_count = 0u32;
    for v in values {
        match v {
            Some(n) => buf.extend_from_slice(&n.to_le_bytes()),
            None => {
                buf.extend_from_slice(&i64::MIN.to_le_bytes()); // sentinel
                null_count += 1;
            }
        }
    }
    (buf, null_count)
}

/// Encode an f32 vector embedding stripe.
pub fn encode_f32_vec_col(values: &[Option<&[f32]>]) -> (Vec<u8>, u32) {
    let mut buf = Vec::new();
    let mut null_count = 0u32;
    for v in values {
        match v {
            Some(floats) => {
                buf.extend_from_slice(&(floats.len() as u32).to_le_bytes());
                for &f in *floats {
                    buf.extend_from_slice(&f.to_le_bytes());
                }
            }
            None => {
                buf.extend_from_slice(&u32::MAX.to_le_bytes());
                null_count += 1;
            }
        }
    }
    (buf, null_count)
}

/// Update `ColumnMeta.min_val` and `max_val` for an i64 column.
pub fn update_i64_bounds(meta: &mut crate::stripe::ColumnMeta, value: i64) {
    let cur_min = i64::from_le_bytes(meta.min_val[0..8].try_into().unwrap_or([0; 8]));
    let cur_max = i64::from_le_bytes(meta.max_val[0..8].try_into().unwrap_or([0; 8]));

    let new_min = if cur_min == 0 && cur_max == 0 { value } else { cur_min.min(value) };
    let new_max = if cur_min == 0 && cur_max == 0 { value } else { cur_max.max(value) };

    meta.min_val[0..8].copy_from_slice(&new_min.to_le_bytes());
    meta.max_val[0..8].copy_from_slice(&new_max.to_le_bytes());
}

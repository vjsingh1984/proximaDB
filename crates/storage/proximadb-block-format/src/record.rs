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

use crate::reader::PaxBlockReader;
use crate::stripe::ColumnRole;

/// Canonical column IDs for ProximaRecord fields in PAX blocks.
/// These are stable and must not change between format versions.
pub mod col_id {
    pub const OID: i32 = 0;
    pub const TENANT_ID: i32 = 1;
    pub const CREATED_AT: i32 = 2;
    pub const UPDATED_AT: i32 = 3;
    pub const VALID_FROM: i32 = 4;
    pub const VALID_TO: i32 = 5;
    pub const ACTOR: i32 = 6;
    pub const ORIGIN: i32 = 7;
    pub const PROPS: i32 = 8;
    pub const LABELS: i32 = 9;
    pub const EDGE_SRC: i32 = 10;
    pub const EDGE_TGT: i32 = 11;
    pub const EDGE_TYPE: i32 = 12;
    pub const EDGE_WEIGHT: i32 = 13;
    /// First column ID for embedding stripes (embedding_0, embedding_1, …).
    pub const EMBED_BASE: i32 = 20;
    /// First column ID for user-defined columns from CatalogTableSchema.
    pub const USER_BASE: i32 = 100;
    /// First column ID for co-located rerank stripes (SQ8 rerank_0, rerank_1, …).
    ///
    /// When a collection writes RaBitQ-quantized embedding stripes (the hot
    /// candidate-scan representation), the writer ALSO emits an SQ8-quantized
    /// copy of the same embedding at `RERANK_BASE + i`. The cascade reranks the
    /// RaBitQ candidate pool against this co-located SQ8 column (4× footprint, no
    /// extra GET against an external f32 tier) before taking the final top-k;
    /// f32 rerank is added only if the recall gate can't be met on SQ8. Chosen
    /// well above `USER_BASE` so it never collides with catalog-driven columns.
    pub const RERANK_BASE: i32 = 1000;
}

/// Descriptor for a single column stripe in a PAX block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDescriptor {
    pub column_id: i32,
    pub name: String,
    pub role: ColumnRole,
    pub nullable: bool,
}

/// Returns the canonical fixed column descriptors for every ProximaDB collection.
///
/// User-defined columns from `CatalogTableSchema` are appended after these.
/// Embedding columns are generated dynamically based on the registered models.
pub fn canonical_columns() -> Vec<ColumnDescriptor> {
    vec![
        ColumnDescriptor {
            column_id: col_id::OID,
            name: "id".into(),
            role: ColumnRole::Identity,
            nullable: false,
        },
        ColumnDescriptor {
            column_id: col_id::TENANT_ID,
            name: "tenant_id".into(),
            role: ColumnRole::Tenant,
            nullable: false,
        },
        ColumnDescriptor {
            column_id: col_id::CREATED_AT,
            name: "created_at".into(),
            role: ColumnRole::Timestamp,
            nullable: false,
        },
        ColumnDescriptor {
            column_id: col_id::UPDATED_AT,
            name: "updated_at".into(),
            role: ColumnRole::Timestamp,
            nullable: false,
        },
        ColumnDescriptor {
            column_id: col_id::VALID_FROM,
            name: "valid_from".into(),
            role: ColumnRole::Temporal,
            nullable: true,
        },
        ColumnDescriptor {
            column_id: col_id::VALID_TO,
            name: "valid_to".into(),
            role: ColumnRole::Temporal,
            nullable: true,
        },
        ColumnDescriptor {
            column_id: col_id::ACTOR,
            name: "actor".into(),
            role: ColumnRole::Provenance,
            nullable: true,
        },
        ColumnDescriptor {
            column_id: col_id::ORIGIN,
            name: "origin".into(),
            role: ColumnRole::Provenance,
            nullable: true,
        },
        ColumnDescriptor {
            column_id: col_id::PROPS,
            name: "props".into(),
            role: ColumnRole::Props,
            nullable: true,
        },
        ColumnDescriptor {
            column_id: col_id::LABELS,
            name: "labels".into(),
            role: ColumnRole::Props,
            nullable: true,
        },
        ColumnDescriptor {
            column_id: col_id::EDGE_SRC,
            name: "edge_source_id".into(),
            role: ColumnRole::Edge,
            nullable: true,
        },
        ColumnDescriptor {
            column_id: col_id::EDGE_TGT,
            name: "edge_target_id".into(),
            role: ColumnRole::Edge,
            nullable: true,
        },
        ColumnDescriptor {
            column_id: col_id::EDGE_TYPE,
            name: "edge_type".into(),
            role: ColumnRole::Edge,
            nullable: true,
        },
        ColumnDescriptor {
            column_id: col_id::EDGE_WEIGHT,
            name: "edge_weight".into(),
            role: ColumnRole::Edge,
            nullable: true,
        },
    ]
}

/// Flat row representation extracted from a `ProximaRecord` for column encoding.
///
/// Each field corresponds directly to one column stripe. Absent optional values
/// contribute a null entry to the null bitmap.
#[derive(Debug, Clone)]
pub struct FlatRow {
    /// Record identifier (oid). Never null.
    pub oid: String,
    /// Tenant identifier. Never null; engine-level RLS.
    pub tenant_id: String,
    pub created_at_ns: i64,
    pub updated_at_ns: i64,
    pub valid_from_ns: Option<i64>,
    pub valid_to_ns: Option<i64>,
    pub actor: Option<String>,
    pub origin: Option<String>,
    /// msgpack-serialised props tree bytes.
    pub props_bytes: Option<Vec<u8>>,
    /// msgpack-serialised label list bytes.
    pub labels_bytes: Option<Vec<u8>>,
    pub edge_src: Option<String>,
    pub edge_tgt: Option<String>,
    pub edge_type: Option<String>,
    pub edge_weight: Option<f64>,
    /// One entry per embedding (model order from record).
    pub embeddings: Vec<Vec<f32>>,
    /// User-defined columns projected from props or other sources.
    pub user_columns: Vec<Option<proximadb_data_model::ProximaValue>>,
}

impl FlatRow {
    /// Extract a `FlatRow` from a `ProximaRecord`.
    ///
    /// `props` and `labels` are serialised to msgpack bytes so they can be
    /// stored opaquely in the Props column stripe.
    pub fn from_record(record: &ProximaRecord) -> anyhow::Result<Self> {
        Self::from_record_with_user_columns(record, &[])
    }

    /// Extract a `FlatRow` from a `ProximaRecord`, projecting specific keys from `props`
    /// into the `user_columns` list.
    pub fn from_record_with_user_columns(
        record: &ProximaRecord,
        user_column_keys: &[String],
    ) -> anyhow::Result<Self> {
        let mut props = record.props.clone();

        // Extract promoted columns from props
        let mut user_columns = Vec::with_capacity(user_column_keys.len());
        for key in user_column_keys {
            let val = match props.remove(key) {
                Some(proximadb_records::ProximaTreeNode::Value(v)) => Some(v),
                _ => None,
            };
            user_columns.push(val);
        }

        let props_bytes = if props.is_empty() {
            None
        } else {
            Some(rmp_serde::to_vec_named(&props)?)
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

        // INT-2.5b: FlatRow holds fp32 vectors; promote non-Fp32
        // variants on the way in. INT-3's PAX writer will receive
        // typed values directly via a different code path that
        // bypasses FlatRow.
        let embeddings: Vec<Vec<f32>> = record
            .embeddings
            .iter()
            .map(|emb| emb.values.to_fp32_owned())
            .collect();

        Ok(FlatRow {
            oid: record.oid.clone(),
            tenant_id: record.tenant_id.clone(),
            created_at_ns: record.created_at_ns,
            updated_at_ns: record.updated_at_ns,
            valid_from_ns: record.valid_from_ns,
            valid_to_ns: record.valid_to_ns,
            actor: record.actor.clone(),
            origin: record.origin.clone(),
            props_bytes,
            labels_bytes,
            edge_src,
            edge_tgt,
            edge_type,
            edge_weight,
            embeddings,
            user_columns,
        })
    }

    /// Reconstruct a `ProximaRecord` from a decoded `FlatRow`.
    ///
    /// Embedding model IDs and user column keys are provided externally (from the collection schema).
    pub fn into_record(
        self,
        embedding_model_ids: &[String],
        user_column_keys: &[String],
    ) -> anyhow::Result<ProximaRecord> {
        use proximadb_records::{EdgeShape, EmbeddingCell, LabelSet};
        use std::collections::HashMap;

        let mut props = match self.props_bytes {
            Some(b) => rmp_serde::from_slice(&b)?,
            None => HashMap::new(),
        };

        // Merge user columns back into props
        for (i, val) in self.user_columns.into_iter().enumerate() {
            if let Some(key) = user_column_keys.get(i)
                && let Some(v) = val
            {
                props.insert(key.clone(), proximadb_records::ProximaTreeNode::Value(v));
            }
        }

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
                edge_type: etype,
                weight: self.edge_weight,
            }),
            _ => None,
        };

        let embeddings: Vec<EmbeddingCell> = self
            .embeddings
            .into_iter()
            .enumerate()
            .map(|(i, values)| {
                let dim = values.len() as u32;
                // INT-2.5b: FlatRow stores fp32, wrap in the typed
                // variant on the way out.
                EmbeddingCell {
                    model_id: embedding_model_ids
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| format!("model_{i}")),
                    modality: "dense".into(),
                    values: proximadb_records::EmbeddingValues::Fp32(values),
                    dim,
                    ..Default::default()
                }
            })
            .collect();

        Ok(ProximaRecord {
            oid: self.oid,
            tenant_id: self.tenant_id,
            created_at_ns: self.created_at_ns,
            updated_at_ns: self.updated_at_ns,
            valid_from_ns: self.valid_from_ns,
            valid_to_ns: self.valid_to_ns,
            actor: self.actor,
            origin: self.origin,
            props,
            labels,
            edge,
            embeddings,
            ..Default::default()
        })
    }

    /// Reconstruct every row of a decoded PAX block into [`FlatRow`]s — the
    /// reader-side inverse of the writer's per-row `add_record` + stripe flush.
    ///
    /// Decodes the canonical column stripes (`col_id::OID`/`TENANT_ID`/timestamps/
    /// provenance/`PROPS`/`LABELS`/`EDGE_*`) plus every contiguous embedding
    /// stripe from `EMBED_BASE`. Pair with [`FlatRow::into_record`] (which needs
    /// the embedding model ids + promoted user-column keys from the schema) to
    /// rebuild full `ProximaRecord`s.
    ///
    /// User-promoted columns (`USER_BASE`+) are not reconstructed here; the
    /// canonical PAX writer path (`from_record`) does not promote, so `PROPS`
    /// carries the full prop tree. Promoted-column reconstruction is a follow-up
    /// for the props-auto-promotion read path.
    pub fn from_block_reader(reader: &PaxBlockReader) -> anyhow::Result<Vec<FlatRow>> {
        let n = reader.row_count() as usize;
        // An absent stripe decodes to an empty Vec; every access below is via
        // `.get(i)` so a short/empty column simply yields `None` for each row.
        let oids = reader.decode_str_stripe(col_id::OID).unwrap_or_default();
        let tenants = reader
            .decode_str_stripe(col_id::TENANT_ID)
            .unwrap_or_default();
        let created = reader
            .decode_i64_stripe(col_id::CREATED_AT)
            .unwrap_or_default();
        let updated = reader
            .decode_i64_stripe(col_id::UPDATED_AT)
            .unwrap_or_default();
        let valid_from = reader
            .decode_i64_stripe(col_id::VALID_FROM)
            .unwrap_or_default();
        let valid_to = reader
            .decode_i64_stripe(col_id::VALID_TO)
            .unwrap_or_default();
        let actors = reader.decode_str_stripe(col_id::ACTOR).unwrap_or_default();
        let origins = reader.decode_str_stripe(col_id::ORIGIN).unwrap_or_default();
        let props = reader
            .decode_bytes_stripe(col_id::PROPS)
            .unwrap_or_default();
        let labels = reader
            .decode_bytes_stripe(col_id::LABELS)
            .unwrap_or_default();
        let edge_src = reader
            .decode_str_stripe(col_id::EDGE_SRC)
            .unwrap_or_default();
        let edge_tgt = reader
            .decode_str_stripe(col_id::EDGE_TGT)
            .unwrap_or_default();
        let edge_type = reader
            .decode_str_stripe(col_id::EDGE_TYPE)
            .unwrap_or_default();
        let edge_weight = reader
            .decode_f64_stripe(col_id::EDGE_WEIGHT)
            .unwrap_or_default();

        // Embedding stripes are contiguous from EMBED_BASE; probe until absent.
        let mut embedding_stripes: Vec<Vec<Option<Vec<f32>>>> = Vec::new();
        let mut e = 0;
        while let Some(stripe) = reader.decode_f32_vec_stripe(col_id::EMBED_BASE + e) {
            embedding_stripes.push(stripe);
            e += 1;
        }

        let at = |col: &[Option<String>], i: usize| col.get(i).cloned().flatten();
        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            let embeddings: Vec<Vec<f32>> = embedding_stripes
                .iter()
                .filter_map(|stripe| stripe.get(i).cloned().flatten())
                .collect();
            rows.push(FlatRow {
                oid: at(&oids, i).unwrap_or_default(),
                tenant_id: at(&tenants, i).unwrap_or_default(),
                created_at_ns: created.get(i).copied().flatten().unwrap_or(0),
                updated_at_ns: updated.get(i).copied().flatten().unwrap_or(0),
                valid_from_ns: valid_from.get(i).copied().flatten(),
                valid_to_ns: valid_to.get(i).copied().flatten(),
                actor: at(&actors, i),
                origin: at(&origins, i),
                props_bytes: props.get(i).cloned().flatten(),
                labels_bytes: labels.get(i).cloned().flatten(),
                edge_src: at(&edge_src, i),
                edge_tgt: at(&edge_tgt, i),
                edge_type: at(&edge_type, i),
                edge_weight: edge_weight.get(i).copied().flatten(),
                embeddings,
                user_columns: Vec::new(),
            });
        }
        Ok(rows)
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

    let new_min = if cur_min == 0 && cur_max == 0 {
        value
    } else {
        cur_min.min(value)
    };
    let new_max = if cur_min == 0 && cur_max == 0 {
        value
    } else {
        cur_max.max(value)
    };

    meta.min_val[0..8].copy_from_slice(&new_min.to_le_bytes());
    meta.max_val[0..8].copy_from_slice(&new_max.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        header::{BlockCompression, BlockMode},
        reader::PaxBlockReader,
        writer::PaxBlockWriter,
    };
    use proximadb_records::{EdgeShape, EmbeddingCell, LabelSet, ProximaRecord, ProximaTreeNode};
    use std::collections::HashMap;

    /// Build a richly populated ProximaRecord with all field types.
    fn rich_record(oid: &str) -> ProximaRecord {
        let mut props = HashMap::new();
        props.insert(
            "score".into(),
            ProximaTreeNode::Value(proximadb_data_model::ProximaValue::Float64(0.95)),
        );
        props.insert(
            "tag".into(),
            ProximaTreeNode::Value(proximadb_data_model::ProximaValue::String("hello".into())),
        );

        let mut labels = LabelSet::new();
        labels.insert("ml");
        labels.insert("test");

        ProximaRecord {
            oid: oid.into(),
            tenant_id: "tenant_x".into(),
            created_at_ns: 1_000_000,
            updated_at_ns: 2_000_000,
            valid_from_ns: Some(500_000),
            valid_to_ns: Some(9_000_000),
            actor: Some("agent-1".into()),
            origin: Some("rest-api".into()),
            props,
            labels,
            edge: Some(EdgeShape {
                source_id: "node_a".into(),
                target_id: "node_b".into(),
                edge_type: "knows".into(),
                weight: Some(0.75),
            }),
            embeddings: vec![EmbeddingCell {
                model_id: "text-embed-v1".into(),
                modality: "text".into(),
                values: proximadb_records::EmbeddingValues::Fp32(vec![0.1, 0.2, 0.3, 0.4]),
                dim: 4,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn flat_row_from_record_preserves_all_fields() {
        let rec = rich_record("r1");
        let flat = FlatRow::from_record(&rec).unwrap();

        assert_eq!(flat.oid, "r1");
        assert_eq!(flat.tenant_id, "tenant_x");
        assert_eq!(flat.created_at_ns, 1_000_000);
        assert_eq!(flat.valid_from_ns, Some(500_000));
        assert_eq!(flat.valid_to_ns, Some(9_000_000));
        assert_eq!(flat.actor.as_deref(), Some("agent-1"));
        assert_eq!(flat.origin.as_deref(), Some("rest-api"));
        assert!(flat.props_bytes.is_some(), "props should be serialised");
        assert!(flat.labels_bytes.is_some(), "labels should be serialised");
        assert_eq!(flat.edge_src.as_deref(), Some("node_a"));
        assert_eq!(flat.edge_tgt.as_deref(), Some("node_b"));
        assert_eq!(flat.edge_type.as_deref(), Some("knows"));
        assert_eq!(flat.edge_weight, Some(0.75));
        assert_eq!(flat.embeddings.len(), 1);
        assert_eq!(flat.embeddings[0], vec![0.1f32, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn pax_block_record_round_trip() {
        let records = vec![rich_record("r1"), rich_record("r2")];

        // Write
        let mut writer = PaxBlockWriter::new(
            BlockMode::Pax,
            BlockCompression::None,
            "collection_rt",
            0,
            1,
        );
        for r in &records {
            writer.add_record(r).unwrap();
        }
        let block_bytes = writer.flush().unwrap();

        // Read back field-by-field via stripe decoders
        let reader = PaxBlockReader::open(&block_bytes).unwrap();
        assert_eq!(reader.row_count(), 2);

        let oids = reader.decode_str_stripe(col_id::OID).unwrap();
        assert_eq!(oids[0].as_deref(), Some("r1"));
        assert_eq!(oids[1].as_deref(), Some("r2"));

        let actors = reader.decode_str_stripe(col_id::ACTOR).unwrap();
        assert_eq!(actors[0].as_deref(), Some("agent-1"));

        let valid_from = reader.decode_i64_stripe(col_id::VALID_FROM).unwrap();
        assert_eq!(valid_from[0], Some(500_000));

        let edge_src = reader.decode_str_stripe(col_id::EDGE_SRC).unwrap();
        assert_eq!(edge_src[0].as_deref(), Some("node_a"));

        let embeddings = reader.decode_f32_vec_stripe(col_id::EMBED_BASE).unwrap();
        let emb0 = embeddings[0].as_ref().unwrap();
        assert!((emb0[0] - 0.1f32).abs() < 1e-6);
        assert!((emb0[3] - 0.4f32).abs() < 1e-6);
    }

    #[test]
    fn olap_block_no_row_directory() {
        let mut writer =
            PaxBlockWriter::new(BlockMode::Olap, BlockCompression::None, "col_olap", 0, 0);
        writer
            .add_record(&ProximaRecord {
                oid: "x".into(),
                tenant_id: "t".into(),
                created_at_ns: 1,
                updated_at_ns: 1,
                ..Default::default()
            })
            .unwrap();
        let block_bytes = writer.flush().unwrap();
        let reader = PaxBlockReader::open(&block_bytes).unwrap();
        assert!(
            reader.row_directory().unwrap().is_none(),
            "OLAP block must not have row directory"
        );
    }
}

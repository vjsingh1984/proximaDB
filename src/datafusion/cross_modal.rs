// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Cross-modal source bridge (Track B — the §8 "zero-ETL multimodal" moat)
//!
//! Turns a **vector-search result set** into a DataFusion-joinable `(id, score)`
//! table, so a SINGLE SQL plan can join vector similarity against relational (and,
//! later, graph/document) data over the one canonical `ProximaRecord` spine. This is
//! the substrate of ProximaDB's durable differentiation per
//! `docs/12-design/DATA_WAREHOUSE_AND_ENGINEERING_COURSE_CORRECTION_2026_06_04.adoc`
//! §8: no competitor lets you filter-by-vector-similarity ⋈ relational-aggregate in
//! one query.
//!
//! ## Scope (this slice)
//! The conversion bridge + a proof that the join executes in one DataFusion plan. The
//! next slices: (a) a `VectorOpsPort`-backed `TableProvider` whose `scan` runs the
//! live search; (b) a frontend `VECTOR_SEARCH(...)` source + a
//! `proximadb-relational-algebra` source node that lowers (via the P4
//! `logical_lowering`) into the shared logical plane so the join is reachable from
//! pgwire SQL. Both reuse [`vector_matches_to_batch`] below.

use std::sync::Arc;

use arrow_array::{Float32Array, RecordBatch, StringArray};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};

use crate::proto::proximadb_v1::SearchVectorRecord;

/// The lean Arrow schema a vector-search source exposes for joins: `(id, score)`.
/// `id` joins against a relational key; `score` is the similarity the SQL can rank
/// or filter on.
pub fn vector_matches_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("score", DataType::Float32, false),
    ]))
}

/// Convert vector-search results into an `(id, score)` [`RecordBatch`] that DataFusion
/// can register as a table and JOIN against relational data on `id`. This is the
/// bridge from the vector modality into the shared (DataFusion) query plane.
pub fn vector_matches_to_batch(results: &[SearchVectorRecord]) -> Result<RecordBatch, ArrowError> {
    let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
    let scores: Vec<f32> = results.iter().map(|r| r.score as f32).collect();
    RecordBatch::try_new(
        vector_matches_schema(),
        vec![
            Arc::new(StringArray::from(ids)),
            Arc::new(Float32Array::from(scores)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;

    fn sv(id: &str, score: f64) -> SearchVectorRecord {
        SearchVectorRecord {
            id: id.to_string(),
            score,
            ..Default::default()
        }
    }

    #[test]
    fn vector_matches_batch_has_id_score_schema() {
        let batch = vector_matches_to_batch(&[sv("a", 0.9), sv("b", 0.5)]).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema().field(0).name(), "id");
        assert_eq!(batch.schema().field(1).name(), "score");
    }

    /// The moat proof: vector-search results JOIN a relational table in ONE
    /// DataFusion SQL plan (filter-by-similarity ⋈ relational), ordered by score.
    #[tokio::test]
    async fn vector_matches_join_relational_in_one_sql_plan() {
        let ctx = SessionContext::new();

        // Vector modality → joinable table (would come from the live VectorOpsPort
        // in the next slice; here we feed a fixed result set through the bridge).
        let matches = vector_matches_to_batch(&[sv("a", 0.95), sv("b", 0.80), sv("c", 0.70)])
            .expect("matches batch");
        ctx.register_table(
            "vmatches",
            Arc::new(MemTable::try_new(vector_matches_schema(), vec![vec![matches]]).unwrap()),
        )
        .unwrap();

        // Relational modality.
        let docs_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
        ]));
        let docs = RecordBatch::try_new(
            docs_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "z"])),
                Arc::new(StringArray::from(vec!["Alpha", "Bravo", "Zulu"])),
            ],
        )
        .unwrap();
        ctx.register_table(
            "docs",
            Arc::new(MemTable::try_new(docs_schema, vec![vec![docs]]).unwrap()),
        )
        .unwrap();

        // One SQL plan joining vector similarity with relational rows.
        let df = ctx
            .sql(
                "SELECT d.id, d.title, m.score \
                 FROM docs d JOIN vmatches m ON d.id = m.id \
                 ORDER BY m.score DESC",
            )
            .await
            .unwrap();
        let batches = df.collect().await.unwrap();
        let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        // a(Alpha,0.95) + b(Bravo,0.80); c has no doc, z has no vector match.
        assert_eq!(rows, 2);
    }
}

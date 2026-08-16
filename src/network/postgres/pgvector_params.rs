//! Extended-protocol parameter inference and result-column description for
//! pgvector queries (TD-102).
//!
//! SQL/filter lowering belongs to `relational_pipeline`; this module contains
//! only PostgreSQL extended-protocol metadata that must be available before a
//! portal executes. See `ADR-022-agent-memory-layer`.

use std::collections::HashSet;

use crate::network::postgres::types::{FieldDescription, PgType};

/// Infer parameter types from `$N` placeholders for an extended-protocol Parse
/// that carried no explicit type OIDs (TD-102). Returns one entry per
/// placeholder index 1..=max(N). `LIMIT`/`OFFSET $N` is typed `int8`; every
/// other placeholder defaults to `text` — which satisfies the shapes mem0's
/// PGVector provider emits (pgvector text literals + integer limits) and keeps
/// the downstream text-substitution path (`bind_parameters`) unchanged.
pub fn infer_param_types(query: &str) -> Vec<PgType> {
    let re = match regex::Regex::new(r"\$(\d+)") {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };
    let mut max_n = 0usize;
    let mut int_positions = HashSet::new();
    for caps in re.captures_iter(query) {
        let Some(n) = caps.get(1).and_then(|g| g.as_str().parse::<usize>().ok()) else {
            continue;
        };
        if n == 0 {
            continue;
        }
        max_n = max_n.max(n);
        let start = caps.get(0).map(|g| g.start()).unwrap_or(0);
        let trimmed = query[..start].to_ascii_uppercase();
        let trimmed = trimmed.trim_end();
        if trimmed.ends_with("LIMIT") || trimmed.ends_with("OFFSET") {
            int_positions.insert(n);
        }
    }
    (1..=max_n)
        .map(|i| {
            if int_positions.contains(&i) {
                PgType::Int8
            } else {
                PgType::Text
            }
        })
        .collect()
}

/// Result columns a prepared statement will return, for Describe(statement)
/// (TD-102). Projection is parsed by the same typed pgvector lowerer that executes
/// the query, keeping extended-protocol Describe consistent with emitted DataRows.
pub fn described_result_fields(query: &str) -> Result<Vec<FieldDescription>, String> {
    super::relational_pipeline::describe_pgvector_select(query)
        .map(|fields| fields.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    // TD-102: extended-protocol parameter inference + Describe column shape.
    #[test]
    fn infer_param_types_counts_and_types_placeholders() {
        let types = infer_param_types("SELECT id FROM t ORDER BY embedding <-> $1 LIMIT $2");
        assert_eq!(types.len(), 2);
        assert!(matches!(types[0], PgType::Text), "vector param is text");
        assert!(matches!(types[1], PgType::Int8), "LIMIT param is int8");
    }

    #[test]
    fn infer_param_types_handles_double_digit_and_gaps() {
        // max placeholder index drives the count; $10 must not be read as $1.
        let types = infer_param_types("WHERE a = $1 AND b = $10");
        assert_eq!(types.len(), 10);
        assert!(types.iter().all(|t| matches!(t, PgType::Text)));
    }

    #[test]
    fn infer_param_types_empty_when_no_placeholders() {
        assert!(infer_param_types("SELECT 1").is_empty());
    }

    #[test]
    fn described_result_fields_follow_projection() {
        let f = described_result_fields("SELECT id FROM t ORDER BY embedding <-> '[0.1]' LIMIT 5")
            .expect("valid vector projection");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name, "id");
        let f = described_result_fields(
            "SELECT id, embedding <-> $1 AS distance, payload FROM t \
             ORDER BY embedding <-> $1 LIMIT $2",
        )
        .expect("bound vector projection describes before binding");
        assert_eq!(
            f.iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "distance", "payload"]
        );
        // Non-vector statements report no columns.
        assert!(
            described_result_fields("SELECT 1")
                .expect("non-vector SELECT")
                .is_empty()
        );
        assert!(
            described_result_fields(
                "SELECT embedding <#> $1 FROM t ORDER BY embedding <=> $1 LIMIT 5"
            )
            .is_err(),
            "Describe must reject the same semantic mismatch as Execute"
        );
    }
}

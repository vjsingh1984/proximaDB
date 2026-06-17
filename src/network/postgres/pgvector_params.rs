//! Pure helpers for the pgwire pgvector path: WHERE-clause metadata filter
//! extraction (TD-100) and extended-protocol parameter inference + result
//! column description (TD-102).
//!
//! These are stateless functions extracted from `protocol.rs` so the protocol
//! handler keeps only thin call-sites and the parsing logic is unit-tested in
//! isolation. See `ADR-022-agent-memory-layer` (mem0's PGVector provider is the
//! consumer that drives these shapes).

use std::collections::HashSet;

use crate::core::search::{ComparisonOperator, FilterExpression};
use crate::network::postgres::types::{FieldDescription, PgType};

/// Build a metadata `FilterExpression` from the WHERE clause of a
/// pgvector-style similarity query (TD-100).
///
/// Supports the equality forms mem0's PGVector provider emits, AND-combined:
/// `payload->>'k' = 'v'`, `metadata->>'k' = 'v'`, and bare `col = v`. The
/// `payload->>`/`metadata->>` wrapper is stripped to the bare metadata key,
/// which is what the downstream search filter matches against. Predicates that
/// are not simple equality (ranges, `@>`, parameter placeholders) are skipped;
/// if nothing parses, returns `None` (unfiltered).
pub fn extract_metadata_filter_from_where(query: &str) -> Option<FilterExpression> {
    let upper = query.to_uppercase();
    let where_pos = upper.find(" WHERE ")?;
    // WHERE ends at ORDER BY / LIMIT / GROUP BY / end-of-string.
    let after = &query[where_pos + " WHERE ".len()..];
    let after_upper = &upper[where_pos + " WHERE ".len()..];
    let end = [" ORDER BY ", " LIMIT ", " GROUP BY "]
        .iter()
        .filter_map(|kw| after_upper.find(kw))
        .min()
        .unwrap_or(after.len());
    let where_clause = &after[..end];

    let mut parts = Vec::new();
    let conds: Vec<&str> = match regex::Regex::new(r"(?i)\s+and\s+") {
        Ok(re) => re.split(where_clause).collect(),
        Err(_) => where_clause.split(" AND ").collect(),
    };
    for raw in conds {
        let cond = raw.trim();
        if cond.is_empty() || !cond.contains('=') || cond.contains("@>") {
            continue; // skip empty, non-equality, and jsonb-containment forms
        }
        let Some(eq) = cond.find('=') else { continue };
        // Skip `!=`, `>=`, `<=`.
        if eq > 0 {
            let prev = cond.as_bytes()[eq - 1];
            if matches!(prev, b'!' | b'>' | b'<') {
                continue;
            }
        }
        let lhs = cond[..eq].trim();
        let rhs = cond[eq + 1..].trim();
        if rhs.contains('$') {
            continue; // unbound parameter placeholder
        }
        let field = normalize_metadata_field(lhs);
        if field.is_empty() {
            continue;
        }
        let value = sql_literal_to_json(rhs);
        parts.push(FilterExpression::Comparison {
            field,
            operator: ComparisonOperator::Equals,
            value,
        });
    }

    match parts.len() {
        0 => None,
        1 => parts.into_iter().next(),
        _ => Some(FilterExpression::And(parts)),
    }
}

/// Strip a `payload->>'k'` / `metadata->>'k'` / `col` LHS down to the bare
/// metadata key the search filter matches against.
pub fn normalize_metadata_field(lhs: &str) -> String {
    let lhs = lhs.trim();
    if let Some(pos) = lhs.find("->>") {
        let key = lhs[pos + 3..].trim();
        return key
            .trim_matches(|c| c == '\'' || c == '"' || c == ' ')
            .to_string();
    }
    // Strip a leading `payload.`/`metadata.` qualifier if present.
    for prefix in ["payload.", "metadata.", "PAYLOAD.", "METADATA."] {
        if let Some(rest) = lhs.strip_prefix(prefix) {
            return rest.trim().to_string();
        }
    }
    lhs.trim_matches(|c| c == '"').to_string()
}

/// Parse a SQL literal (quoted string / bool / int / float) into JSON.
pub fn sql_literal_to_json(rhs: &str) -> serde_json::Value {
    let t = rhs
        .trim()
        .trim_end_matches("::jsonb")
        .trim_end_matches("::text")
        .trim();
    if (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
        || (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
    {
        return serde_json::Value::String(t[1..t.len() - 1].to_string());
    }
    if t.eq_ignore_ascii_case("true") {
        return serde_json::Value::Bool(true);
    }
    if t.eq_ignore_ascii_case("false") {
        return serde_json::Value::Bool(false);
    }
    if let Ok(i) = t.parse::<i64>() {
        return serde_json::json!(i);
    }
    if let Ok(f) = t.parse::<f64>() {
        return serde_json::json!(f);
    }
    serde_json::Value::String(t.to_string())
}

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
/// (TD-102). A vector-search SELECT returns (id, distance, metadata) — the same
/// shape the vector-search executor streams; everything else reports no
/// columns. Keeps the extended-protocol Describe consistent with the DataRows
/// emitted during Execute.
pub fn described_result_fields(query: &str) -> Vec<FieldDescription> {
    let upper = query.to_uppercase();
    let is_vector_search = upper.contains("<->")
        || upper.contains("<=>")
        || upper.contains("<#>")
        || upper.contains("VECTOR_DISTANCE");
    if is_vector_search {
        vec![
            FieldDescription::new("id", PgType::Text),
            FieldDescription::new("distance", PgType::Float8),
            FieldDescription::new("metadata", PgType::Jsonb),
        ]
    } else {
        Vec::new()
    }
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
    fn described_result_fields_vector_search_has_three_columns() {
        let f = described_result_fields("SELECT id FROM t ORDER BY embedding <-> '[0.1]' LIMIT 5");
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].name, "id");
        assert_eq!(f[1].name, "distance");
        assert_eq!(f[2].name, "metadata");
        // Non-vector statements report no columns.
        assert!(described_result_fields("SELECT 1").is_empty());
    }

    // TD-100: WHERE metadata-filter pushdown for mem0-style pgvector queries.
    #[test]
    fn metadata_filter_parses_payload_jsonb_equality() {
        let q = "SELECT id, payload FROM mem WHERE payload->>'type' = 'fact' \
                 ORDER BY embedding <-> '[0.1,0.2]' LIMIT 5";
        let f = extract_metadata_filter_from_where(q).expect("filter");
        match f {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                assert_eq!(field, "type");
                assert!(matches!(operator, ComparisonOperator::Equals));
                assert_eq!(value, serde_json::Value::String("fact".to_string()));
            }
            other => panic!("expected single Comparison, got {other:?}"),
        }
    }

    #[test]
    fn metadata_filter_ands_multiple_equalities() {
        let q = "SELECT * FROM mem WHERE payload->>'type' = 'fact' AND tenant_id = 'acme' \
                 ORDER BY embedding <-> '[0.1]' LIMIT 3";
        match extract_metadata_filter_from_where(q).expect("filter") {
            FilterExpression::And(parts) => assert_eq!(parts.len(), 2),
            other => panic!("expected And, got {other:?}"),
        }
    }

    #[test]
    fn metadata_filter_none_without_where() {
        let q = "SELECT * FROM mem ORDER BY embedding <-> '[0.1,0.2]' LIMIT 5";
        assert!(extract_metadata_filter_from_where(q).is_none());
    }

    #[test]
    fn metadata_filter_skips_unbound_parameter() {
        let q = "SELECT * FROM mem WHERE payload->>'type' = $1 \
                 ORDER BY embedding <-> '[0.1]' LIMIT 5";
        assert!(extract_metadata_filter_from_where(q).is_none());
    }

    #[test]
    fn normalize_metadata_field_strips_jsonb_accessor() {
        assert_eq!(normalize_metadata_field("payload->>'type'"), "type");
        assert_eq!(normalize_metadata_field("metadata.tenant"), "tenant");
        assert_eq!(normalize_metadata_field("tenant_id"), "tenant_id");
    }

    #[test]
    fn sql_literal_to_json_parses_scalars() {
        assert_eq!(
            sql_literal_to_json("'fact'"),
            serde_json::Value::String("fact".to_string())
        );
        assert_eq!(sql_literal_to_json("true"), serde_json::json!(true));
        assert_eq!(sql_literal_to_json("42"), serde_json::json!(42));
    }
}

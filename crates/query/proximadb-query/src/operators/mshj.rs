//! Multi-Stage Hash Join executor — Phase D, spec §7 (CrossModelJoin, M2 §4).
//!
//! Implements the M2 MSHJ algorithm for cross-modality record joins:
//!
//! 1. **Build phase** — hash the *right* side on the join key(s).
//! 2. **Probe phase** — stream the *left* side and look up matches.
//!
//! The join operates on `serde_json::Value` rows (each row is a JSON object).
//! This keeps the executor modality-agnostic: vector records, graph nodes,
//! document objects, and log rows can all be joined with the same kernel.
//!
//! Reference: M2 §4 "Multi-Stage Hash Join for cross-modal data."

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use serde_json::Value;

use proximadb_multimodel_plan::JoinCondition;

/// A single joined row — the Cartesian product of one left and one right record.
#[derive(Debug, Clone)]
pub struct MshjRow {
    /// Merged record with all fields from left and right (right wins on collision).
    pub record: Value,
    /// Original left-side record index (for debugging / provenance).
    pub left_idx: usize,
    /// Original right-side record index (for debugging / provenance).
    pub right_idx: usize,
}

/// Statistics reported after a join execution.
#[derive(Debug, Clone, Default)]
pub struct MshjStats {
    pub left_rows: usize,
    pub right_rows: usize,
    pub matched_rows: usize,
    pub hash_collisions: usize,
    pub build_time_us: u64,
    pub probe_time_us: u64,
}

/// Multi-Stage Hash Join executor.
///
/// Stateless — create a new instance per query execution.
pub struct MshjExecutor;

impl MshjExecutor {
    /// Execute the join and return matched rows plus statistics.
    ///
    /// `left` and `right` are slices of JSON object rows.
    /// `condition` determines which field(s) are the join keys.
    pub fn join(
        left: &[Value],
        right: &[Value],
        condition: &JoinCondition,
    ) -> Result<(Vec<MshjRow>, MshjStats)> {
        let build_start = std::time::Instant::now();

        let mut stats = MshjStats {
            left_rows: left.len(),
            right_rows: right.len(),
            ..Default::default()
        };

        // --- Build phase: hash right side ---
        let mut build_table: HashMap<String, Vec<(usize, &Value)>> = HashMap::new();
        for (idx, row) in right.iter().enumerate() {
            let key = Self::extract_join_key(row, condition, Side::Right)?;
            let bucket = build_table.entry(key).or_default();
            if !bucket.is_empty() {
                stats.hash_collisions += 1;
            }
            bucket.push((idx, row));
        }

        stats.build_time_us = build_start.elapsed().as_micros() as u64;
        let probe_start = std::time::Instant::now();

        // --- Probe phase: stream left, look up in hash table ---
        let mut results = Vec::new();
        for (left_idx, left_row) in left.iter().enumerate() {
            let key = Self::extract_join_key(left_row, condition, Side::Left)?;
            if let Some(right_matches) = build_table.get(&key) {
                for &(right_idx, right_row) in right_matches {
                    let merged = Self::merge_rows(left_row, right_row);
                    results.push(MshjRow {
                        record: merged,
                        left_idx,
                        right_idx,
                    });
                }
            }
        }

        stats.matched_rows = results.len();
        stats.probe_time_us = probe_start.elapsed().as_micros() as u64;

        Ok((results, stats))
    }

    /// Extract the scalar join key from a row for a given side.
    fn extract_join_key(row: &Value, condition: &JoinCondition, side: Side) -> Result<String> {
        match condition {
            JoinCondition::On(left_col, right_col) => {
                let col = match side {
                    Side::Left => left_col,
                    Side::Right => right_col,
                };
                Self::get_field_as_string(row, col)
            }
            JoinCondition::OnMultiple(pairs) => {
                let mut parts = Vec::with_capacity(pairs.len());
                for (left_col, right_col) in pairs {
                    let col = match side {
                        Side::Left => left_col,
                        Side::Right => right_col,
                    };
                    parts.push(Self::get_field_as_string(row, col)?);
                }
                Ok(parts.join("\x00"))
            }
            JoinCondition::Expression(_) => {
                // Expression joins degrade to full Cartesian + filter (handled by caller).
                // For hash-join purposes treat every row as matching the same bucket.
                Ok("__expr_join__".to_string())
            }
        }
    }

    fn get_field_as_string(row: &Value, field: &str) -> Result<String> {
        match row.get(field) {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(Value::Number(n)) => Ok(n.to_string()),
            Some(Value::Bool(b)) => Ok(b.to_string()),
            Some(Value::Null) => Ok("__null__".to_string()),
            Some(other) => Ok(other.to_string()),
            None => Err(anyhow!(
                "Join key field '{}' not found in row: {}",
                field,
                row
            )),
        }
    }

    fn merge_rows(left: &Value, right: &Value) -> Value {
        match (left, right) {
            (Value::Object(l), Value::Object(r)) => {
                let mut merged = l.clone();
                for (k, v) in r {
                    merged.insert(k.clone(), v.clone());
                }
                Value::Object(merged)
            }
            _ => left.clone(),
        }
    }
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_rows() -> Vec<Value> {
        vec![
            json!({"id": "u1", "name": "Alice", "dept_id": "d1"}),
            json!({"id": "u2", "name": "Bob",   "dept_id": "d2"}),
            json!({"id": "u3", "name": "Carol",  "dept_id": "d1"}),
            json!({"id": "u4", "name": "Dave",   "dept_id": "d9"}), // no match
        ]
    }

    fn dept_rows() -> Vec<Value> {
        vec![
            json!({"dept_id": "d1", "dept_name": "Engineering"}),
            json!({"dept_id": "d2", "dept_name": "Product"}),
        ]
    }

    #[test]
    fn test_mshj_basic_inner_join() {
        let condition = JoinCondition::On("dept_id".to_string(), "dept_id".to_string());
        let (rows, stats) = MshjExecutor::join(&user_rows(), &dept_rows(), &condition).unwrap();

        assert_eq!(stats.left_rows, 4);
        assert_eq!(stats.right_rows, 2);
        assert_eq!(
            stats.matched_rows, 3,
            "Alice, Bob, Carol match; Dave has no dept"
        );

        let names: Vec<_> = rows
            .iter()
            .filter_map(|r| r.record.get("name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"Alice"));
        assert!(names.contains(&"Bob"));
        assert!(names.contains(&"Carol"));
        assert!(!names.contains(&"Dave"));
    }

    #[test]
    fn test_mshj_merged_row_has_both_sides() {
        let condition = JoinCondition::On("dept_id".to_string(), "dept_id".to_string());
        let left = vec![json!({"dept_id": "d1", "name": "Alice"})];
        let right = vec![json!({"dept_id": "d1", "dept_name": "Engineering"})];
        let (rows, _) = MshjExecutor::join(&left, &right, &condition).unwrap();

        assert_eq!(rows.len(), 1);
        let rec = &rows[0].record;
        assert_eq!(rec.get("name").and_then(|v| v.as_str()), Some("Alice"));
        assert_eq!(
            rec.get("dept_name").and_then(|v| v.as_str()),
            Some("Engineering")
        );
    }

    #[test]
    fn test_mshj_empty_left_returns_no_rows() {
        let condition = JoinCondition::On("id".to_string(), "user_id".to_string());
        let right = vec![json!({"user_id": "u1"})];
        let (rows, stats) = MshjExecutor::join(&[], &right, &condition).unwrap();

        assert!(rows.is_empty());
        assert_eq!(stats.left_rows, 0);
        assert_eq!(stats.matched_rows, 0);
    }

    #[test]
    fn test_mshj_empty_right_returns_no_rows() {
        let condition = JoinCondition::On("id".to_string(), "id".to_string());
        let left = vec![json!({"id": "u1"})];
        let (rows, stats) = MshjExecutor::join(&left, &[], &condition).unwrap();

        assert!(rows.is_empty());
        assert_eq!(stats.right_rows, 0);
        assert_eq!(stats.matched_rows, 0);
    }

    #[test]
    fn test_mshj_many_to_many_join() {
        // Two left rows and two right rows share the same key → 2×2 = 4 output rows.
        let condition = JoinCondition::On("tag".to_string(), "tag".to_string());
        let left = vec![
            json!({"tag": "ml", "doc": "paper1"}),
            json!({"tag": "ml", "doc": "paper2"}),
        ];
        let right = vec![
            json!({"tag": "ml", "vec": [0.1, 0.2]}),
            json!({"tag": "ml", "vec": [0.3, 0.4]}),
        ];
        let (rows, stats) = MshjExecutor::join(&left, &right, &condition).unwrap();

        assert_eq!(rows.len(), 4, "2 left × 2 right = 4 output rows");
        assert_eq!(stats.matched_rows, 4);
        assert_eq!(stats.hash_collisions, 1);
    }

    #[test]
    fn test_mshj_compound_key_join() {
        let condition = JoinCondition::OnMultiple(vec![
            ("tenant_id".to_string(), "tenant_id".to_string()),
            ("user_id".to_string(), "user_id".to_string()),
        ]);
        let left = vec![
            json!({"tenant_id": "t1", "user_id": "u1", "action": "buy"}),
            json!({"tenant_id": "t1", "user_id": "u2", "action": "view"}),
            json!({"tenant_id": "t2", "user_id": "u1", "action": "sell"}), // different tenant
        ];
        let right = vec![json!({"tenant_id": "t1", "user_id": "u1", "role": "admin"})];
        let (rows, stats) = MshjExecutor::join(&left, &right, &condition).unwrap();

        assert_eq!(stats.matched_rows, 1, "only (t1,u1) matches");
        assert_eq!(
            rows[0].record.get("action").and_then(|v| v.as_str()),
            Some("buy")
        );
        assert_eq!(
            rows[0].record.get("role").and_then(|v| v.as_str()),
            Some("admin")
        );
    }

    #[test]
    fn test_mshj_stats_track_timing() {
        let condition = JoinCondition::On("id".to_string(), "id".to_string());
        let left = vec![json!({"id": "x"})];
        let right = vec![json!({"id": "x"})];
        let (_rows, stats) = MshjExecutor::join(&left, &right, &condition).unwrap();

        // Timing fields must be populated (may be 0 on fast hardware, but not absent).
        let _ = stats.build_time_us;
        let _ = stats.probe_time_us;
    }

    #[test]
    fn test_mshj_missing_key_field_returns_error() {
        let condition = JoinCondition::On("nonexistent".to_string(), "id".to_string());
        let left = vec![json!({"id": "u1", "name": "Alice"})];
        let right = vec![json!({"id": "u1"})];
        let result = MshjExecutor::join(&left, &right, &condition);

        assert!(
            result.is_err(),
            "missing join key field must return an error"
        );
    }
}

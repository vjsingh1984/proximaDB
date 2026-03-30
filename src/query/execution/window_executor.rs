//! Window Function Execution Engine
//!
//! Evaluates SQL window functions over partitioned and ordered row sets.
//! Supports ranking (ROW_NUMBER, RANK, DENSE_RANK), aggregate (SUM, AVG, COUNT,
//! MIN, MAX), and navigation (LAG, LEAD, FIRST_VALUE, LAST_VALUE) functions.
//!
//! Execution algorithm:
//!   1. Partition rows by PARTITION BY expressions
//!   2. Sort within each partition by ORDER BY expressions
//!   3. Compute window function values per row
//!   4. Merge results back into the original row set

use super::QueryRow;
use anyhow::{Result, anyhow};
use serde_json::Value as JsonValue;
use std::cmp::Ordering;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Supported window functions.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowFunction {
    // Ranking
    RowNumber,
    Rank,
    DenseRank,

    // Aggregate
    Sum,
    Avg,
    Count,
    Min,
    Max,

    // Navigation
    Lag,
    Lead,
    FirstValue,
    LastValue,
}

impl WindowFunction {
    /// Parse a function name (case-insensitive) into a `WindowFunction`.
    pub fn from_name(name: &str) -> Result<Self> {
        match name.to_uppercase().as_str() {
            "ROW_NUMBER" => Ok(Self::RowNumber),
            "RANK" => Ok(Self::Rank),
            "DENSE_RANK" => Ok(Self::DenseRank),
            "SUM" => Ok(Self::Sum),
            "AVG" => Ok(Self::Avg),
            "COUNT" => Ok(Self::Count),
            "MIN" => Ok(Self::Min),
            "MAX" => Ok(Self::Max),
            "LAG" => Ok(Self::Lag),
            "LEAD" => Ok(Self::Lead),
            "FIRST_VALUE" => Ok(Self::FirstValue),
            "LAST_VALUE" => Ok(Self::LastValue),
            other => Err(anyhow!("Unsupported window function: {}", other)),
        }
    }

    /// Returns true if the function is a ranking function.
    pub fn is_ranking(&self) -> bool {
        matches!(self, Self::RowNumber | Self::Rank | Self::DenseRank)
    }

    /// Returns true if the function is a navigation function.
    pub fn is_navigation(&self) -> bool {
        matches!(
            self,
            Self::Lag | Self::Lead | Self::FirstValue | Self::LastValue
        )
    }
}

/// Sort direction for ORDER BY within a window.
#[derive(Debug, Clone, PartialEq)]
pub enum SortDirection {
    Asc,
    Desc,
}

/// A single ORDER BY element inside a window specification.
#[derive(Debug, Clone)]
pub struct WindowOrderBy {
    /// Field name to sort by.
    pub field: String,
    /// Sort direction.
    pub direction: SortDirection,
}

/// Window frame boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameBound {
    /// UNBOUNDED PRECEDING / FOLLOWING
    Unbounded,
    /// CURRENT ROW
    CurrentRow,
    /// N PRECEDING / N FOLLOWING
    Offset(u64),
}

/// Window frame definition (ROWS-based).
#[derive(Debug, Clone)]
pub struct FrameDefinition {
    pub start: FrameBound,
    pub end: FrameBound,
}

impl Default for FrameDefinition {
    /// Default frame: ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW.
    fn default() -> Self {
        Self {
            start: FrameBound::Unbounded,
            end: FrameBound::CurrentRow,
        }
    }
}

/// Complete window specification: partitioning, ordering, and framing.
#[derive(Debug, Clone)]
pub struct WindowSpec {
    /// Fields to partition by.
    pub partition_by: Vec<String>,
    /// ORDER BY within each partition.
    pub order_by: Vec<WindowOrderBy>,
    /// Frame definition for aggregate functions.
    pub frame: FrameDefinition,
}

/// A window function call together with its specification.
#[derive(Debug, Clone)]
pub struct WindowFunctionCall {
    /// The window function to evaluate.
    pub function: WindowFunction,
    /// Arguments: for aggregates the first element is the field name; for
    /// LAG/LEAD it is the field name with an optional offset (second element
    /// as a numeric string) and default value (third element).
    pub args: Vec<String>,
    /// Window specification (partitioning, ordering, framing).
    pub spec: WindowSpec,
    /// Output field name where the result will be stored.
    pub output_field: String,
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Executes window functions over a set of query rows.
pub struct WindowExecutor;

impl WindowExecutor {
    /// Create a new `WindowExecutor`.
    pub fn new() -> Self {
        Self
    }

    /// Execute one or more window function definitions against the provided rows.
    ///
    /// For each `WindowFunctionCall` the executor:
    ///   1. Partitions rows by `spec.partition_by`
    ///   2. Sorts each partition by `spec.order_by`
    ///   3. Computes the function value for every row
    ///   4. Writes the result into `output_field`
    ///
    /// Returns the modified rows (same order as the input).
    pub fn execute_window_functions(
        &self,
        rows: Vec<QueryRow>,
        window_calls: &[WindowFunctionCall],
    ) -> Result<Vec<QueryRow>> {
        if rows.is_empty() {
            return Ok(rows);
        }

        // We tag each row with its original index so we can restore order after
        // partitioning and sorting.
        let mut indexed_rows: Vec<(usize, QueryRow)> = rows.into_iter().enumerate().collect();

        for call in window_calls {
            // 1. Partition
            let partitions = Self::partition_rows(&indexed_rows, &call.spec.partition_by);

            // 2. Sort each partition and compute
            let mut results: HashMap<usize, JsonValue> = HashMap::new();

            for mut partition in partitions {
                Self::sort_partition(&mut partition, &call.spec.order_by)?;
                Self::compute_function(&partition, call, &mut results)?;
            }

            // 3. Write results back into the rows
            for (idx, row) in &mut indexed_rows {
                if let Some(val) = results.remove(idx) {
                    row.fields.insert(call.output_field.clone(), val);
                }
            }
        }

        // Restore original order
        indexed_rows.sort_by_key(|(idx, _)| *idx);
        Ok(indexed_rows.into_iter().map(|(_, row)| row).collect())
    }

    // -- internal helpers ---------------------------------------------------

    /// Partition rows into groups sharing the same PARTITION BY key values.
    fn partition_rows(
        rows: &[(usize, QueryRow)],
        partition_by: &[String],
    ) -> Vec<Vec<(usize, QueryRow)>> {
        if partition_by.is_empty() {
            // Entire input is one partition.
            return vec![rows.to_vec()];
        }

        let mut map: HashMap<String, Vec<(usize, QueryRow)>> = HashMap::new();
        for (idx, row) in rows {
            let key = Self::partition_key(row, partition_by);
            map.entry(key).or_default().push((*idx, row.clone()));
        }

        map.into_values().collect()
    }

    /// Build a deterministic partition key string from the row fields.
    fn partition_key(row: &QueryRow, fields: &[String]) -> String {
        fields
            .iter()
            .map(|f| {
                row.fields
                    .get(f).map_or_else(|| "null".to_string(), |v| v.to_string())
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    /// Sort a partition in-place according to ORDER BY specifications.
    fn sort_partition(
        partition: &mut [(usize, QueryRow)],
        order_by: &[WindowOrderBy],
    ) -> Result<()> {
        partition.sort_by(|a, b| {
            for ob in order_by {
                let va = a.1.fields.get(&ob.field);
                let vb = b.1.fields.get(&ob.field);
                let cmp = Self::compare_json_values(va, vb);
                let cmp = match ob.direction {
                    SortDirection::Asc => cmp,
                    SortDirection::Desc => cmp.reverse(),
                };
                if cmp != Ordering::Equal {
                    return cmp;
                }
            }
            Ordering::Equal
        });
        Ok(())
    }

    /// Compare two optional JSON values for ordering.
    fn compare_json_values(a: Option<&JsonValue>, b: Option<&JsonValue>) -> Ordering {
        match (a, b) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(va), Some(vb)) => {
                // Try numeric comparison first
                if let (Some(na), Some(nb)) = (va.as_f64(), vb.as_f64()) {
                    return na.partial_cmp(&nb).unwrap_or(Ordering::Equal);
                }
                // Fall back to string comparison
                let sa = va
                    .as_str().map_or_else(|| va.to_string(), |s| s.to_string());
                let sb = vb
                    .as_str().map_or_else(|| vb.to_string(), |s| s.to_string());
                sa.cmp(&sb)
            }
        }
    }

    /// Compute the window function value for every row in a sorted partition.
    fn compute_function(
        partition: &[(usize, QueryRow)],
        call: &WindowFunctionCall,
        results: &mut HashMap<usize, JsonValue>,
    ) -> Result<()> {
        match &call.function {
            // --- Ranking -------------------------------------------------------
            WindowFunction::RowNumber => {
                for (pos, (idx, _)) in partition.iter().enumerate() {
                    results.insert(*idx, JsonValue::from((pos + 1) as u64));
                }
            }
            WindowFunction::Rank => {
                Self::compute_rank(partition, &call.spec.order_by, results, false)?;
            }
            WindowFunction::DenseRank => {
                Self::compute_rank(partition, &call.spec.order_by, results, true)?;
            }

            // --- Aggregates ----------------------------------------------------
            WindowFunction::Sum
            | WindowFunction::Avg
            | WindowFunction::Count
            | WindowFunction::Min
            | WindowFunction::Max => {
                Self::compute_aggregate(partition, call, results)?;
            }

            // --- Navigation ----------------------------------------------------
            WindowFunction::Lag => {
                Self::compute_navigation(partition, call, results, true)?;
            }
            WindowFunction::Lead => {
                Self::compute_navigation(partition, call, results, false)?;
            }
            WindowFunction::FirstValue => {
                Self::compute_first_last(partition, call, results, true)?;
            }
            WindowFunction::LastValue => {
                Self::compute_first_last(partition, call, results, false)?;
            }
        }
        Ok(())
    }

    // -- Ranking helpers ----------------------------------------------------

    /// Compute RANK or DENSE_RANK for a sorted partition.
    fn compute_rank(
        partition: &[(usize, QueryRow)],
        order_by: &[WindowOrderBy],
        results: &mut HashMap<usize, JsonValue>,
        dense: bool,
    ) -> Result<()> {
        if partition.is_empty() {
            return Ok(());
        }

        let mut current_rank: u64 = 1;
        let mut dense_rank: u64 = 1;

        // First row always gets rank 1
        results.insert(partition[0].0, JsonValue::from(1_u64));

        for i in 1..partition.len() {
            let same = Self::rows_equal_on_order_by(&partition[i - 1].1, &partition[i].1, order_by);
            if same {
                // Tie: same rank as previous
                let rank = if dense { dense_rank } else { current_rank };
                results.insert(partition[i].0, JsonValue::from(rank));
            } else {
                if dense {
                    dense_rank += 1;
                    results.insert(partition[i].0, JsonValue::from(dense_rank));
                } else {
                    current_rank = (i as u64) + 1;
                    results.insert(partition[i].0, JsonValue::from(current_rank));
                }
            }
        }
        Ok(())
    }

    /// Check whether two rows are equal on the ORDER BY fields.
    fn rows_equal_on_order_by(a: &QueryRow, b: &QueryRow, order_by: &[WindowOrderBy]) -> bool {
        order_by.iter().all(|ob| {
            let va = a.fields.get(&ob.field);
            let vb = b.fields.get(&ob.field);
            Self::compare_json_values(va, vb) == Ordering::Equal
        })
    }

    // -- Aggregate helpers --------------------------------------------------

    /// Compute an aggregate window function with frame support.
    fn compute_aggregate(
        partition: &[(usize, QueryRow)],
        call: &WindowFunctionCall,
        results: &mut HashMap<usize, JsonValue>,
    ) -> Result<()> {
        let field = call
            .args
            .first()
            .ok_or_else(|| anyhow!("Aggregate window function requires at least one argument"))?;

        let len = partition.len();

        for current_pos in 0..len {
            let (start, end) = Self::resolve_frame(&call.spec.frame, current_pos, len);

            let frame_values: Vec<f64> = (start..=end)
                .filter_map(|i| {
                    partition
                        .get(i)
                        .and_then(|(_, row)| row.fields.get(field))
                        .and_then(|v| v.as_f64())
                })
                .collect();

            let result = match &call.function {
                WindowFunction::Sum => JsonValue::from(frame_values.iter().sum::<f64>()),
                WindowFunction::Avg => {
                    if frame_values.is_empty() {
                        JsonValue::Null
                    } else {
                        JsonValue::from(
                            frame_values.iter().sum::<f64>() / frame_values.len() as f64,
                        )
                    }
                }
                WindowFunction::Count => {
                    // COUNT counts non-null values in the frame
                    let count: u64 = (start..=end)
                        .filter(|i| {
                            partition
                                .get(*i)
                                .and_then(|(_, row)| row.fields.get(field))
                                .is_some_and(|v| !v.is_null())
                        })
                        .count() as u64;
                    JsonValue::from(count)
                }
                WindowFunction::Min => {
                    if frame_values.is_empty() {
                        JsonValue::Null
                    } else {
                        let min = frame_values.iter().copied().fold(f64::INFINITY, f64::min);
                        JsonValue::from(min)
                    }
                }
                WindowFunction::Max => {
                    if frame_values.is_empty() {
                        JsonValue::Null
                    } else {
                        let max = frame_values
                            .iter()
                            .copied()
                            .fold(f64::NEG_INFINITY, f64::max);
                        JsonValue::from(max)
                    }
                }
                _ => JsonValue::Null,
            };

            results.insert(partition[current_pos].0, result);
        }
        Ok(())
    }

    /// Resolve a `FrameDefinition` into concrete start/end indices.
    fn resolve_frame(frame: &FrameDefinition, current: usize, len: usize) -> (usize, usize) {
        let start = match &frame.start {
            FrameBound::Unbounded => 0,
            FrameBound::CurrentRow => current,
            FrameBound::Offset(n) => current.saturating_sub(*n as usize),
        };
        let end = match &frame.end {
            FrameBound::Unbounded => len.saturating_sub(1),
            FrameBound::CurrentRow => current,
            FrameBound::Offset(n) => (current + *n as usize).min(len.saturating_sub(1)),
        };
        (start, end)
    }

    // -- Navigation helpers -------------------------------------------------

    /// Compute LAG or LEAD.
    fn compute_navigation(
        partition: &[(usize, QueryRow)],
        call: &WindowFunctionCall,
        results: &mut HashMap<usize, JsonValue>,
        is_lag: bool,
    ) -> Result<()> {
        let field = call
            .args
            .first()
            .ok_or_else(|| anyhow!("LAG/LEAD requires a field argument"))?;

        let offset: usize = call
            .args
            .get(1)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);

        let default_val: JsonValue = call
            .args
            .get(2)
            .map_or(JsonValue::Null, |s| {
                // Try to parse as number, otherwise use as string
                if let Ok(n) = s.parse::<f64>() {
                    JsonValue::from(n)
                } else if s == "null" {
                    JsonValue::Null
                } else {
                    JsonValue::from(s.clone())
                }
            });

        for (pos, (idx, _)) in partition.iter().enumerate() {
            let target_pos = if is_lag {
                if pos >= offset {
                    Some(pos - offset)
                } else {
                    None
                }
            } else {
                let t = pos + offset;
                if t < partition.len() { Some(t) } else { None }
            };

            let val = target_pos
                .and_then(|tp| partition.get(tp))
                .and_then(|(_, row)| row.fields.get(field))
                .cloned()
                .unwrap_or_else(|| default_val.clone());

            results.insert(*idx, val);
        }
        Ok(())
    }

    /// Compute FIRST_VALUE or LAST_VALUE within the frame.
    fn compute_first_last(
        partition: &[(usize, QueryRow)],
        call: &WindowFunctionCall,
        results: &mut HashMap<usize, JsonValue>,
        is_first: bool,
    ) -> Result<()> {
        let field = call
            .args
            .first()
            .ok_or_else(|| anyhow!("FIRST_VALUE/LAST_VALUE requires a field argument"))?;

        let len = partition.len();

        for current_pos in 0..len {
            let (start, end) = Self::resolve_frame(&call.spec.frame, current_pos, len);
            let target = if is_first { start } else { end };

            let val = partition
                .get(target)
                .and_then(|(_, row)| row.fields.get(field))
                .cloned()
                .unwrap_or(JsonValue::Null);

            results.insert(partition[current_pos].0, val);
        }
        Ok(())
    }
}

impl Default for WindowExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: create a `QueryRow` from a list of (field, value) pairs.
    fn make_row(pairs: Vec<(&str, JsonValue)>) -> QueryRow {
        let mut fields = HashMap::new();
        for (k, v) in pairs {
            fields.insert(k.to_string(), v);
        }
        QueryRow {
            fields,
            similarity_score: None,
            graph_distance: None,
            provenance: None,
        }
    }

    fn default_spec(partition_by: Vec<&str>, order_by: Vec<(&str, SortDirection)>) -> WindowSpec {
        WindowSpec {
            partition_by: partition_by.into_iter().map(String::from).collect(),
            order_by: order_by
                .into_iter()
                .map(|(f, d)| WindowOrderBy {
                    field: f.to_string(),
                    direction: d,
                })
                .collect(),
            frame: FrameDefinition::default(),
        }
    }

    // -- Ranking tests ------------------------------------------------------

    #[test]
    fn test_row_number_single_partition() {
        let rows = vec![
            make_row(vec![("name", json!("alice")), ("score", json!(90))]),
            make_row(vec![("name", json!("bob")), ("score", json!(80))]),
            make_row(vec![("name", json!("carol")), ("score", json!(70))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::RowNumber,
            args: vec![],
            spec: default_spec(vec![], vec![("score", SortDirection::Desc)]),
            output_field: "rn".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // Rows should be back in original order; alice(90)->1, bob(80)->2, carol(70)->3
        assert_eq!(result[0].fields["rn"], json!(1));
        assert_eq!(result[1].fields["rn"], json!(2));
        assert_eq!(result[2].fields["rn"], json!(3));
    }

    #[test]
    fn test_row_number_with_partitions() {
        let rows = vec![
            make_row(vec![("dept", json!("eng")), ("score", json!(90))]),
            make_row(vec![("dept", json!("eng")), ("score", json!(80))]),
            make_row(vec![("dept", json!("sales")), ("score", json!(70))]),
            make_row(vec![("dept", json!("sales")), ("score", json!(95))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::RowNumber,
            args: vec![],
            spec: default_spec(vec!["dept"], vec![("score", SortDirection::Desc)]),
            output_field: "rn".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // eng partition: 90->1, 80->2
        assert_eq!(result[0].fields["rn"], json!(1));
        assert_eq!(result[1].fields["rn"], json!(2));
        // sales partition: 95->1, 70->2
        assert_eq!(result[2].fields["rn"], json!(2));
        assert_eq!(result[3].fields["rn"], json!(1));
    }

    #[test]
    fn test_rank_with_ties() {
        let rows = vec![
            make_row(vec![("name", json!("a")), ("score", json!(100))]),
            make_row(vec![("name", json!("b")), ("score", json!(100))]),
            make_row(vec![("name", json!("c")), ("score", json!(90))]),
            make_row(vec![("name", json!("d")), ("score", json!(80))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::Rank,
            args: vec![],
            spec: default_spec(vec![], vec![("score", SortDirection::Desc)]),
            output_field: "rnk".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // Tied at 100 get rank 1, then 90 gets rank 3, 80 gets rank 4
        assert_eq!(result[0].fields["rnk"], json!(1));
        assert_eq!(result[1].fields["rnk"], json!(1));
        assert_eq!(result[2].fields["rnk"], json!(3));
        assert_eq!(result[3].fields["rnk"], json!(4));
    }

    #[test]
    fn test_dense_rank_with_ties() {
        let rows = vec![
            make_row(vec![("name", json!("a")), ("score", json!(100))]),
            make_row(vec![("name", json!("b")), ("score", json!(100))]),
            make_row(vec![("name", json!("c")), ("score", json!(90))]),
            make_row(vec![("name", json!("d")), ("score", json!(80))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::DenseRank,
            args: vec![],
            spec: default_spec(vec![], vec![("score", SortDirection::Desc)]),
            output_field: "drnk".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // Tied at 100 get dense_rank 1, 90->2, 80->3
        assert_eq!(result[0].fields["drnk"], json!(1));
        assert_eq!(result[1].fields["drnk"], json!(1));
        assert_eq!(result[2].fields["drnk"], json!(2));
        assert_eq!(result[3].fields["drnk"], json!(3));
    }

    // -- Aggregate tests ----------------------------------------------------

    #[test]
    fn test_sum_over_default_frame() {
        // Default frame: UNBOUNDED PRECEDING to CURRENT ROW (running sum)
        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::Sum,
            args: vec!["val".to_string()],
            spec: default_spec(vec![], vec![("val", SortDirection::Asc)]),
            output_field: "running_sum".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        assert_eq!(result[0].fields["running_sum"], json!(10.0));
        assert_eq!(result[1].fields["running_sum"], json!(30.0));
        assert_eq!(result[2].fields["running_sum"], json!(60.0));
    }

    #[test]
    fn test_avg_over_full_frame() {
        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::Avg,
            args: vec!["val".to_string()],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![WindowOrderBy {
                    field: "val".to_string(),
                    direction: SortDirection::Asc,
                }],
                frame: FrameDefinition {
                    start: FrameBound::Unbounded,
                    end: FrameBound::Unbounded,
                },
            },
            output_field: "avg_val".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // Full frame avg = 20.0 for all rows
        assert_eq!(result[0].fields["avg_val"], json!(20.0));
        assert_eq!(result[1].fields["avg_val"], json!(20.0));
        assert_eq!(result[2].fields["avg_val"], json!(20.0));
    }

    #[test]
    fn test_count_over_frame() {
        let rows = vec![
            make_row(vec![("val", json!(1))]),
            make_row(vec![("val", json!(2))]),
            make_row(vec![("val", json!(3))]),
            make_row(vec![("val", json!(4))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::Count,
            args: vec!["val".to_string()],
            spec: default_spec(vec![], vec![("val", SortDirection::Asc)]),
            output_field: "cnt".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // Running count: 1, 2, 3, 4
        assert_eq!(result[0].fields["cnt"], json!(1));
        assert_eq!(result[1].fields["cnt"], json!(2));
        assert_eq!(result[2].fields["cnt"], json!(3));
        assert_eq!(result[3].fields["cnt"], json!(4));
    }

    #[test]
    fn test_min_max_over_frame() {
        let rows = vec![
            make_row(vec![("val", json!(30))]),
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
        ];

        let min_call = WindowFunctionCall {
            function: WindowFunction::Min,
            args: vec!["val".to_string()],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![WindowOrderBy {
                    field: "val".to_string(),
                    direction: SortDirection::Asc,
                }],
                frame: FrameDefinition {
                    start: FrameBound::Unbounded,
                    end: FrameBound::Unbounded,
                },
            },
            output_field: "min_val".to_string(),
        };

        let max_call = WindowFunctionCall {
            function: WindowFunction::Max,
            args: vec!["val".to_string()],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![WindowOrderBy {
                    field: "val".to_string(),
                    direction: SortDirection::Asc,
                }],
                frame: FrameDefinition {
                    start: FrameBound::Unbounded,
                    end: FrameBound::Unbounded,
                },
            },
            output_field: "max_val".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor
            .execute_window_functions(rows, &[min_call, max_call])
            .unwrap();

        // Full-frame min/max should be the same for all rows
        for row in &result {
            assert_eq!(row.fields["min_val"], json!(10.0));
            assert_eq!(row.fields["max_val"], json!(30.0));
        }
    }

    #[test]
    fn test_sum_with_offset_frame() {
        // Frame: 1 PRECEDING to 1 FOLLOWING (sliding window of 3)
        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
            make_row(vec![("val", json!(40))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::Sum,
            args: vec!["val".to_string()],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![WindowOrderBy {
                    field: "val".to_string(),
                    direction: SortDirection::Asc,
                }],
                frame: FrameDefinition {
                    start: FrameBound::Offset(1),
                    end: FrameBound::Offset(1),
                },
            },
            output_field: "sliding_sum".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // Row 0: 10+20 = 30
        // Row 1: 10+20+30 = 60
        // Row 2: 20+30+40 = 90
        // Row 3: 30+40 = 70
        assert_eq!(result[0].fields["sliding_sum"], json!(30.0));
        assert_eq!(result[1].fields["sliding_sum"], json!(60.0));
        assert_eq!(result[2].fields["sliding_sum"], json!(90.0));
        assert_eq!(result[3].fields["sliding_sum"], json!(70.0));
    }

    // -- Navigation tests ---------------------------------------------------

    #[test]
    fn test_lag() {
        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::Lag,
            args: vec!["val".to_string()],
            spec: default_spec(vec![], vec![("val", SortDirection::Asc)]),
            output_field: "prev_val".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        assert_eq!(result[0].fields["prev_val"], JsonValue::Null);
        assert_eq!(result[1].fields["prev_val"], json!(10));
        assert_eq!(result[2].fields["prev_val"], json!(20));
    }

    #[test]
    fn test_lag_with_offset_and_default() {
        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::Lag,
            args: vec!["val".to_string(), "2".to_string(), "0".to_string()],
            spec: default_spec(vec![], vec![("val", SortDirection::Asc)]),
            output_field: "prev2".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // LAG(val, 2, 0): row 0 -> default 0, row 1 -> default 0, row 2 -> 10
        assert_eq!(result[0].fields["prev2"], json!(0.0));
        assert_eq!(result[1].fields["prev2"], json!(0.0));
        assert_eq!(result[2].fields["prev2"], json!(10));
    }

    #[test]
    fn test_lead() {
        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::Lead,
            args: vec!["val".to_string()],
            spec: default_spec(vec![], vec![("val", SortDirection::Asc)]),
            output_field: "next_val".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        assert_eq!(result[0].fields["next_val"], json!(20));
        assert_eq!(result[1].fields["next_val"], json!(30));
        assert_eq!(result[2].fields["next_val"], JsonValue::Null);
    }

    #[test]
    fn test_first_value() {
        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::FirstValue,
            args: vec!["val".to_string()],
            spec: default_spec(vec![], vec![("val", SortDirection::Asc)]),
            output_field: "first".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // Default frame (UNBOUNDED PRECEDING to CURRENT ROW): first_value is always 10
        assert_eq!(result[0].fields["first"], json!(10));
        assert_eq!(result[1].fields["first"], json!(10));
        assert_eq!(result[2].fields["first"], json!(10));
    }

    #[test]
    fn test_last_value() {
        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let call = WindowFunctionCall {
            function: WindowFunction::LastValue,
            args: vec!["val".to_string()],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![WindowOrderBy {
                    field: "val".to_string(),
                    direction: SortDirection::Asc,
                }],
                frame: FrameDefinition {
                    start: FrameBound::Unbounded,
                    end: FrameBound::Unbounded,
                },
            },
            output_field: "last".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        // Full frame: last_value is always 30
        assert_eq!(result[0].fields["last"], json!(30));
        assert_eq!(result[1].fields["last"], json!(30));
        assert_eq!(result[2].fields["last"], json!(30));
    }

    // -- Edge case tests ----------------------------------------------------

    #[test]
    fn test_empty_rows() {
        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(vec![], &[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_single_row() {
        let rows = vec![make_row(vec![("val", json!(42))])];

        let call = WindowFunctionCall {
            function: WindowFunction::RowNumber,
            args: vec![],
            spec: default_spec(vec![], vec![]),
            output_field: "rn".to_string(),
        };

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &[call]).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fields["rn"], json!(1));
    }

    #[test]
    fn test_multiple_window_calls() {
        let rows = vec![
            make_row(vec![("val", json!(10))]),
            make_row(vec![("val", json!(20))]),
            make_row(vec![("val", json!(30))]),
        ];

        let calls = vec![
            WindowFunctionCall {
                function: WindowFunction::RowNumber,
                args: vec![],
                spec: default_spec(vec![], vec![("val", SortDirection::Asc)]),
                output_field: "rn".to_string(),
            },
            WindowFunctionCall {
                function: WindowFunction::Sum,
                args: vec!["val".to_string()],
                spec: default_spec(vec![], vec![("val", SortDirection::Asc)]),
                output_field: "running_sum".to_string(),
            },
        ];

        let executor = WindowExecutor::new();
        let result = executor.execute_window_functions(rows, &calls).unwrap();

        assert_eq!(result[0].fields["rn"], json!(1));
        assert_eq!(result[0].fields["running_sum"], json!(10.0));
        assert_eq!(result[2].fields["rn"], json!(3));
        assert_eq!(result[2].fields["running_sum"], json!(60.0));
    }

    #[test]
    fn test_window_function_from_name() {
        assert_eq!(
            WindowFunction::from_name("row_number").unwrap(),
            WindowFunction::RowNumber
        );
        assert_eq!(
            WindowFunction::from_name("RANK").unwrap(),
            WindowFunction::Rank
        );
        assert_eq!(
            WindowFunction::from_name("Dense_Rank").unwrap(),
            WindowFunction::DenseRank
        );
        assert_eq!(
            WindowFunction::from_name("sum").unwrap(),
            WindowFunction::Sum
        );
        assert_eq!(
            WindowFunction::from_name("AVG").unwrap(),
            WindowFunction::Avg
        );
        assert_eq!(
            WindowFunction::from_name("lag").unwrap(),
            WindowFunction::Lag
        );
        assert_eq!(
            WindowFunction::from_name("LEAD").unwrap(),
            WindowFunction::Lead
        );
        assert_eq!(
            WindowFunction::from_name("FIRST_VALUE").unwrap(),
            WindowFunction::FirstValue
        );
        assert_eq!(
            WindowFunction::from_name("LAST_VALUE").unwrap(),
            WindowFunction::LastValue
        );
        assert!(WindowFunction::from_name("UNKNOWN").is_err());
    }

    #[test]
    fn test_window_function_classification() {
        assert!(WindowFunction::RowNumber.is_ranking());
        assert!(WindowFunction::Rank.is_ranking());
        assert!(WindowFunction::DenseRank.is_ranking());
        assert!(!WindowFunction::Sum.is_ranking());

        assert!(WindowFunction::Lag.is_navigation());
        assert!(WindowFunction::Lead.is_navigation());
        assert!(WindowFunction::FirstValue.is_navigation());
        assert!(WindowFunction::LastValue.is_navigation());
        assert!(!WindowFunction::Count.is_navigation());
    }
}

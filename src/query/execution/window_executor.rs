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
    /// N PRECEDING / N FOLLOWING. Interpreted per the frame's [`FrameUnit`]:
    /// a physical row count for `Rows`, a logical value offset on the ORDER BY
    /// key for `Range`, and a peer-group count for `Groups`.
    Offset(u64),
}

/// How a window frame's bounds are interpreted.
///
/// This mirrors the SQL `ROWS | RANGE | GROUPS` frame units and the AST
/// `WindowFrameUnit`. It is threaded through so a `RANGE`/`GROUPS` frame that
/// parses and lowers correctly also *executes* as declared instead of silently
/// collapsing to `ROWS` (which produces wrong results in the presence of ties).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameUnit {
    /// Bounds count physical rows (ROWS).
    #[default]
    Rows,
    /// Bounds are logical value offsets on the single ORDER BY key (RANGE).
    Range,
    /// Bounds count peer groups — maximal runs of rows equal on ORDER BY (GROUPS).
    Groups,
}

/// Window frame definition.
#[derive(Debug, Clone)]
pub struct FrameDefinition {
    /// How the bounds are interpreted (ROWS / RANGE / GROUPS).
    pub unit: FrameUnit,
    pub start: FrameBound,
    pub end: FrameBound,
}

impl Default for FrameDefinition {
    /// Default frame: ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW.
    fn default() -> Self {
        Self {
            unit: FrameUnit::Rows,
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
                    .get(f)
                    .map_or_else(|| "null".to_string(), |v| v.to_string())
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
                    .as_str()
                    .map_or_else(|| va.to_string(), |s| s.to_string());
                let sb = vb
                    .as_str()
                    .map_or_else(|| vb.to_string(), |s| s.to_string());
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
            let (start, end) = Self::resolve_frame(
                &call.spec.frame,
                partition,
                &call.spec.order_by,
                current_pos,
            )?;

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

    /// Resolve a `FrameDefinition` into concrete inclusive start/end indices
    /// within the (already sorted) `partition`, honouring the frame unit.
    ///
    /// The three units differ precisely on rows that are *peers* (equal on the
    /// ORDER BY key), which is why `ROWS` alone is a correctness bug:
    ///
    /// - `ROWS` — bounds count physical rows.
    /// - `RANGE` — bounds are value offsets on the single ORDER BY key; the frame
    ///   spans every row whose key is within the value window.
    /// - `GROUPS` — bounds count peer groups; the frame spans whole groups.
    fn resolve_frame(
        frame: &FrameDefinition,
        partition: &[(usize, QueryRow)],
        order_by: &[WindowOrderBy],
        current: usize,
    ) -> Result<(usize, usize)> {
        let len = partition.len();
        match frame.unit {
            FrameUnit::Rows => Ok(Self::resolve_rows_frame(frame, current, len)),
            FrameUnit::Groups => Self::resolve_groups_frame(frame, partition, order_by, current),
            FrameUnit::Range => Self::resolve_range_frame(frame, partition, order_by, current),
        }
    }

    /// ROWS: bounds are physical row offsets from the current row.
    fn resolve_rows_frame(frame: &FrameDefinition, current: usize, len: usize) -> (usize, usize) {
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

    /// GROUPS: bounds count peer groups (maximal runs of rows equal on ORDER BY).
    /// The partition is pre-sorted in ORDER BY direction, so "preceding" groups
    /// are simply lower-indexed groups regardless of ASC/DESC.
    fn resolve_groups_frame(
        frame: &FrameDefinition,
        partition: &[(usize, QueryRow)],
        order_by: &[WindowOrderBy],
        current: usize,
    ) -> Result<(usize, usize)> {
        let len = partition.len();
        if len == 0 {
            return Ok((0, 0));
        }
        // Assign each row a peer-group id and record each group's first row index.
        let mut group_id = vec![0usize; len];
        let mut group_starts: Vec<usize> = vec![0];
        for i in 1..len {
            if Self::rows_equal_on_order_by(&partition[i - 1].1, &partition[i].1, order_by) {
                group_id[i] = group_id[i - 1];
            } else {
                group_id[i] = group_id[i - 1] + 1;
                group_starts.push(i);
            }
        }
        let n_groups = group_starts.len();
        // Last row index of group `g` (groups are contiguous).
        let group_last = |g: usize| -> usize {
            if g + 1 < n_groups {
                group_starts[g + 1] - 1
            } else {
                len - 1
            }
        };
        let cur = group_id[current];
        let start_g = match &frame.start {
            FrameBound::Unbounded => 0,
            FrameBound::CurrentRow => cur,
            FrameBound::Offset(n) => cur.saturating_sub(*n as usize),
        };
        let end_g = match &frame.end {
            FrameBound::Unbounded => n_groups - 1,
            FrameBound::CurrentRow => cur,
            FrameBound::Offset(n) => (cur + *n as usize).min(n_groups - 1),
        };
        Ok((group_starts[start_g], group_last(end_g)))
    }

    /// RANGE: bounds are logical value offsets on the ORDER BY key. With an
    /// explicit numeric offset the frame spans every row whose key is within the
    /// value window; with only UNBOUNDED/CURRENT ROW bounds it spans whole peer
    /// groups (which needs no numeric key).
    fn resolve_range_frame(
        frame: &FrameDefinition,
        partition: &[(usize, QueryRow)],
        order_by: &[WindowOrderBy],
        current: usize,
    ) -> Result<(usize, usize)> {
        let len = partition.len();
        if len == 0 {
            return Ok((0, 0));
        }
        // No ORDER BY: the whole partition is a single peer group.
        if order_by.is_empty() {
            return Ok((0, len - 1));
        }

        let has_offset = matches!(frame.start, FrameBound::Offset(_))
            || matches!(frame.end, FrameBound::Offset(_));

        if !has_offset {
            // Peer-based RANGE (only UNBOUNDED / CURRENT ROW bounds): CURRENT ROW
            // means the current peer group, not the current physical row.
            let mut group_start = current;
            while group_start > 0
                && Self::rows_equal_on_order_by(
                    &partition[group_start - 1].1,
                    &partition[current].1,
                    order_by,
                )
            {
                group_start -= 1;
            }
            let mut group_end = current;
            while group_end + 1 < len
                && Self::rows_equal_on_order_by(
                    &partition[group_end + 1].1,
                    &partition[current].1,
                    order_by,
                )
            {
                group_end += 1;
            }
            // Only UNBOUNDED / CURRENT ROW reach here (Offset ⇒ the value path
            // below). Map defensively rather than assuming, so an unexpected
            // bound fails loud instead of silently mis-framing.
            let peer_bound = |bound: &FrameBound, unbounded_idx: usize, peer_idx: usize| match bound
            {
                FrameBound::Unbounded => Ok(unbounded_idx),
                FrameBound::CurrentRow => Ok(peer_idx),
                FrameBound::Offset(_) => Err(anyhow!(
                    "internal error: value-offset RANGE bound reached the peer-based path"
                )),
            };
            let start = peer_bound(&frame.start, 0, group_start)?;
            let end = peer_bound(&frame.end, len - 1, group_end)?;
            return Ok((start, end));
        }

        // Value-based RANGE requires exactly one numeric ORDER BY key.
        if order_by.len() != 1 {
            return Err(anyhow!(
                "RANGE frame with a value offset requires exactly one ORDER BY column (got {})",
                order_by.len()
            ));
        }
        let ob = &order_by[0];
        let asc = matches!(ob.direction, SortDirection::Asc);
        let value_at = |i: usize| -> Result<f64> {
            partition
                .get(i)
                .and_then(|(_, row)| row.fields.get(&ob.field))
                .and_then(|v| v.as_f64())
                .ok_or_else(|| {
                    anyhow!(
                        "RANGE frame with a value offset requires a numeric ORDER BY key; \
                         column '{}' is null or non-numeric",
                        ob.field
                    )
                })
        };
        let cur_val = value_at(current)?;

        // Inclusive value window [lo, hi]. For ASC the start bound is the low
        // side and the end bound the high side; for DESC the roles flip because
        // preceding rows carry larger keys.
        let (lo, hi) = if asc {
            let lo = match &frame.start {
                FrameBound::Unbounded => f64::NEG_INFINITY,
                FrameBound::CurrentRow => cur_val,
                FrameBound::Offset(n) => cur_val - *n as f64,
            };
            let hi = match &frame.end {
                FrameBound::Unbounded => f64::INFINITY,
                FrameBound::CurrentRow => cur_val,
                FrameBound::Offset(n) => cur_val + *n as f64,
            };
            (lo, hi)
        } else {
            let hi = match &frame.start {
                FrameBound::Unbounded => f64::INFINITY,
                FrameBound::CurrentRow => cur_val,
                FrameBound::Offset(n) => cur_val + *n as f64,
            };
            let lo = match &frame.end {
                FrameBound::Unbounded => f64::NEG_INFINITY,
                FrameBound::CurrentRow => cur_val,
                FrameBound::Offset(n) => cur_val - *n as f64,
            };
            (lo, hi)
        };

        // The partition is sorted on the key, so the in-window rows are
        // contiguous; scan for the first and last that fall inside [lo, hi].
        let mut start = None;
        let mut end = current;
        for i in 0..len {
            let v = value_at(i)?;
            if v >= lo && v <= hi {
                if start.is_none() {
                    start = Some(i);
                }
                end = i;
            }
        }
        // The current row always satisfies its own window, so `start` is set.
        Ok((start.unwrap_or(current), end))
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

        let default_val: JsonValue = call.args.get(2).map_or(JsonValue::Null, |s| {
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
            let (start, end) = Self::resolve_frame(
                &call.spec.frame,
                partition,
                &call.spec.order_by,
                current_pos,
            )?;
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
                    unit: FrameUnit::Rows,
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
                    unit: FrameUnit::Rows,
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
                    unit: FrameUnit::Rows,
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
                    unit: FrameUnit::Rows,
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
                    unit: FrameUnit::Rows,
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

    // -- Frame unit tests (ROWS vs RANGE vs GROUPS) -------------------------
    //
    // Ties on the ORDER BY key are exactly what distinguishes the three frame
    // units, so every case here is built on an ORDERED dataset WITH TIES and
    // the expected sums are hand-verified. These guard the correctness bug
    // where a RANGE/GROUPS frame silently executed as ROWS.

    fn frame(unit: FrameUnit, start: FrameBound, end: FrameBound) -> FrameDefinition {
        FrameDefinition { unit, start, end }
    }

    /// Run `SUM(x) OVER (ORDER BY x <dir> <frame>)` over `x` (already in the
    /// given sort direction) and return the per-row result in input order.
    fn run_sum(x: &[f64], dir: SortDirection, frame: FrameDefinition) -> Result<Vec<f64>> {
        let rows: Vec<QueryRow> = x.iter().map(|&v| make_row(vec![("x", json!(v))])).collect();
        let call = WindowFunctionCall {
            function: WindowFunction::Sum,
            args: vec!["x".to_string()],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![WindowOrderBy {
                    field: "x".to_string(),
                    direction: dir,
                }],
                frame,
            },
            output_field: "s".to_string(),
        };
        let out = WindowExecutor::new().execute_window_functions(rows, &[call])?;
        Ok(out
            .iter()
            .map(|r| {
                r.fields
                    .get("s")
                    .and_then(|v| v.as_f64())
                    .expect("sum present")
            })
            .collect())
    }

    #[test]
    fn frame_units_diverge_with_value_offset_and_ties() {
        // x = [1, 3, 3, 4]; SUM(x) OVER (ORDER BY x <unit> BETWEEN 1 PRECEDING
        // AND CURRENT ROW). Hand-verified, all three distinct at the tie rows:
        //   ROWS   — physical rows:  [1], [1,3], [3,3], [3,4]           = 1, 4, 6, 7
        //   RANGE  — value in [x-1,x]: [1], [3,3], [3,3], [3,3,4]       = 1, 6, 6, 10
        //   GROUPS — whole peer groups: [1], [1,3,3], [1,3,3], [3,3,4]  = 1, 7, 7, 10
        let x = [1.0, 3.0, 3.0, 4.0];
        let asc = SortDirection::Asc;

        let rows = run_sum(
            &x,
            asc.clone(),
            frame(
                FrameUnit::Rows,
                FrameBound::Offset(1),
                FrameBound::CurrentRow,
            ),
        )
        .unwrap();
        assert_eq!(rows, vec![1.0, 4.0, 6.0, 7.0], "ROWS");

        let range = run_sum(
            &x,
            asc.clone(),
            frame(
                FrameUnit::Range,
                FrameBound::Offset(1),
                FrameBound::CurrentRow,
            ),
        )
        .unwrap();
        assert_eq!(range, vec![1.0, 6.0, 6.0, 10.0], "RANGE");

        let groups = run_sum(
            &x,
            asc,
            frame(
                FrameUnit::Groups,
                FrameBound::Offset(1),
                FrameBound::CurrentRow,
            ),
        )
        .unwrap();
        assert_eq!(groups, vec![1.0, 7.0, 7.0, 10.0], "GROUPS");

        // The whole point of the fix: the three are NOT equal.
        assert_ne!(rows, range);
        assert_ne!(range, groups);
    }

    #[test]
    fn range_and_groups_current_row_span_peers_not_the_single_row() {
        // x = [1, 2, 2, 3]; UNBOUNDED PRECEDING AND CURRENT ROW.
        //   ROWS   running total per row:      1, 3, 5, 8
        //   RANGE  through current peer group: 1, 5, 5, 8
        //   GROUPS through current peer group: 1, 5, 5, 8
        let x = [1.0, 2.0, 2.0, 3.0];
        let asc = SortDirection::Asc;

        let rows = run_sum(
            &x,
            asc.clone(),
            frame(
                FrameUnit::Rows,
                FrameBound::Unbounded,
                FrameBound::CurrentRow,
            ),
        )
        .unwrap();
        assert_eq!(rows, vec![1.0, 3.0, 5.0, 8.0], "ROWS running total");

        let range = run_sum(
            &x,
            asc.clone(),
            frame(
                FrameUnit::Range,
                FrameBound::Unbounded,
                FrameBound::CurrentRow,
            ),
        )
        .unwrap();
        assert_eq!(range, vec![1.0, 5.0, 5.0, 8.0], "RANGE peers");

        let groups = run_sum(
            &x,
            asc,
            frame(
                FrameUnit::Groups,
                FrameBound::Unbounded,
                FrameBound::CurrentRow,
            ),
        )
        .unwrap();
        assert_eq!(groups, vec![1.0, 5.0, 5.0, 8.0], "GROUPS peers");
    }

    #[test]
    fn range_offset_respects_desc_ordering() {
        // ORDER BY x DESC, sorted input [4, 3, 3, 1].
        // RANGE BETWEEN 1 PRECEDING AND CURRENT ROW: preceding = higher value,
        // so the window is [x, x+1].
        //   x=4 -> [4]        = 4
        //   x=3 -> [4,3,3]    = 10
        //   x=3 -> [4,3,3]    = 10
        //   x=1 -> [1]        = 1
        let x = [4.0, 3.0, 3.0, 1.0];
        let range = run_sum(
            &x,
            SortDirection::Desc,
            frame(
                FrameUnit::Range,
                FrameBound::Offset(1),
                FrameBound::CurrentRow,
            ),
        )
        .unwrap();
        assert_eq!(range, vec![4.0, 10.0, 10.0, 1.0]);
    }

    #[test]
    fn groups_following_spans_following_peer_groups() {
        // x = [1, 3, 3, 4]; GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING.
        //   g0={1}, g1={3,3}, g2={4}
        //   pos0 -> g0..g1 = [1,3,3] = 7
        //   pos1 -> g1..g2 = [3,3,4] = 10
        //   pos2 -> g1..g2 = [3,3,4] = 10
        //   pos3 -> g2..g2 = [4]     = 4
        let x = [1.0, 3.0, 3.0, 4.0];
        let groups = run_sum(
            &x,
            SortDirection::Asc,
            frame(
                FrameUnit::Groups,
                FrameBound::CurrentRow,
                FrameBound::Offset(1),
            ),
        )
        .unwrap();
        assert_eq!(groups, vec![7.0, 10.0, 10.0, 4.0]);
    }

    #[test]
    fn range_value_offset_fails_loud_on_non_numeric_key() {
        // A value-based RANGE offset needs a numeric ORDER BY key; a text key
        // must fail loudly rather than silently returning a wrong number.
        let rows = vec![
            make_row(vec![("x", json!("a"))]),
            make_row(vec![("x", json!("b"))]),
        ];
        let call = WindowFunctionCall {
            function: WindowFunction::Sum,
            args: vec!["x".to_string()],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![WindowOrderBy {
                    field: "x".to_string(),
                    direction: SortDirection::Asc,
                }],
                frame: frame(
                    FrameUnit::Range,
                    FrameBound::Offset(1),
                    FrameBound::CurrentRow,
                ),
            },
            output_field: "s".to_string(),
        };
        let err = WindowExecutor::new()
            .execute_window_functions(rows, &[call])
            .unwrap_err();
        assert!(
            err.to_string().contains("numeric ORDER BY"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn range_value_offset_fails_loud_on_multiple_order_by_keys() {
        // SQL forbids a value-offset RANGE with more than one ORDER BY column.
        let rows = vec![make_row(vec![("x", json!(1.0)), ("y", json!(2.0))])];
        let call = WindowFunctionCall {
            function: WindowFunction::Sum,
            args: vec!["x".to_string()],
            spec: WindowSpec {
                partition_by: vec![],
                order_by: vec![
                    WindowOrderBy {
                        field: "x".to_string(),
                        direction: SortDirection::Asc,
                    },
                    WindowOrderBy {
                        field: "y".to_string(),
                        direction: SortDirection::Asc,
                    },
                ],
                frame: frame(
                    FrameUnit::Range,
                    FrameBound::Offset(1),
                    FrameBound::CurrentRow,
                ),
            },
            output_field: "s".to_string(),
        };
        let err = WindowExecutor::new()
            .execute_window_functions(rows, &[call])
            .unwrap_err();
        assert!(
            err.to_string().contains("exactly one ORDER BY"),
            "unexpected error: {err}"
        );
    }
}

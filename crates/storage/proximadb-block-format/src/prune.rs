// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Block-level predicate pruning — turn a [`FilterExpression`] into a decision
//! about whether a PAX block can be skipped without decoding any stripe.
//!
//! This is the seam that makes the reader's zone-map methods
//! ([`PaxBlockReader::column_may_contain_i64`] etc.) actually do work: it walks
//! the And/Or/Not filter tree, maps each leaf's field name to a canonical PAX
//! column id, and consults that column's min/max / bloom statistics.
//!
//! Soundness contract: pruning is **conservative**. [`PruneResult::Skip`] is
//! returned only when the block provably contains no matching row; anything
//! uncertain (an unknown column, an operator the zone map cannot bound, or a
//! negation) returns [`PruneResult::MayMatch`]. A correct pruner may yield false
//! positives (a block kept that turns out empty) but never a false negative (a
//! block skipped that held a match) — verified by `prune_never_false_negative`.

use proximadb_filter_expression::{ComparisonOperator, FilterExpression};
use serde_json::Value;

use crate::reader::PaxBlockReader;

/// Outcome of evaluating a filter against a block's statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneResult {
    /// The block provably contains no matching row — safe to skip entirely.
    Skip,
    /// The block may contain matching rows — must be read.
    MayMatch,
}

impl PruneResult {
    /// True when the block must be read (cannot be skipped).
    pub fn keep(self) -> bool {
        matches!(self, PruneResult::MayMatch)
    }
}

/// Maps a filter field name to a canonical PAX column id, or `None` if the field
/// is not a stored column (then the predicate cannot prune). Built by the engine
/// from the collection schema (canonical ids in [`crate::record::col_id`] +
/// user columns from `USER_BASE`).
pub type FieldToColumn<'a> = dyn Fn(&str) -> Option<i32> + 'a;

// data_type_id conventions stamped by the writer.
const DT_F32_VECTOR: u8 = 0x01;
const DT_I64: u8 = 0x03;
const DT_F64: u8 = 0x07;
const DT_BYTES_OR_STR: u8 = 0xff;

/// Evaluate a filter against `reader`'s block statistics.
///
/// `field_to_col` resolves leaf field names to column ids. See [`PruneResult`]
/// for the soundness contract.
pub fn evaluate_block(
    reader: &PaxBlockReader<'_>,
    filter: &FilterExpression,
    field_to_col: &FieldToColumn<'_>,
) -> PruneResult {
    match filter {
        FilterExpression::And(children) => {
            // AND prunes if ANY conjunct prunes the block.
            for c in children {
                if evaluate_block(reader, c, field_to_col) == PruneResult::Skip {
                    return PruneResult::Skip;
                }
            }
            PruneResult::MayMatch
        }
        FilterExpression::Or(children) => {
            // OR prunes only if EVERY disjunct prunes the block.
            if children.is_empty() {
                return PruneResult::MayMatch;
            }
            if children
                .iter()
                .all(|c| evaluate_block(reader, c, field_to_col) == PruneResult::Skip)
            {
                PruneResult::Skip
            } else {
                PruneResult::MayMatch
            }
        }
        // Negation cannot be bounded by a min/max zone map: never prune.
        FilterExpression::Not(_) => PruneResult::MayMatch,
        FilterExpression::Comparison {
            field,
            operator,
            value,
        } => evaluate_leaf(reader, field, operator, value, field_to_col),
    }
}

/// Return the indices of row groups that may contain matching rows for `filter`.
///
/// Uses the block's row-group sub-index (`reader.row_groups()`); a block with no
/// sub-index conservatively returns all row groups. Same soundness contract as
/// [`evaluate_block`]: a row group is dropped only when provably empty.
pub fn evaluate_row_groups(
    reader: &PaxBlockReader<'_>,
    filter: &FilterExpression,
    field_to_col: &FieldToColumn<'_>,
) -> Vec<usize> {
    let rg_block = reader.row_groups();
    let n = rg_block.n_row_groups as usize;
    if rg_block.is_empty() || n == 0 {
        // No sub-index: fall back to "all groups" (block-level pruning still applies).
        let count = crate::rowgroup::RowGroupBlock::group_count(reader.row_count()) as usize;
        return (0..count.max(1)).collect();
    }
    (0..n)
        .filter(|&rg| {
            eval_rg(reader, filter, rg as u32, field_to_col) == PruneResult::MayMatch
        })
        .collect()
}

fn eval_rg(
    reader: &PaxBlockReader<'_>,
    filter: &FilterExpression,
    rg: u32,
    field_to_col: &FieldToColumn<'_>,
) -> PruneResult {
    match filter {
        FilterExpression::And(children) => {
            for c in children {
                if eval_rg(reader, c, rg, field_to_col) == PruneResult::Skip {
                    return PruneResult::Skip;
                }
            }
            PruneResult::MayMatch
        }
        FilterExpression::Or(children) => {
            if children.is_empty() {
                return PruneResult::MayMatch;
            }
            if children
                .iter()
                .all(|c| eval_rg(reader, c, rg, field_to_col) == PruneResult::Skip)
            {
                PruneResult::Skip
            } else {
                PruneResult::MayMatch
            }
        }
        FilterExpression::Not(_) => PruneResult::MayMatch,
        FilterExpression::Comparison {
            field,
            operator,
            value,
        } => {
            let Some(col) = field_to_col(field) else {
                return PruneResult::MayMatch;
            };
            let Some(entry) = reader.row_groups().get(col, rg) else {
                return PruneResult::MayMatch; // no rg stats for this column
            };
            match entry.data_type_id {
                DT_I64 => prune_i64_bounds(entry, operator, value),
                DT_F64 => prune_f64_bounds(entry, operator, value),
                _ => PruneResult::MayMatch,
            }
        }
    }
}

fn prune_i64_bounds(
    entry: &crate::rowgroup::RowGroupEntry,
    op: &ComparisonOperator,
    value: &Value,
) -> PruneResult {
    match op {
        ComparisonOperator::Equals => match as_i64(value) {
            Some(v) => keep_if(entry.i64_range_overlaps(v, v)),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::GreaterThan | ComparisonOperator::GreaterThanOrEqual => {
            match as_i64(value) {
                Some(v) => keep_if(entry.i64_range_overlaps(v, i64::MAX)),
                None => PruneResult::MayMatch,
            }
        }
        ComparisonOperator::LessThan | ComparisonOperator::LessThanOrEqual => match as_i64(value) {
            Some(v) => keep_if(entry.i64_range_overlaps(i64::MIN, v)),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::Between => match between_bounds(value) {
            Some((lo, hi)) => keep_if(entry.i64_range_overlaps(lo as i64, hi.ceil() as i64)),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::In => match value.as_array() {
            Some(arr) => {
                let had_ints = arr.iter().any(|v| as_i64(v).is_some());
                let any = arr
                    .iter()
                    .filter_map(as_i64)
                    .any(|v| entry.i64_range_overlaps(v, v));
                if had_ints { keep_if(any) } else { PruneResult::MayMatch }
            }
            None => PruneResult::MayMatch,
        },
        _ => PruneResult::MayMatch,
    }
}

fn prune_f64_bounds(
    entry: &crate::rowgroup::RowGroupEntry,
    op: &ComparisonOperator,
    value: &Value,
) -> PruneResult {
    match op {
        ComparisonOperator::Equals => match as_f64(value) {
            Some(v) => keep_if(entry.f64_range_overlaps(v, v)),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::GreaterThan | ComparisonOperator::GreaterThanOrEqual => {
            match as_f64(value) {
                Some(v) => keep_if(entry.f64_range_overlaps(v, f64::INFINITY)),
                None => PruneResult::MayMatch,
            }
        }
        ComparisonOperator::LessThan | ComparisonOperator::LessThanOrEqual => match as_f64(value) {
            Some(v) => keep_if(entry.f64_range_overlaps(f64::NEG_INFINITY, v)),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::Between => match between_bounds(value) {
            Some((lo, hi)) => keep_if(entry.f64_range_overlaps(lo, hi)),
            None => PruneResult::MayMatch,
        },
        _ => PruneResult::MayMatch,
    }
}

fn evaluate_leaf(
    reader: &PaxBlockReader<'_>,
    field: &str,
    op: &ComparisonOperator,
    value: &Value,
    field_to_col: &FieldToColumn<'_>,
) -> PruneResult {
    let Some(col) = field_to_col(field) else {
        return PruneResult::MayMatch; // unknown column → cannot prune
    };
    let Some(type_id) = reader
        .column_metas()
        .iter()
        .find(|m| m.column_id == col)
        .map(|m| m.data_type_id)
    else {
        return PruneResult::MayMatch; // column not in this block
    };

    match type_id {
        DT_I64 => prune_i64(reader, col, op, value),
        DT_F64 => prune_f64(reader, col, op, value),
        DT_BYTES_OR_STR => prune_str(reader, col, op, value),
        // Vectors and anything else: not a scalar zone-map-prunable column.
        DT_F32_VECTOR | _ => PruneResult::MayMatch,
    }
}

fn keep_if(b: bool) -> PruneResult {
    if b {
        PruneResult::MayMatch
    } else {
        PruneResult::Skip
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}

fn prune_i64(
    reader: &PaxBlockReader<'_>,
    col: i32,
    op: &ComparisonOperator,
    value: &Value,
) -> PruneResult {
    match op {
        ComparisonOperator::Equals => match as_i64(value) {
            Some(v) => keep_if(reader.column_may_contain_i64(col, v)),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::GreaterThan | ComparisonOperator::GreaterThanOrEqual => {
            match as_i64(value) {
                Some(v) => keep_if(reader.column_range_overlaps_i64(col, v, i64::MAX)),
                None => PruneResult::MayMatch,
            }
        }
        ComparisonOperator::LessThan | ComparisonOperator::LessThanOrEqual => match as_i64(value) {
            Some(v) => keep_if(reader.column_range_overlaps_i64(col, i64::MIN, v)),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::Between => match between_bounds(value) {
            Some((lo, hi)) => keep_if(reader.column_range_overlaps_i64(
                col,
                lo as i64,
                hi.ceil() as i64,
            )),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::In => match value.as_array() {
            // Keep if ANY listed value may be present; skip only if none can.
            Some(arr) => {
                let any = arr.iter().filter_map(as_i64).any(|v| {
                    reader.column_may_contain_i64(col, v)
                });
                let had_ints = arr.iter().any(|v| as_i64(v).is_some());
                if had_ints { keep_if(any) } else { PruneResult::MayMatch }
            }
            None => PruneResult::MayMatch,
        },
        // NotEquals / NotIn / IsNull / IsNotNull / string ops: not prunable here.
        _ => PruneResult::MayMatch,
    }
}

fn prune_f64(
    reader: &PaxBlockReader<'_>,
    col: i32,
    op: &ComparisonOperator,
    value: &Value,
) -> PruneResult {
    match op {
        ComparisonOperator::Equals => match as_f64(value) {
            Some(v) => keep_if(reader.column_range_overlaps_f64(col, v, v)),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::GreaterThan | ComparisonOperator::GreaterThanOrEqual => {
            match as_f64(value) {
                Some(v) => keep_if(reader.column_range_overlaps_f64(col, v, f64::INFINITY)),
                None => PruneResult::MayMatch,
            }
        }
        ComparisonOperator::LessThan | ComparisonOperator::LessThanOrEqual => match as_f64(value) {
            Some(v) => keep_if(reader.column_range_overlaps_f64(col, f64::NEG_INFINITY, v)),
            None => PruneResult::MayMatch,
        },
        ComparisonOperator::Between => match between_bounds(value) {
            Some((lo, hi)) => keep_if(reader.column_range_overlaps_f64(col, lo, hi)),
            None => PruneResult::MayMatch,
        },
        _ => PruneResult::MayMatch,
    }
}

fn prune_str(
    reader: &PaxBlockReader<'_>,
    col: i32,
    op: &ComparisonOperator,
    value: &Value,
) -> PruneResult {
    match op {
        // Only exact equality can use the string hash bounds + bloom filter.
        ComparisonOperator::Equals => match value.as_str() {
            Some(s) => keep_if(reader.column_may_contain_str(col, s)),
            None => PruneResult::MayMatch,
        },
        _ => PruneResult::MayMatch,
    }
}

/// Extract `[lo, hi]` from a BETWEEN value (`[a, b]` array).
fn between_bounds(value: &Value) -> Option<(f64, f64)> {
    let arr = value.as_array()?;
    if arr.len() != 2 {
        return None;
    }
    let lo = as_f64(&arr[0])?;
    let hi = as_f64(&arr[1])?;
    Some((lo.min(hi), lo.max(hi)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{BlockCompression, BlockMode};
    use crate::record::col_id;
    use crate::writer::PaxBlockWriter;
    use proximadb_records::ProximaRecord;
    use serde_json::json;

    fn rec(oid: &str, ts: i64) -> ProximaRecord {
        ProximaRecord {
            oid: oid.into(),
            tenant_id: "t".into(),
            created_at_ns: ts,
            updated_at_ns: ts,
            ..Default::default()
        }
    }

    // created_at is column CREATED_AT (i64); map "created_at" to it.
    fn field_map(field: &str) -> Option<i32> {
        match field {
            "created_at" => Some(col_id::CREATED_AT),
            "tenant_id" => Some(col_id::TENANT_ID),
            _ => None,
        }
    }

    fn block_with_ts(tss: &[i64]) -> Vec<u8> {
        let mut w = PaxBlockWriter::new(BlockMode::Pax, BlockCompression::None, "c", 0, 0);
        for (i, &ts) in tss.iter().enumerate() {
            w.add_record(&rec(&format!("r{i}"), ts)).unwrap();
        }
        w.flush().unwrap()
    }

    #[test]
    fn prune_and_or_not_correctness() {
        // Block holds created_at in [1000, 2000].
        let block = block_with_ts(&[1000, 1500, 2000]);
        let reader = PaxBlockReader::open(&block).unwrap();

        // created_at > 5000 → provably empty → Skip.
        let gt = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::GreaterThan,
            value: json!(5000),
        };
        assert_eq!(evaluate_block(&reader, &gt, &field_map), PruneResult::Skip);

        // created_at > 1200 → overlaps → MayMatch.
        let gt2 = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::GreaterThan,
            value: json!(1200),
        };
        assert_eq!(evaluate_block(&reader, &gt2, &field_map), PruneResult::MayMatch);

        // AND(empty, overlap) → Skip (any conjunct prunes).
        let and = FilterExpression::And(vec![gt.clone(), gt2.clone()]);
        assert_eq!(evaluate_block(&reader, &and, &field_map), PruneResult::Skip);

        // OR(empty, overlap) → MayMatch (not all disjuncts prune).
        let or = FilterExpression::Or(vec![gt.clone(), gt2.clone()]);
        assert_eq!(evaluate_block(&reader, &or, &field_map), PruneResult::MayMatch);

        // OR(empty, empty) → Skip.
        let gt3 = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::LessThan,
            value: json!(500),
        };
        let or2 = FilterExpression::Or(vec![gt.clone(), gt3]);
        assert_eq!(evaluate_block(&reader, &or2, &field_map), PruneResult::Skip);

        // NOT(empty) → never prune.
        let not = FilterExpression::Not(Box::new(gt));
        assert_eq!(evaluate_block(&reader, &not, &field_map), PruneResult::MayMatch);

        // Unknown column → never prune.
        let unknown = FilterExpression::Comparison {
            field: "nope".into(),
            operator: ComparisonOperator::Equals,
            value: json!(1),
        };
        assert_eq!(
            evaluate_block(&reader, &unknown, &field_map),
            PruneResult::MayMatch
        );
    }

    #[test]
    fn prune_never_false_negative() {
        // For a spread of blocks and equality predicates over created_at, any
        // value actually present must NEVER be pruned (no false negatives).
        let tss = [10i64, 20, 30, 40, 50];
        let block = block_with_ts(&tss);
        let reader = PaxBlockReader::open(&block).unwrap();
        for &present in &tss {
            let f = FilterExpression::Comparison {
                field: "created_at".into(),
                operator: ComparisonOperator::Equals,
                value: json!(present),
            };
            assert_eq!(
                evaluate_block(&reader, &f, &field_map),
                PruneResult::MayMatch,
                "present value {present} was wrongly pruned"
            );
        }
        // A value far outside the [10,50] zone is safely skipped.
        let out = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::Equals,
            value: json!(99999),
        };
        assert_eq!(evaluate_block(&reader, &out, &field_map), PruneResult::Skip);
    }

    #[test]
    fn row_group_subset_selection() {
        // > ROW_GROUP_SIZE rows with monotonically increasing created_at puts
        // low timestamps in rg0 and high timestamps in rg1.
        let rgs = crate::rowgroup::ROW_GROUP_SIZE;
        let n = (rgs + 1000) as usize; // two row groups
        let mut w = PaxBlockWriter::new(BlockMode::Pax, BlockCompression::None, "c", 0, 0);
        for i in 0..n {
            w.add_record(&rec(&format!("r{i}"), 1000 + i as i64)).unwrap();
        }
        let block = w.flush().unwrap();
        let reader = PaxBlockReader::open(&block).unwrap();
        assert_eq!(reader.row_groups().n_row_groups, 2);

        // rg1 covers rows [rgs .. n) → created_at >= 1000 + rgs.
        let hi_threshold = 1000 + rgs as i64 + 500;
        let f_high = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::GreaterThanOrEqual,
            value: json!(hi_threshold),
        };
        // Only rg1 can match.
        assert_eq!(evaluate_row_groups(&reader, &f_high, &field_map), vec![1]);

        // created_at < 2000 → only rg0 can match.
        let f_low = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::LessThan,
            value: json!(2000),
        };
        assert_eq!(evaluate_row_groups(&reader, &f_low, &field_map), vec![0]);

        // No predicate match anywhere → empty.
        let f_none = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::GreaterThan,
            value: json!(10_000_000),
        };
        assert!(evaluate_row_groups(&reader, &f_none, &field_map).is_empty());
    }

    #[test]
    fn prune_between_and_in() {
        let block = block_with_ts(&[1000, 1500, 2000]);
        let reader = PaxBlockReader::open(&block).unwrap();

        let between_out = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::Between,
            value: json!([3000, 4000]),
        };
        assert_eq!(
            evaluate_block(&reader, &between_out, &field_map),
            PruneResult::Skip
        );

        let between_in = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::Between,
            value: json!([1400, 1600]),
        };
        assert_eq!(
            evaluate_block(&reader, &between_in, &field_map),
            PruneResult::MayMatch
        );

        let in_out = FilterExpression::Comparison {
            field: "created_at".into(),
            operator: ComparisonOperator::In,
            value: json!([7000, 8000]),
        };
        assert_eq!(evaluate_block(&reader, &in_out, &field_map), PruneResult::Skip);
    }
}

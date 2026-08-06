// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Plan geometry — a cheap, pre-execution geometric summary of a [`PhysicalPlan`]
//! (TD-EXEC-2 Slice 1, observe-only).
//!
//! A query plan is a geometric object; one `O(nodes)` traversal yields a feature
//! vector — depth, node/leaf counts, fan-out, per-op histogram, blocking-operator
//! count — that later drives *three* resource decisions from one measured cost
//! spine (TD-EXEC-2): stack sizing, engine routing, and parallelism. This module
//! produces the vector; it makes **no decision** and changes **no behavior** — it
//! is the shared measurement input.
//!
//! The traversal is deliberately **iterative** (an explicit work-list, never
//! recursion): a recursive geometry pass would re-introduce the very stack
//! overflow it is used to characterize on the deep plans it is asked to measure
//! (TD-EXEC-2 §Slice-1). It is therefore safe on any plan depth, unlike the
//! recursive lowering/execution it summarizes.

use crate::PhysicalPlan;
use std::collections::BTreeMap;

/// The kind of a physical operator — the stable histogram key for [`PlanGeometry`].
///
/// One variant per [`PhysicalPlan`] variant. Ordered so [`PlanGeometry::op_histogram`]
/// (a `BTreeMap`) has a deterministic iteration order for reporting/serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OpKind {
    Scan,
    Filter,
    Project,
    Join,
    Aggregate,
    Sort,
    Limit,
    Distinct,
    AssertMaxOneRow,
    Union,
    SetOp,
    Values,
}

impl OpKind {
    /// The operator kind of `plan`.
    pub fn of(plan: &PhysicalPlan) -> OpKind {
        match plan {
            PhysicalPlan::Scan { .. } => OpKind::Scan,
            PhysicalPlan::Filter { .. } => OpKind::Filter,
            PhysicalPlan::Project { .. } => OpKind::Project,
            PhysicalPlan::Join { .. } => OpKind::Join,
            PhysicalPlan::Aggregate { .. } => OpKind::Aggregate,
            PhysicalPlan::Sort { .. } => OpKind::Sort,
            PhysicalPlan::Limit { .. } => OpKind::Limit,
            PhysicalPlan::Distinct { .. } => OpKind::Distinct,
            PhysicalPlan::AssertMaxOneRow { .. } => OpKind::AssertMaxOneRow,
            PhysicalPlan::Union { .. } => OpKind::Union,
            PhysicalPlan::SetOp { .. } => OpKind::SetOp,
            PhysicalPlan::Values { .. } => OpKind::Values,
        }
    }

    /// Is this a **pipeline breaker** — an operator that must buffer (potentially
    /// spill) its input before emitting output? These are the memory/spill risk and
    /// the parallelism-gating signal in the geometry vector: hash join build, sort,
    /// and hash aggregation. Streaming operators (Filter/Project/Limit/…) are not.
    pub fn is_blocking(self) -> bool {
        matches!(self, OpKind::Join | OpKind::Sort | OpKind::Aggregate)
    }

    /// Stable lower-case label for reporting/serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            OpKind::Scan => "scan",
            OpKind::Filter => "filter",
            OpKind::Project => "project",
            OpKind::Join => "join",
            OpKind::Aggregate => "aggregate",
            OpKind::Sort => "sort",
            OpKind::Limit => "limit",
            OpKind::Distinct => "distinct",
            OpKind::AssertMaxOneRow => "assert_max_one_row",
            OpKind::Union => "union",
            OpKind::SetOp => "setop",
            OpKind::Values => "values",
        }
    }
}

/// A cheap, pre-execution geometric summary of a physical plan (TD-EXEC-2).
///
/// Every field maps first-principles to a dominant resource/cost term:
/// * `max_depth` → recursion stack (planner lowering, executor build) + pipeline length.
/// * `node_count` → total lowering/build frames.
/// * `leaf_count` → source fan-in (number of scans/values).
/// * `max_fanout` → parallelism (morsel lanes) + peak concurrent buffers.
/// * `op_histogram` → engine-cost feature + blocking/spill risk.
/// * `blocking_count` → pipeline breakers → memory, spill.
///
/// The struct is **data, not a decision** — the shared input to the TD-EXEC-2
/// resource laws.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanGeometry {
    /// Longest root→leaf path (root = depth 1). Drives stack recursion depth.
    pub max_depth: usize,
    /// Total operator count in the tree.
    pub node_count: usize,
    /// Number of leaves (Scan / Values) — the source fan-in.
    pub leaf_count: usize,
    /// Widest direct-children set at any single node (binary join = 2, N-way union = N).
    pub max_fanout: usize,
    /// Count of each operator kind. `BTreeMap` for deterministic reporting order.
    pub op_histogram: BTreeMap<OpKind, u16>,
    /// Number of pipeline-breaker operators (joins + sorts + aggregates).
    pub blocking_count: u16,
}

impl PlanGeometry {
    /// Count of operators of a given kind (0 if absent).
    pub fn count(&self, kind: OpKind) -> u16 {
        self.op_histogram.get(&kind).copied().unwrap_or(0)
    }
}

/// The direct child subplans of `plan` (0 for leaves, 1 for unary, 2 for binary,
/// N for an N-way `Union`). Ordered left-to-right so the traversal is deterministic.
fn children(plan: &PhysicalPlan) -> Vec<&PhysicalPlan> {
    match plan {
        PhysicalPlan::Scan { .. } | PhysicalPlan::Values { .. } => Vec::new(),
        PhysicalPlan::Filter { input, .. }
        | PhysicalPlan::Project { input, .. }
        | PhysicalPlan::Aggregate { input, .. }
        | PhysicalPlan::Sort { input, .. }
        | PhysicalPlan::Limit { input, .. }
        | PhysicalPlan::Distinct { input, .. }
        | PhysicalPlan::AssertMaxOneRow { input } => vec![input.as_ref()],
        PhysicalPlan::Join { left, right, .. } | PhysicalPlan::SetOp { left, right, .. } => {
            vec![left.as_ref(), right.as_ref()]
        }
        PhysicalPlan::Union { inputs, .. } => inputs.iter().collect(),
    }
}

/// Measure the plan's geometry in one **iterative** `O(nodes)` traversal.
///
/// Uses an explicit work-list of `(node, depth)` — never recursion — so the pass is
/// safe on arbitrarily deep plans (it is the tool that characterizes deep-plan stack
/// cost; it must not itself overflow). A physical plan is a tree, so each node is
/// visited exactly once and its `depth` is its unique distance from the root.
pub fn measure_geometry(root: &PhysicalPlan) -> PlanGeometry {
    let mut g = PlanGeometry::default();
    let mut work: Vec<(&PhysicalPlan, usize)> = vec![(root, 1)];
    while let Some((node, depth)) = work.pop() {
        g.node_count += 1;
        g.max_depth = g.max_depth.max(depth);

        let kind = OpKind::of(node);
        let entry = g.op_histogram.entry(kind).or_insert(0);
        *entry = entry.saturating_add(1);
        if kind.is_blocking() {
            g.blocking_count = g.blocking_count.saturating_add(1);
        }

        let kids = children(node);
        g.max_fanout = g.max_fanout.max(kids.len());
        if kids.is_empty() {
            g.leaf_count += 1;
        }
        for kid in kids {
            work.push((kid, depth + 1));
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AggregateStrategy, ScanAccess};
    use proximadb_data_model::ProximaValue;
    use proximadb_relational_algebra::{JoinKind, JoinStrategy, TableId};
    use proximadb_relational_types::{Expr, RelationalSchema};

    fn scan() -> PhysicalPlan {
        PhysicalPlan::Scan {
            table: TableId::new("t"),
            output_schema: RelationalSchema::new(vec![]),
            projection: None,
            predicate: None,
            limit: None,
            access: ScanAccess::FullScan,
        }
    }

    fn filter(input: PhysicalPlan) -> PhysicalPlan {
        PhysicalPlan::Filter {
            input: Box::new(input),
            predicate: Expr::literal(ProximaValue::Int64(1)),
        }
    }

    #[test]
    fn single_scan_is_a_depth_one_leaf() {
        let g = measure_geometry(&scan());
        assert_eq!(g.max_depth, 1);
        assert_eq!(g.node_count, 1);
        assert_eq!(g.leaf_count, 1);
        assert_eq!(g.max_fanout, 0);
        assert_eq!(g.blocking_count, 0);
        assert_eq!(g.count(OpKind::Scan), 1);
    }

    #[test]
    fn nested_unary_chain_measures_depth_and_count() {
        // Filter(Filter(Filter(Scan)))
        let mut p = scan();
        for _ in 0..3 {
            p = filter(p);
        }
        let g = measure_geometry(&p);
        assert_eq!(g.max_depth, 4); // 3 filters + 1 scan
        assert_eq!(g.node_count, 4);
        assert_eq!(g.leaf_count, 1);
        assert_eq!(g.max_fanout, 1); // each unary node has exactly one child
        assert_eq!(g.count(OpKind::Filter), 3);
        assert_eq!(g.count(OpKind::Scan), 1);
        assert_eq!(g.blocking_count, 0); // filters are streaming, not blocking
    }

    #[test]
    fn join_counts_two_leaves_and_a_blocking_op() {
        // Join(Scan, Filter(Scan)) — fanout 2, one blocking op, two leaves.
        let p = PhysicalPlan::Join {
            left: Box::new(scan()),
            right: Box::new(filter(scan())),
            kind: JoinKind::Inner,
            on: None,
            strategy: JoinStrategy::NestedLoop,
        };
        let g = measure_geometry(&p);
        assert_eq!(g.node_count, 4); // join + scan + filter + scan
        assert_eq!(g.leaf_count, 2); // two scans
        assert_eq!(g.max_fanout, 2); // the join has two children
        assert_eq!(g.max_depth, 3); // join → filter → scan
        assert_eq!(g.blocking_count, 1); // the join
        assert_eq!(g.count(OpKind::Join), 1);
    }

    #[test]
    fn aggregate_is_blocking() {
        let p = PhysicalPlan::Aggregate {
            input: Box::new(scan()),
            group_by: vec![],
            aggregates: vec![],
            having: None,
            strategy: AggregateStrategy::Streaming,
        };
        let g = measure_geometry(&p);
        assert_eq!(g.blocking_count, 1);
        assert_eq!(g.count(OpKind::Aggregate), 1);
        assert!(OpKind::Aggregate.is_blocking());
        assert!(!OpKind::Filter.is_blocking());
    }

    #[test]
    fn union_reports_n_way_fanout_and_all_leaves() {
        // Union of 5 scans — widest sibling set is 5, all children are leaves.
        let p = PhysicalPlan::Union {
            inputs: vec![scan(), scan(), scan(), scan(), scan()],
            all: true,
        };
        let g = measure_geometry(&p);
        assert_eq!(g.max_fanout, 5);
        assert_eq!(g.leaf_count, 5);
        assert_eq!(g.node_count, 6); // union + 5 scans
        assert_eq!(g.max_depth, 2);
        assert_eq!(g.count(OpKind::Union), 1);
        assert!(!OpKind::Union.is_blocking());
    }

    #[test]
    fn traversal_is_iterative_and_safe_on_deep_plans() {
        // A pathologically deep unary chain: `measure_geometry` must NOT overflow
        // (it is iterative). Build iteratively; the deep plan's own recursive Drop
        // is avoided by leaking it — only the measurement is under test here.
        const DEPTH: usize = 100_000;
        let mut p = scan();
        for _ in 0..DEPTH {
            p = filter(p);
        }
        let g = measure_geometry(&p);
        assert_eq!(g.max_depth, DEPTH + 1);
        assert_eq!(g.node_count, DEPTH + 1);
        assert_eq!(g.leaf_count, 1);
        std::mem::forget(p); // avoid a 100k-deep recursive Drop overflowing the stack
    }
}

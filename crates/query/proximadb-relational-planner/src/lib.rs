//! Physical planner for ProximaDB's relational query path
//! (ADR-019 L3 / S2). Consumes [`LogicalNode`] from the algebra
//! crate plus [`ReaderCapabilities`] from the reader crate, and
//! produces a [`PhysicalPlan`] with concrete physical strategies
//! and pushed-down operations on leaf `Scan` nodes.
//!
//! The planner runs three passes:
//!
//! 1. **Logical rewrites** (work on `LogicalNode`):
//!    - Constant folding (`Literal + Literal` → `Literal`).
//!    - Filter merge (adjacent `Filter` nodes combine into one
//!      with AND-ed predicate).
//!
//! 2. **Lower to physical** (`LogicalNode` → `PhysicalPlan`):
//!    - Strategy selection — picks `JoinStrategy`,
//!      `AggregateStrategy`, `SortStrategy`, `DistinctStrategy`.
//!    - Defaults for MVP: HashJoin for equi-joins,
//!      NestedLoop fallback; HashAggregate when GROUP BY,
//!      StreamingAggregate otherwise; InMemorySort always;
//!      HashDistinct always.
//!
//! 3. **Physical rewrites**:
//!    - Predicate pushdown into `Scan` (only when the adapter
//!      declares `push_predicate = true` via the capability
//!      resolver).
//!    - Projection pushdown into `Scan` (only when the adapter
//!      declares `project_columns = true`).
//!    - PK-lookup access selection: `Filter(Scan)` where the
//!      filter is a PK equality conjunction becomes
//!      `Scan { access: PkLookup, … }` (when the adapter
//!      declares `pk_lookup = true`).
//!
//! Cost-based optimisation is intentionally out of scope at MVP.
//! Every choice is rule-driven; the `JoinStrategy::Hash` build
//! side is picked statically (right side builds — adjust later
//! when we have row-count statistics).
//!
//! The planner is pure: no I/O. The capability resolver is the
//! only side-channel; it's injected as a closure so tests can
//! pin capabilities deterministically.

use proximadb_data_model::{ProximaType, ProximaValue};
use proximadb_relational_algebra::{
    AggregateExpr, AlgebraError, JoinKind, JoinSide, JoinStrategy, LogicalNode,
    NamedAggregate, NamedExpr, SortKey, TableId,
};
use proximadb_relational_reader::ReaderCapabilities;
use proximadb_relational_types::{
    BinaryOp, ColumnRef, Expr, ExprError, NoFunctions, RelationalSchema, UnaryOp,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// =========================================================================
// Errors
// =========================================================================

#[derive(Debug, Error, Clone, PartialEq)]
pub enum PlanError {
    #[error("algebra error: {0}")]
    Algebra(#[from] AlgebraError),

    #[error("expression error: {0}")]
    Expr(#[from] ExprError),

    #[error("internal planner error: {0}")]
    Internal(String),
}

// =========================================================================
// Physical strategy enums
// =========================================================================

/// How a `Scan` reads rows. The default is `FullScan`; the
/// PK-lookup rule rewrites to `PkLookup` when a `Filter` over the
/// scan is an equality conjunction matching the full primary key
/// AND the adapter declares `pk_lookup = true`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScanAccess {
    FullScan,
    PkLookup {
        /// Expressions that evaluate to the primary-key components
        /// at execution time. Same ordinal layout as the table's
        /// declared primary key.
        key: Vec<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AggregateStrategy {
    /// `GROUP BY` present — build a hash table keyed on group_by.
    Hash,
    /// No `GROUP BY` — single accumulator across all rows.
    Streaming,
    /// Input is already sorted on the group_by keys; stream
    /// groups incrementally. Reserved for Phase 3 once sort
    /// ordering propagation lands.
    Sorted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortStrategy {
    /// All rows held in memory, sorted at EOF.
    InMemory,
    /// External k-way merge with spilling. Phase 3.
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DistinctStrategy {
    /// Hash-based de-duplication.
    Hash,
    /// Sort-based — useful when input is already sorted on the
    /// distinct columns. Phase 3.
    Sort,
}

// =========================================================================
// PhysicalPlan
// =========================================================================

/// Physical plan — produced by [`Planner::plan`]. Mirrors
/// [`LogicalNode`] but every operator has a concrete strategy and
/// leaf `Scan`s carry pushed-down hints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PhysicalPlan {
    Scan {
        table: TableId,
        /// Schema the scan emits AFTER projection pushdown.
        output_schema: RelationalSchema,
        /// Pushed-down projection (column names). `None` = emit
        /// every column in the table's natural order.
        projection: Option<Vec<String>>,
        /// Pushed-down predicate. The adapter MAY use it for
        /// block skipping; the executor always re-applies per row.
        predicate: Option<Expr>,
        /// Pushed-down limit hint (the executor enforces the
        /// final limit defensively).
        limit: Option<u64>,
        access: ScanAccess,
    },
    Filter {
        input: Box<PhysicalPlan>,
        predicate: Expr,
    },
    Project {
        input: Box<PhysicalPlan>,
        outputs: Vec<NamedExpr>,
    },
    Join {
        left: Box<PhysicalPlan>,
        right: Box<PhysicalPlan>,
        kind: JoinKind,
        on: Option<Expr>,
        /// Concrete strategy — never `Auto` after planning.
        strategy: JoinStrategy,
    },
    Aggregate {
        input: Box<PhysicalPlan>,
        group_by: Vec<NamedExpr>,
        aggregates: Vec<NamedAggregate>,
        having: Option<Expr>,
        strategy: AggregateStrategy,
    },
    Sort {
        input: Box<PhysicalPlan>,
        keys: Vec<SortKey>,
        strategy: SortStrategy,
    },
    Limit {
        input: Box<PhysicalPlan>,
        limit: Option<u64>,
        offset: u64,
    },
    Distinct {
        input: Box<PhysicalPlan>,
        strategy: DistinctStrategy,
    },
    Union {
        inputs: Vec<PhysicalPlan>,
        all: bool,
    },
    Values {
        rows: Vec<Vec<Expr>>,
        output_schema: RelationalSchema,
    },
}

impl PhysicalPlan {
    /// Output schema of this node. Mirrors the algebra crate's
    /// `LogicalNode::output_schema` because the planner is
    /// non-destructive: rewrites preserve every node's output
    /// shape.
    pub fn output_schema(&self) -> RelationalSchema {
        match self {
            PhysicalPlan::Scan { output_schema, .. } => output_schema.clone(),
            PhysicalPlan::Filter { input, .. } => input.output_schema(),
            PhysicalPlan::Project { outputs, .. } => RelationalSchema::new(
                outputs
                    .iter()
                    .map(|o| proximadb_relational_types::ColumnInfo {
                        name: o.name.clone(),
                        ty: o.expr.result_type(),
                        nullable: true,
                    })
                    .collect(),
            ),
            PhysicalPlan::Join {
                left, right, kind, ..
            } => {
                let mut cols = left.output_schema().columns;
                if matches!(kind, JoinKind::Semi | JoinKind::Anti) {
                    return RelationalSchema::new(cols);
                }
                cols.extend(right.output_schema().columns);
                RelationalSchema::new(cols)
            }
            PhysicalPlan::Aggregate {
                group_by,
                aggregates,
                ..
            } => {
                let mut cols: Vec<proximadb_relational_types::ColumnInfo> = group_by
                    .iter()
                    .map(|g| proximadb_relational_types::ColumnInfo {
                        name: g.name.clone(),
                        ty: g.expr.result_type(),
                        nullable: true,
                    })
                    .collect();
                for a in aggregates {
                    cols.push(proximadb_relational_types::ColumnInfo {
                        name: a.name.clone(),
                        ty: a.agg.result_type(),
                        nullable: !matches!(a.agg, AggregateExpr::Count { .. }),
                    });
                }
                RelationalSchema::new(cols)
            }
            PhysicalPlan::Sort { input, .. }
            | PhysicalPlan::Limit { input, .. }
            | PhysicalPlan::Distinct { input, .. } => input.output_schema(),
            PhysicalPlan::Union { inputs, .. } => inputs
                .first()
                .map(|i| i.output_schema())
                .unwrap_or_default(),
            PhysicalPlan::Values { output_schema, .. } => output_schema.clone(),
        }
    }
}

// =========================================================================
// Planner
// =========================================================================

/// Capability lookup: given a table, what can the adapter do?
/// Production callers wire this to a registry; tests pass a
/// closure with deterministic capabilities.
pub trait CapabilityResolver: Send + Sync {
    fn capabilities(&self, table: &TableId) -> ReaderCapabilities;

    /// Primary key column ordinals for `table`. Used by the
    /// PK-lookup rewrite. Empty Vec means no PK (or unknown);
    /// PK-lookup rewriting is skipped.
    fn primary_key(&self, table: &TableId) -> Vec<usize> {
        let _ = table;
        Vec::new()
    }
}

/// Reference resolver that always returns the same capabilities
/// and PK. Used in tests; production wires a real registry.
pub struct StaticCapabilities {
    pub caps: ReaderCapabilities,
    pub pk_columns: Vec<usize>,
}

impl CapabilityResolver for StaticCapabilities {
    fn capabilities(&self, _table: &TableId) -> ReaderCapabilities {
        self.caps
    }
    fn primary_key(&self, _table: &TableId) -> Vec<usize> {
        self.pk_columns.clone()
    }
}

/// The planner. Holds a [`CapabilityResolver`] for adapter
/// pushdown decisions.
pub struct Planner<R: CapabilityResolver> {
    resolver: R,
}

impl<R: CapabilityResolver> Planner<R> {
    pub fn new(resolver: R) -> Self {
        Self { resolver }
    }

    /// Run the full planning pipeline: logical rewrites → lower
    /// → physical rewrites. Validates the input plan first.
    pub fn plan(&self, logical: LogicalNode) -> Result<PhysicalPlan, PlanError> {
        logical.validate()?;
        // 1. Logical rewrites.
        let logical = rewrite_logical(logical);
        // 2. Lower to physical.
        let physical = lower_to_physical(logical);
        // 3. Physical rewrites: predicate pushdown (with the
        //    PK-lookup rewrite as a sub-case), then projection
        //    pushdown (which narrows each Scan's output and
        //    rebinds upstream column ordinals).
        let physical = push_predicates(physical, &self.resolver);
        let physical = push_projections(physical)?;
        Ok(physical)
    }
}

// =========================================================================
// Pass 1: Logical rewrites
// =========================================================================

pub fn rewrite_logical(node: LogicalNode) -> LogicalNode {
    proximadb_relational_algebra::transform(node, |n| {
        let n = merge_filters(n);
        constant_fold_expressions(n)
    })
}

/// `Filter(Filter(input, a), b)` → `Filter(input, a AND b)`.
/// Reduces tree depth and lets predicate-pushdown collapse the
/// combined predicate into the scan in one pass.
pub fn merge_filters(node: LogicalNode) -> LogicalNode {
    match node {
        LogicalNode::Filter {
            input,
            predicate: outer,
        } => {
            if let LogicalNode::Filter {
                input: inner,
                predicate: inner_pred,
            } = *input
            {
                LogicalNode::Filter {
                    input: inner,
                    predicate: Expr::BinaryOp {
                        op: BinaryOp::And,
                        left: Box::new(inner_pred),
                        right: Box::new(outer),
                    },
                }
            } else {
                LogicalNode::Filter {
                    input: Box::new(*input),
                    predicate: outer,
                }
            }
        }
        other => other,
    }
}

/// Constant folding on expressions inside a node. The
/// `transform` visitor calls this bottom-up; only the
/// expressions in the current node are folded (children are
/// already visited).
pub fn constant_fold_expressions(node: LogicalNode) -> LogicalNode {
    match node {
        LogicalNode::Filter { input, predicate } => LogicalNode::Filter {
            input,
            predicate: fold_expr(predicate),
        },
        LogicalNode::Project { input, outputs } => LogicalNode::Project {
            input,
            outputs: outputs
                .into_iter()
                .map(|o| NamedExpr {
                    name: o.name,
                    expr: fold_expr(o.expr),
                })
                .collect(),
        },
        LogicalNode::Sort { input, keys } => LogicalNode::Sort {
            input,
            keys: keys
                .into_iter()
                .map(|k| SortKey {
                    expr: fold_expr(k.expr),
                    descending: k.descending,
                    nulls_first: k.nulls_first,
                })
                .collect(),
        },
        LogicalNode::Join {
            left,
            right,
            kind,
            on,
            strategy,
        } => LogicalNode::Join {
            left,
            right,
            kind,
            on: on.map(fold_expr),
            strategy,
        },
        LogicalNode::Aggregate {
            input,
            group_by,
            aggregates,
            having,
        } => LogicalNode::Aggregate {
            input,
            group_by: group_by
                .into_iter()
                .map(|g| NamedExpr {
                    name: g.name,
                    expr: fold_expr(g.expr),
                })
                .collect(),
            aggregates,
            having: having.map(fold_expr),
        },
        other => other,
    }
}

/// Single-pass constant fold over an expression. Pure literals
/// short-circuit; column refs and other non-literal subtrees pass
/// through untouched.
pub fn fold_expr(expr: Expr) -> Expr {
    match expr {
        Expr::BinaryOp { op, left, right } => {
            let left = fold_expr(*left);
            let right = fold_expr(*right);
            // If BOTH sides are literals, evaluate.
            if let (Expr::Literal { .. }, Expr::Literal { .. }) = (&left, &right) {
                if let Ok(v) = (Expr::BinaryOp {
                    op,
                    left: Box::new(left.clone()),
                    right: Box::new(right.clone()),
                })
                .eval(&Vec::new(), &NoFunctions)
                {
                    return Expr::literal(v);
                }
            }
            Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            }
        }
        Expr::UnaryOp { op, expr } => {
            let inner = fold_expr(*expr);
            if let Expr::Literal { .. } = &inner {
                if let Ok(v) = (Expr::UnaryOp {
                    op,
                    expr: Box::new(inner.clone()),
                })
                .eval(&Vec::new(), &NoFunctions)
                {
                    return Expr::literal(v);
                }
            }
            Expr::UnaryOp {
                op,
                expr: Box::new(inner),
            }
        }
        Expr::Cast { expr, ty } => Expr::Cast {
            expr: Box::new(fold_expr(*expr)),
            ty,
        },
        Expr::IsNull { expr, not } => Expr::IsNull {
            expr: Box::new(fold_expr(*expr)),
            not,
        },
        Expr::Between {
            expr,
            low,
            high,
            not,
        } => Expr::Between {
            expr: Box::new(fold_expr(*expr)),
            low: Box::new(fold_expr(*low)),
            high: Box::new(fold_expr(*high)),
            not,
        },
        Expr::In { expr, list, not } => Expr::In {
            expr: Box::new(fold_expr(*expr)),
            list: list.into_iter().map(fold_expr).collect(),
            not,
        },
        Expr::Like {
            expr,
            pattern,
            not,
            case_insensitive,
        } => Expr::Like {
            expr: Box::new(fold_expr(*expr)),
            pattern: Box::new(fold_expr(*pattern)),
            not,
            case_insensitive,
        },
        Expr::Case {
            branches,
            otherwise,
        } => Expr::Case {
            branches: branches
                .into_iter()
                .map(|(c, t)| (fold_expr(c), fold_expr(t)))
                .collect(),
            otherwise: otherwise.map(|o| Box::new(fold_expr(*o))),
        },
        Expr::Coalesce(args) => Expr::Coalesce(args.into_iter().map(fold_expr).collect()),
        Expr::NullIf { left, right } => Expr::NullIf {
            left: Box::new(fold_expr(*left)),
            right: Box::new(fold_expr(*right)),
        },
        Expr::FuncCall {
            name,
            args,
            return_ty,
        } => Expr::FuncCall {
            name,
            args: args.into_iter().map(fold_expr).collect(),
            return_ty,
        },
        other => other,
    }
}

// =========================================================================
// Pass 2: Lower to physical
// =========================================================================

pub fn lower_to_physical(node: LogicalNode) -> PhysicalPlan {
    match node {
        LogicalNode::Scan {
            table,
            table_schema,
            projected_columns,
            predicate,
        } => PhysicalPlan::Scan {
            table,
            output_schema: match &projected_columns {
                Some(cols) => table_schema.project(cols),
                None => table_schema.clone(),
            },
            projection: projected_columns
                .as_ref()
                .map(|cols| cols.iter().map(|c| c.name.clone()).collect()),
            predicate,
            limit: None,
            access: ScanAccess::FullScan,
        },
        LogicalNode::Filter { input, predicate } => PhysicalPlan::Filter {
            input: Box::new(lower_to_physical(*input)),
            predicate,
        },
        LogicalNode::Project { input, outputs } => PhysicalPlan::Project {
            input: Box::new(lower_to_physical(*input)),
            outputs,
        },
        LogicalNode::Join {
            left,
            right,
            kind,
            on,
            strategy: _,
        } => {
            let strategy = pick_join_strategy(kind, on.as_ref());
            PhysicalPlan::Join {
                left: Box::new(lower_to_physical(*left)),
                right: Box::new(lower_to_physical(*right)),
                kind,
                on,
                strategy,
            }
        }
        LogicalNode::Aggregate {
            input,
            group_by,
            aggregates,
            having,
        } => {
            let strategy = if group_by.is_empty() {
                AggregateStrategy::Streaming
            } else {
                AggregateStrategy::Hash
            };
            PhysicalPlan::Aggregate {
                input: Box::new(lower_to_physical(*input)),
                group_by,
                aggregates,
                having,
                strategy,
            }
        }
        LogicalNode::Sort { input, keys } => PhysicalPlan::Sort {
            input: Box::new(lower_to_physical(*input)),
            keys,
            strategy: SortStrategy::InMemory,
        },
        LogicalNode::Limit {
            input,
            limit,
            offset,
        } => PhysicalPlan::Limit {
            input: Box::new(lower_to_physical(*input)),
            limit,
            offset,
        },
        LogicalNode::Distinct { input } => PhysicalPlan::Distinct {
            input: Box::new(lower_to_physical(*input)),
            strategy: DistinctStrategy::Hash,
        },
        LogicalNode::Union { inputs, all } => PhysicalPlan::Union {
            inputs: inputs.into_iter().map(lower_to_physical).collect(),
            all,
        },
        LogicalNode::Values {
            rows,
            output_schema,
        } => PhysicalPlan::Values {
            rows,
            output_schema,
        },
        LogicalNode::CteBind { usages, .. } => {
            // MVP: inline single-use CTEs by lowering `usages`
            // directly. Multi-use materialisation is Phase 3.
            lower_to_physical(*usages)
        }
        LogicalNode::CteRef { .. } => {
            // Unbound at MVP — should have been resolved by a
            // pre-pass. For now we lower to an empty Values node
            // with the declared schema so downstream operators
            // don't crash; the planner's validate() catches the
            // case before we get here.
            PhysicalPlan::Values {
                rows: Vec::new(),
                output_schema: RelationalSchema::default(),
            }
        }
    }
}

/// Default join-strategy picker for MVP. Cost-aware selection
/// (build-side choice via cardinality estimate) is Phase 3.
pub fn pick_join_strategy(kind: JoinKind, on: Option<&Expr>) -> JoinStrategy {
    match kind {
        // CROSS has no ON — nested loop is the only correct
        // strategy (and the optimiser shouldn't see CROSS very
        // often in well-formed plans).
        JoinKind::Cross => JoinStrategy::NestedLoop,
        _ => match on {
            Some(predicate) if is_equi_join_predicate(predicate) => JoinStrategy::Hash {
                build_side: JoinSide::Right,
            },
            _ => JoinStrategy::NestedLoop,
        },
    }
}

/// True if the predicate is purely equality conjunctions
/// (`col_l = col_r [AND col_l = col_r]…`). Hash join wants this
/// shape; anything more complex falls back to nested loop.
fn is_equi_join_predicate(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp {
            op: BinaryOp::Eq, ..
        } => true,
        Expr::BinaryOp {
            op: BinaryOp::And,
            left,
            right,
        } => is_equi_join_predicate(left) && is_equi_join_predicate(right),
        _ => false,
    }
}

// =========================================================================
// Pass 3: Physical rewrites
// =========================================================================

/// Predicate pushdown — `Filter(Scan)` becomes
/// `Scan{predicate}` when the adapter declares
/// `push_predicate = true`. Walks the tree bottom-up.
pub fn push_predicates<R: CapabilityResolver>(
    plan: PhysicalPlan,
    resolver: &R,
) -> PhysicalPlan {
    match plan {
        PhysicalPlan::Filter { input, predicate } => {
            let input = push_predicates(*input, resolver);
            // Only push into a bare Scan node — don't reach past
            // intermediate operators (those would need rewriting).
            match input {
                PhysicalPlan::Scan {
                    table,
                    output_schema,
                    projection,
                    predicate: existing,
                    limit,
                    access,
                } => {
                    let caps = resolver.capabilities(&table);
                    // Try PK-lookup rewrite first when applicable.
                    let pk_cols = resolver.primary_key(&table);
                    let pk_lookup_supported = caps.pk_lookup;
                    let combined =
                        combine_predicates(existing.clone(), Some(predicate.clone()));
                    if pk_lookup_supported
                        && !pk_cols.is_empty()
                        && matches!(access, ScanAccess::FullScan)
                        && let Some(key) =
                            try_pk_lookup(&combined, &pk_cols, &output_schema)
                    {
                        return PhysicalPlan::Scan {
                            table,
                            output_schema,
                            projection,
                            // PK lookup acts as the predicate; we
                            // can drop the predicate from the scan
                            // since the adapter resolves the single
                            // matching row directly.
                            predicate: None,
                            limit,
                            access: ScanAccess::PkLookup { key },
                        };
                    }
                    if caps.push_predicate {
                        PhysicalPlan::Scan {
                            table,
                            output_schema,
                            projection,
                            predicate: combined,
                            limit,
                            access,
                        }
                    } else {
                        // Adapter doesn't push predicates;
                        // restore the Filter above the Scan.
                        PhysicalPlan::Filter {
                            input: Box::new(PhysicalPlan::Scan {
                                table,
                                output_schema,
                                projection,
                                predicate: existing,
                                limit,
                                access,
                            }),
                            predicate,
                        }
                    }
                }
                other => PhysicalPlan::Filter {
                    input: Box::new(other),
                    predicate,
                },
            }
        }
        PhysicalPlan::Project { input, outputs } => PhysicalPlan::Project {
            input: Box::new(push_predicates(*input, resolver)),
            outputs,
        },
        PhysicalPlan::Sort {
            input,
            keys,
            strategy,
        } => PhysicalPlan::Sort {
            input: Box::new(push_predicates(*input, resolver)),
            keys,
            strategy,
        },
        PhysicalPlan::Limit {
            input,
            limit,
            offset,
        } => PhysicalPlan::Limit {
            input: Box::new(push_predicates(*input, resolver)),
            limit,
            offset,
        },
        PhysicalPlan::Distinct { input, strategy } => PhysicalPlan::Distinct {
            input: Box::new(push_predicates(*input, resolver)),
            strategy,
        },
        PhysicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
            having,
            strategy,
        } => PhysicalPlan::Aggregate {
            input: Box::new(push_predicates(*input, resolver)),
            group_by,
            aggregates,
            having,
            strategy,
        },
        PhysicalPlan::Join {
            left,
            right,
            kind,
            on,
            strategy,
        } => PhysicalPlan::Join {
            left: Box::new(push_predicates(*left, resolver)),
            right: Box::new(push_predicates(*right, resolver)),
            kind,
            on,
            strategy,
        },
        PhysicalPlan::Union { inputs, all } => PhysicalPlan::Union {
            inputs: inputs
                .into_iter()
                .map(|i| push_predicates(i, resolver))
                .collect(),
            all,
        },
        leaf @ (PhysicalPlan::Scan { .. } | PhysicalPlan::Values { .. }) => leaf,
    }
}

fn combine_predicates(a: Option<Expr>, b: Option<Expr>) -> Option<Expr> {
    match (a, b) {
        (None, None) => None,
        (Some(p), None) | (None, Some(p)) => Some(p),
        (Some(l), Some(r)) => Some(Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(l),
            right: Box::new(r),
        }),
    }
}

/// Attempt to extract a PK lookup from a predicate. Returns
/// `Some(key_exprs)` when the predicate is an AND-chain of
/// equalities covering every PK column with a literal-or-bound
/// expression on the right-hand side.
fn try_pk_lookup(
    predicate: &Option<Expr>,
    pk_cols: &[usize],
    _output_schema: &RelationalSchema,
) -> Option<Vec<Expr>> {
    let p = predicate.as_ref()?;
    let conjuncts = flatten_and(p);
    let mut key: Vec<Option<Expr>> = vec![None; pk_cols.len()];
    for conj in &conjuncts {
        let Expr::BinaryOp {
            op: BinaryOp::Eq,
            left,
            right,
        } = conj
        else {
            return None;
        };
        // Find which side is the column ref and which is the
        // value. PK lookup wants `pk_col = <literal-or-bound>`.
        let (col, value) = match (left.as_ref(), right.as_ref()) {
            (Expr::Column(c), v) => (c, v),
            (v, Expr::Column(c)) => (c, v),
            _ => return None,
        };
        // Find which PK slot the column matches.
        let slot = pk_cols.iter().position(|p| *p == col.ordinal)?;
        // Value must not itself reference a column (we need a
        // constant expression to probe with).
        if expression_references_columns(value) {
            return None;
        }
        if key[slot].is_some() {
            // Duplicate predicate on the same PK column —
            // conservative: don't rewrite.
            return None;
        }
        key[slot] = Some(value.clone());
    }
    // All PK slots must be covered.
    key.into_iter().collect()
}

fn flatten_and(expr: &Expr) -> Vec<&Expr> {
    let mut out = Vec::new();
    flatten_and_into(expr, &mut out);
    out
}

fn flatten_and_into<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::BinaryOp {
            op: BinaryOp::And,
            left,
            right,
        } => {
            flatten_and_into(left, out);
            flatten_and_into(right, out);
        }
        other => out.push(other),
    }
}

fn expression_references_columns(expr: &Expr) -> bool {
    match expr {
        Expr::Column(_) => true,
        Expr::Literal { .. } => false,
        Expr::Cast { expr, .. } | Expr::UnaryOp { expr, .. } | Expr::IsNull { expr, .. } => {
            expression_references_columns(expr)
        }
        Expr::BinaryOp { left, right, .. } => {
            expression_references_columns(left) || expression_references_columns(right)
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            expression_references_columns(expr)
                || expression_references_columns(low)
                || expression_references_columns(high)
        }
        Expr::In { expr, list, .. } => {
            expression_references_columns(expr) || list.iter().any(expression_references_columns)
        }
        Expr::Like { expr, pattern, .. } => {
            expression_references_columns(expr) || expression_references_columns(pattern)
        }
        Expr::Case {
            branches,
            otherwise,
        } => {
            branches
                .iter()
                .any(|(c, t)| expression_references_columns(c) || expression_references_columns(t))
                || otherwise
                    .as_ref()
                    .is_some_and(|o| expression_references_columns(o))
        }
        Expr::Coalesce(args) => args.iter().any(expression_references_columns),
        Expr::NullIf { left, right } => {
            expression_references_columns(left) || expression_references_columns(right)
        }
        Expr::FuncCall { args, .. } => args.iter().any(expression_references_columns),
    }
}

/// Projection pushdown — narrow each Scan to only the columns
/// referenced upstream, then walk back up the tree rebinding
/// every `Expr::Column` ordinal against the narrowed schema so
/// runtime evaluators don't index into the wrong slot.
///
/// Bottom-up name collection (top-down pass) figures out which
/// columns each Scan must emit; the recursion then narrows the
/// Scan and threads each operator's expressions through
/// [`rebind_columns`] against its (possibly narrowed) input
/// schema. Join subtrees are skipped: rebinding across a
/// two-input combined schema needs name disambiguation that the
/// current `ColumnRef` shape doesn't carry. Phase 3 follow-up
/// will lift that restriction.
pub fn push_projections(plan: PhysicalPlan) -> Result<PhysicalPlan, PlanError> {
    let required: Vec<String> = plan
        .output_schema()
        .columns
        .iter()
        .map(|c| c.name.clone())
        .collect();
    push_projections_inner(plan, &required)
}

/// Walk an expression and rebind every `Expr::Column.ordinal`
/// against `schema` by looking up the column name. Used by the
/// projection-pushdown rebind pass: when a Scan is narrowed, its
/// rows have different ordinal positions, so the parent
/// operator's expressions need their ordinals reshuffled.
fn rebind_columns(expr: Expr, schema: &RelationalSchema) -> Result<Expr, PlanError> {
    match expr {
        Expr::Column(c) => {
            let (idx, info) = schema.column_by_name(&c.name).ok_or_else(|| {
                PlanError::Internal(format!(
                    "rebind_columns: column `{}` not in narrowed schema",
                    c.name
                ))
            })?;
            Ok(Expr::Column(ColumnRef {
                name: c.name,
                ordinal: idx,
                ty: info.ty.clone(),
                nullable: info.nullable,
            }))
        }
        Expr::Literal { .. } => Ok(expr),
        Expr::Cast { expr, ty } => Ok(Expr::Cast {
            expr: Box::new(rebind_columns(*expr, schema)?),
            ty,
        }),
        Expr::UnaryOp { op, expr } => Ok(Expr::UnaryOp {
            op,
            expr: Box::new(rebind_columns(*expr, schema)?),
        }),
        Expr::BinaryOp { op, left, right } => Ok(Expr::BinaryOp {
            op,
            left: Box::new(rebind_columns(*left, schema)?),
            right: Box::new(rebind_columns(*right, schema)?),
        }),
        Expr::IsNull { expr, not } => Ok(Expr::IsNull {
            expr: Box::new(rebind_columns(*expr, schema)?),
            not,
        }),
        Expr::Between {
            expr,
            low,
            high,
            not,
        } => Ok(Expr::Between {
            expr: Box::new(rebind_columns(*expr, schema)?),
            low: Box::new(rebind_columns(*low, schema)?),
            high: Box::new(rebind_columns(*high, schema)?),
            not,
        }),
        Expr::In { expr, list, not } => Ok(Expr::In {
            expr: Box::new(rebind_columns(*expr, schema)?),
            list: list
                .into_iter()
                .map(|e| rebind_columns(e, schema))
                .collect::<Result<_, _>>()?,
            not,
        }),
        Expr::Like {
            expr,
            pattern,
            not,
            case_insensitive,
        } => Ok(Expr::Like {
            expr: Box::new(rebind_columns(*expr, schema)?),
            pattern: Box::new(rebind_columns(*pattern, schema)?),
            not,
            case_insensitive,
        }),
        Expr::Case {
            branches,
            otherwise,
        } => Ok(Expr::Case {
            branches: branches
                .into_iter()
                .map(|(c, t)| -> Result<_, PlanError> {
                    Ok((rebind_columns(c, schema)?, rebind_columns(t, schema)?))
                })
                .collect::<Result<_, _>>()?,
            otherwise: match otherwise {
                Some(o) => Some(Box::new(rebind_columns(*o, schema)?)),
                None => None,
            },
        }),
        Expr::Coalesce(args) => Ok(Expr::Coalesce(
            args.into_iter()
                .map(|e| rebind_columns(e, schema))
                .collect::<Result<_, _>>()?,
        )),
        Expr::NullIf { left, right } => Ok(Expr::NullIf {
            left: Box::new(rebind_columns(*left, schema)?),
            right: Box::new(rebind_columns(*right, schema)?),
        }),
        Expr::FuncCall {
            name,
            args,
            return_ty,
        } => Ok(Expr::FuncCall {
            name,
            args: args
                .into_iter()
                .map(|e| rebind_columns(e, schema))
                .collect::<Result<_, _>>()?,
            return_ty,
        }),
    }
}

/// Same as [`rebind_columns`] but for an [`AggregateExpr`]'s
/// argument expressions (which reference the Aggregate node's
/// INPUT schema, not the output schema). Mirrors the shape of
/// [`aggregate_input_exprs`].
fn rebind_aggregate(
    agg: AggregateExpr,
    schema: &RelationalSchema,
) -> Result<AggregateExpr, PlanError> {
    Ok(match agg {
        AggregateExpr::Count { arg, distinct } => AggregateExpr::Count {
            arg: match arg {
                Some(e) => Some(rebind_columns(e, schema)?),
                None => None,
            },
            distinct,
        },
        AggregateExpr::Sum { arg, distinct } => AggregateExpr::Sum {
            arg: rebind_columns(arg, schema)?,
            distinct,
        },
        AggregateExpr::Avg { arg, distinct } => AggregateExpr::Avg {
            arg: rebind_columns(arg, schema)?,
            distinct,
        },
        AggregateExpr::Min { arg } => AggregateExpr::Min {
            arg: rebind_columns(arg, schema)?,
        },
        AggregateExpr::Max { arg } => AggregateExpr::Max {
            arg: rebind_columns(arg, schema)?,
        },
        AggregateExpr::StringAgg {
            arg,
            separator,
            distinct,
        } => AggregateExpr::StringAgg {
            arg: rebind_columns(arg, schema)?,
            separator,
            distinct,
        },
        AggregateExpr::Custom {
            name,
            args,
            distinct,
            return_ty,
        } => AggregateExpr::Custom {
            name,
            args: args
                .into_iter()
                .map(|e| rebind_columns(e, schema))
                .collect::<Result<_, _>>()?,
            distinct,
            return_ty,
        },
    })
}

fn push_projections_inner(
    plan: PhysicalPlan,
    required: &[String],
) -> Result<PhysicalPlan, PlanError> {
    match plan {
        PhysicalPlan::Scan {
            table,
            output_schema,
            projection,
            predicate,
            limit,
            access,
        } => {
            // PK-lookup scans return the full table row; the
            // executor doesn't yet apply a post-lookup projection,
            // so skip narrowing on this path. (Phase 3 will
            // honour projection through lookup_pk by trimming the
            // returned row in ScanExec.)
            if matches!(access, ScanAccess::PkLookup { .. }) {
                return Ok(PhysicalPlan::Scan {
                    table,
                    output_schema,
                    projection,
                    predicate,
                    limit,
                    access,
                });
            }
            // If the scan already has an explicit projection,
            // honour it. Otherwise propagate the upstream
            // requirement when it's a strict subset of the
            // table's columns.
            let new_projection = match projection {
                Some(p) => Some(p),
                None => {
                    let table_cols: Vec<String> = output_schema
                        .columns
                        .iter()
                        .map(|c| c.name.clone())
                        .collect();
                    let needed: Vec<String> = required
                        .iter()
                        .filter(|n| table_cols.iter().any(|c| c == *n))
                        .cloned()
                        .collect();
                    // Pushing also requires any columns referenced
                    // by the pushed-down predicate. Conservative
                    // path: include every column the predicate
                    // touches.
                    let pred_cols = predicate
                        .as_ref()
                        .map(collect_column_refs)
                        .unwrap_or_default();
                    let mut combined: Vec<String> = needed;
                    for c in &pred_cols {
                        if !combined.contains(c) {
                            combined.push(c.clone());
                        }
                    }
                    if combined.is_empty()
                        || combined.len() == output_schema.len()
                    {
                        // No narrowing possible.
                        None
                    } else {
                        Some(combined)
                    }
                }
            };
            // Recompute output_schema if we narrowed.
            let projected_schema = match &new_projection {
                Some(cols) => RelationalSchema::new(
                    cols.iter()
                        .filter_map(|n| {
                            output_schema
                                .columns
                                .iter()
                                .find(|c| &c.name == n)
                                .cloned()
                        })
                        .collect(),
                ),
                None => output_schema,
            };
            Ok(PhysicalPlan::Scan {
                table,
                output_schema: projected_schema,
                projection: new_projection,
                predicate,
                limit,
                access,
            })
        }
        PhysicalPlan::Filter { input, predicate } => {
            let mut needed: Vec<String> = required.to_vec();
            for c in collect_column_refs(&predicate) {
                if !needed.contains(&c) {
                    needed.push(c);
                }
            }
            let new_input = push_projections_inner(*input, &needed)?;
            let schema = new_input.output_schema();
            let new_predicate = rebind_columns(predicate, &schema)?;
            Ok(PhysicalPlan::Filter {
                input: Box::new(new_input),
                predicate: new_predicate,
            })
        }
        PhysicalPlan::Project { input, outputs } => {
            let mut needed: Vec<String> = Vec::new();
            for out in &outputs {
                for c in collect_column_refs(&out.expr) {
                    if !needed.contains(&c) {
                        needed.push(c);
                    }
                }
            }
            let new_input = push_projections_inner(*input, &needed)?;
            let schema = new_input.output_schema();
            let new_outputs: Vec<NamedExpr> = outputs
                .into_iter()
                .map(|o| -> Result<_, PlanError> {
                    Ok(NamedExpr {
                        name: o.name,
                        expr: rebind_columns(o.expr, &schema)?,
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok(PhysicalPlan::Project {
                input: Box::new(new_input),
                outputs: new_outputs,
            })
        }
        PhysicalPlan::Sort {
            input,
            keys,
            strategy,
        } => {
            let mut needed: Vec<String> = required.to_vec();
            for k in &keys {
                for c in collect_column_refs(&k.expr) {
                    if !needed.contains(&c) {
                        needed.push(c);
                    }
                }
            }
            let new_input = push_projections_inner(*input, &needed)?;
            let schema = new_input.output_schema();
            let new_keys: Vec<SortKey> = keys
                .into_iter()
                .map(|k| -> Result<_, PlanError> {
                    Ok(SortKey {
                        expr: rebind_columns(k.expr, &schema)?,
                        descending: k.descending,
                        nulls_first: k.nulls_first,
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok(PhysicalPlan::Sort {
                input: Box::new(new_input),
                keys: new_keys,
                strategy,
            })
        }
        PhysicalPlan::Limit {
            input,
            limit,
            offset,
        } => Ok(PhysicalPlan::Limit {
            input: Box::new(push_projections_inner(*input, required)?),
            limit,
            offset,
        }),
        PhysicalPlan::Distinct { input, strategy } => Ok(PhysicalPlan::Distinct {
            input: Box::new(push_projections_inner(*input, required)?),
            strategy,
        }),
        PhysicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
            having,
            strategy,
        } => {
            let mut needed: Vec<String> = Vec::new();
            for g in &group_by {
                for c in collect_column_refs(&g.expr) {
                    if !needed.contains(&c) {
                        needed.push(c);
                    }
                }
            }
            for a in &aggregates {
                for e in aggregate_input_exprs(&a.agg) {
                    for c in collect_column_refs(e) {
                        if !needed.contains(&c) {
                            needed.push(c);
                        }
                    }
                }
            }
            // HAVING references the POST-aggregate schema, not the
            // input schema, so don't add its columns to `needed`.
            let new_input = push_projections_inner(*input, &needed)?;
            let input_schema = new_input.output_schema();
            let new_group_by: Vec<NamedExpr> = group_by
                .into_iter()
                .map(|g| -> Result<_, PlanError> {
                    Ok(NamedExpr {
                        name: g.name,
                        expr: rebind_columns(g.expr, &input_schema)?,
                    })
                })
                .collect::<Result<_, _>>()?;
            let new_aggregates: Vec<NamedAggregate> = aggregates
                .into_iter()
                .map(|a| -> Result<_, PlanError> {
                    Ok(NamedAggregate {
                        name: a.name,
                        agg: rebind_aggregate(a.agg, &input_schema)?,
                    })
                })
                .collect::<Result<_, _>>()?;
            // HAVING runs against the post-aggregate schema —
            // don't rebind against input_schema. Phase 3 will
            // build a proper post-aggregate scope for HAVING.
            Ok(PhysicalPlan::Aggregate {
                input: Box::new(new_input),
                group_by: new_group_by,
                aggregates: new_aggregates,
                having,
                strategy,
            })
        }
        PhysicalPlan::Join {
            left,
            right,
            kind,
            on,
            strategy,
        } => {
            // Skip projection pushdown across joins: rebinding
            // across a combined left+right schema needs name
            // disambiguation that ColumnRef doesn't yet carry.
            // Recurse with each side's full schema so no
            // narrowing happens, and pass `on` through unchanged.
            let left_full: Vec<String> = left
                .output_schema()
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect();
            let right_full: Vec<String> = right
                .output_schema()
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect();
            Ok(PhysicalPlan::Join {
                left: Box::new(push_projections_inner(*left, &left_full)?),
                right: Box::new(push_projections_inner(*right, &right_full)?),
                kind,
                on,
                strategy,
            })
        }
        PhysicalPlan::Union { inputs, all } => {
            let new_inputs: Vec<PhysicalPlan> = inputs
                .into_iter()
                .map(|i| push_projections_inner(i, required))
                .collect::<Result<_, _>>()?;
            Ok(PhysicalPlan::Union {
                inputs: new_inputs,
                all,
            })
        }
        leaf @ PhysicalPlan::Values { .. } => Ok(leaf),
    }
}

fn collect_column_refs(expr: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    collect_into(expr, &mut out);
    out
}

fn collect_into(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Column(c) => {
            if !out.contains(&c.name) {
                out.push(c.name.clone());
            }
        }
        Expr::Literal { .. } => {}
        Expr::Cast { expr, .. } | Expr::UnaryOp { expr, .. } | Expr::IsNull { expr, .. } => {
            collect_into(expr, out)
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_into(left, out);
            collect_into(right, out);
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_into(expr, out);
            collect_into(low, out);
            collect_into(high, out);
        }
        Expr::In { expr, list, .. } => {
            collect_into(expr, out);
            for e in list {
                collect_into(e, out);
            }
        }
        Expr::Like { expr, pattern, .. } => {
            collect_into(expr, out);
            collect_into(pattern, out);
        }
        Expr::Case {
            branches,
            otherwise,
        } => {
            for (c, t) in branches {
                collect_into(c, out);
                collect_into(t, out);
            }
            if let Some(o) = otherwise {
                collect_into(o, out);
            }
        }
        Expr::Coalesce(args) => {
            for a in args {
                collect_into(a, out);
            }
        }
        Expr::NullIf { left, right } => {
            collect_into(left, out);
            collect_into(right, out);
        }
        Expr::FuncCall { args, .. } => {
            for a in args {
                collect_into(a, out);
            }
        }
    }
}

fn aggregate_input_exprs(agg: &AggregateExpr) -> Vec<&Expr> {
    match agg {
        AggregateExpr::Count { arg, .. } => arg.iter().collect(),
        AggregateExpr::Sum { arg, .. }
        | AggregateExpr::Avg { arg, .. }
        | AggregateExpr::Min { arg }
        | AggregateExpr::Max { arg }
        | AggregateExpr::StringAgg { arg, .. } => vec![arg],
        AggregateExpr::Custom { args, .. } => args.iter().collect(),
    }
}

// Keep UnaryOp/ProximaType referenced so the compiler doesn't whine if rules
// later get reorganised; they're part of the supported surface.
#[allow(dead_code)]
fn _supported_surface(t: ProximaType, _v: ProximaValue, _c: ColumnRef, _u: UnaryOp) -> ProximaType {
    t
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_relational_algebra::AggregateExpr as AggExpr;
    use proximadb_relational_types::{ColumnInfo, ColumnRef};

    fn users_schema() -> RelationalSchema {
        RelationalSchema::new(vec![
            ColumnInfo::new("id", ProximaType::Int64, false),
            ColumnInfo::new("name", ProximaType::String, true),
            ColumnInfo::new("age", ProximaType::Int32, true),
        ])
    }

    fn orders_schema() -> RelationalSchema {
        RelationalSchema::new(vec![
            ColumnInfo::new("id", ProximaType::Int64, false),
            ColumnInfo::new("user_id", ProximaType::Int64, false),
            ColumnInfo::new("total", ProximaType::Float64, false),
        ])
    }

    fn users_scan() -> LogicalNode {
        LogicalNode::Scan {
            table: TableId::new("users"),
            table_schema: users_schema(),
            projected_columns: None,
            predicate: None,
        }
    }

    fn orders_scan() -> LogicalNode {
        LogicalNode::Scan {
            table: TableId::new("orders"),
            table_schema: orders_schema(),
            projected_columns: None,
            predicate: None,
        }
    }

    fn cap_full(pk: Vec<usize>) -> StaticCapabilities {
        StaticCapabilities {
            caps: ReaderCapabilities::full(),
            pk_columns: pk,
        }
    }

    fn cap_no_pushdown() -> StaticCapabilities {
        StaticCapabilities {
            caps: ReaderCapabilities::none(),
            pk_columns: Vec::new(),
        }
    }

    // --- Constant folding ---------------------------------------------

    #[test]
    fn fold_literal_arithmetic() {
        // 2 + 3 → 5
        let e = Expr::bin(
            BinaryOp::Plus,
            Expr::literal(ProximaValue::Int64(2)),
            Expr::literal(ProximaValue::Int64(3)),
        );
        let folded = fold_expr(e);
        assert!(matches!(
            folded,
            Expr::Literal {
                value: ProximaValue::Int64(5),
                ..
            }
        ));
    }

    #[test]
    fn fold_does_not_fold_column_refs() {
        // col + 1 stays as binary op (col is non-literal)
        let col = ColumnRef {
            name: "x".into(),
            ordinal: 0,
            ty: ProximaType::Int64,
            nullable: false,
        };
        let e = Expr::bin(BinaryOp::Plus, Expr::column(col), Expr::literal(ProximaValue::Int64(1)));
        let folded = fold_expr(e);
        assert!(matches!(folded, Expr::BinaryOp { .. }));
    }

    #[test]
    fn fold_unary_neg() {
        let e = Expr::unary(UnaryOp::Neg, Expr::literal(ProximaValue::Int64(7)));
        let folded = fold_expr(e);
        assert!(matches!(
            folded,
            Expr::Literal {
                value: ProximaValue::Int64(-7),
                ..
            }
        ));
    }

    // --- Filter merge -------------------------------------------------

    #[test]
    fn merge_adjacent_filters_into_and() {
        // Filter(Filter(scan, p1), p2) → Filter(scan, p1 AND p2)
        let id = users_schema().resolve_column("id").unwrap();
        let p1 = Expr::bin(
            BinaryOp::Gt,
            Expr::column(id.clone()),
            Expr::literal(ProximaValue::Int64(5)),
        );
        let p2 = Expr::bin(
            BinaryOp::Lt,
            Expr::column(id),
            Expr::literal(ProximaValue::Int64(100)),
        );
        let logical = LogicalNode::Filter {
            input: Box::new(LogicalNode::Filter {
                input: Box::new(users_scan()),
                predicate: p1,
            }),
            predicate: p2,
        };
        let merged = merge_filters(logical);
        match merged {
            LogicalNode::Filter {
                predicate:
                    Expr::BinaryOp {
                        op: BinaryOp::And, ..
                    },
                ..
            } => {}
            other => panic!("expected Filter(AND(...)), got {other:?}"),
        }
    }

    // --- Lowering -----------------------------------------------------

    #[test]
    fn lower_scan_default_is_full_scan() {
        let physical = lower_to_physical(users_scan());
        match physical {
            PhysicalPlan::Scan { access, .. } => assert_eq!(access, ScanAccess::FullScan),
            _ => panic!(),
        }
    }

    #[test]
    fn lower_join_picks_hash_for_equi_join() {
        let users = users_scan();
        let orders = orders_scan();
        let combined = {
            let mut c = users_schema().columns;
            c.extend(orders_schema().columns);
            RelationalSchema::new(c)
        };
        let u_id = combined.resolve_column("id").unwrap();
        // orders.user_id is at ordinal 4 in the combined schema.
        let o_uid = ColumnRef {
            name: "user_id".into(),
            ordinal: 4,
            ty: ProximaType::Int64,
            nullable: false,
        };
        let logical = LogicalNode::Join {
            left: Box::new(users),
            right: Box::new(orders),
            kind: JoinKind::Inner,
            on: Some(Expr::bin(BinaryOp::Eq, Expr::column(u_id), Expr::column(o_uid))),
            strategy: JoinStrategy::Auto,
        };
        let physical = lower_to_physical(logical);
        match physical {
            PhysicalPlan::Join { strategy, .. } => {
                assert!(matches!(strategy, JoinStrategy::Hash { .. }));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn lower_join_picks_nested_loop_for_non_equi() {
        let logical = LogicalNode::Join {
            left: Box::new(users_scan()),
            right: Box::new(orders_scan()),
            kind: JoinKind::Inner,
            on: Some(Expr::literal(ProximaValue::Boolean(true))), // not equi
            strategy: JoinStrategy::Auto,
        };
        let physical = lower_to_physical(logical);
        match physical {
            PhysicalPlan::Join { strategy, .. } => {
                assert_eq!(strategy, JoinStrategy::NestedLoop);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn lower_aggregate_no_group_by_is_streaming() {
        let logical = LogicalNode::Aggregate {
            input: Box::new(users_scan()),
            group_by: vec![],
            aggregates: vec![NamedAggregate::new(
                "n",
                AggExpr::Count {
                    arg: None,
                    distinct: false,
                },
            )],
            having: None,
        };
        let physical = lower_to_physical(logical);
        match physical {
            PhysicalPlan::Aggregate { strategy, .. } => {
                assert_eq!(strategy, AggregateStrategy::Streaming);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn lower_aggregate_with_group_by_is_hash() {
        let age = users_schema().resolve_column("age").unwrap();
        let logical = LogicalNode::Aggregate {
            input: Box::new(users_scan()),
            group_by: vec![NamedExpr::new("age", Expr::column(age))],
            aggregates: vec![NamedAggregate::new(
                "n",
                AggExpr::Count {
                    arg: None,
                    distinct: false,
                },
            )],
            having: None,
        };
        let physical = lower_to_physical(logical);
        match physical {
            PhysicalPlan::Aggregate { strategy, .. } => {
                assert_eq!(strategy, AggregateStrategy::Hash);
            }
            _ => panic!(),
        }
    }

    // --- Predicate pushdown ------------------------------------------

    #[test]
    fn push_predicate_into_scan_when_capable() {
        let id = users_schema().resolve_column("id").unwrap();
        let pred = Expr::bin(
            BinaryOp::Gt,
            Expr::column(id),
            Expr::literal(ProximaValue::Int64(5)),
        );
        let physical = PhysicalPlan::Filter {
            input: Box::new(lower_to_physical(users_scan())),
            predicate: pred.clone(),
        };
        let result = push_predicates(physical, &cap_full(Vec::new()));
        // Filter is gone; predicate moved to Scan.
        match result {
            PhysicalPlan::Scan { predicate, .. } => {
                assert_eq!(predicate, Some(pred));
            }
            other => panic!("expected Scan, got {other:?}"),
        }
    }

    #[test]
    fn push_predicate_preserved_when_adapter_lacks_capability() {
        let id = users_schema().resolve_column("id").unwrap();
        let pred = Expr::bin(
            BinaryOp::Gt,
            Expr::column(id),
            Expr::literal(ProximaValue::Int64(5)),
        );
        let physical = PhysicalPlan::Filter {
            input: Box::new(lower_to_physical(users_scan())),
            predicate: pred,
        };
        let result = push_predicates(physical, &cap_no_pushdown());
        // Filter stays above the Scan because adapter doesn't push.
        assert!(matches!(result, PhysicalPlan::Filter { .. }));
    }

    // --- PK lookup rewrite -------------------------------------------

    #[test]
    fn push_predicate_pk_eq_literal_becomes_pk_lookup() {
        let id = users_schema().resolve_column("id").unwrap();
        let pred = Expr::bin(
            BinaryOp::Eq,
            Expr::column(id),
            Expr::literal(ProximaValue::Int64(42)),
        );
        let physical = PhysicalPlan::Filter {
            input: Box::new(lower_to_physical(users_scan())),
            predicate: pred,
        };
        let result = push_predicates(physical, &cap_full(vec![0])); // id is PK
        match result {
            PhysicalPlan::Scan {
                access: ScanAccess::PkLookup { key },
                predicate,
                ..
            } => {
                assert_eq!(key.len(), 1);
                // Predicate dropped because PK lookup is exact.
                assert_eq!(predicate, None);
            }
            other => panic!("expected PkLookup scan, got {other:?}"),
        }
    }

    #[test]
    fn push_predicate_non_pk_eq_does_not_become_pk_lookup() {
        let age = users_schema().resolve_column("age").unwrap();
        let pred = Expr::bin(
            BinaryOp::Eq,
            Expr::column(age),
            Expr::literal(ProximaValue::Int32(30)),
        );
        let physical = PhysicalPlan::Filter {
            input: Box::new(lower_to_physical(users_scan())),
            predicate: pred,
        };
        let result = push_predicates(physical, &cap_full(vec![0])); // id is PK
        match result {
            PhysicalPlan::Scan {
                access: ScanAccess::FullScan,
                predicate: Some(_),
                ..
            } => {}
            other => panic!("expected FullScan with predicate, got {other:?}"),
        }
    }

    #[test]
    fn push_predicate_pk_skipped_when_adapter_lacks_pk_lookup() {
        let id = users_schema().resolve_column("id").unwrap();
        let pred = Expr::bin(
            BinaryOp::Eq,
            Expr::column(id),
            Expr::literal(ProximaValue::Int64(42)),
        );
        let physical = PhysicalPlan::Filter {
            input: Box::new(lower_to_physical(users_scan())),
            predicate: pred,
        };
        // Caps: push_predicate but NOT pk_lookup.
        let caps = StaticCapabilities {
            caps: ReaderCapabilities::full().with_pk_lookup(false),
            pk_columns: vec![0],
        };
        let result = push_predicates(physical, &caps);
        // Falls back to predicate pushdown (FullScan + predicate).
        match result {
            PhysicalPlan::Scan {
                access: ScanAccess::FullScan,
                predicate: Some(_),
                ..
            } => {}
            other => panic!("expected FullScan, got {other:?}"),
        }
    }

    // --- Projection pushdown -----------------------------------------

    #[test]
    fn push_projection_narrows_scan_to_referenced_columns() {
        // SELECT id FROM users
        let id = users_schema().resolve_column("id").unwrap();
        let physical = PhysicalPlan::Project {
            input: Box::new(lower_to_physical(users_scan())),
            outputs: vec![NamedExpr::new("id", Expr::column(id))],
        };
        let result = push_projections(physical).unwrap();
        match result {
            PhysicalPlan::Project { input, outputs } => {
                // After rebind, the Project's column ref points
                // at ordinal 0 (id is the only column in the
                // narrowed scan).
                match &outputs[0].expr {
                    Expr::Column(c) => {
                        assert_eq!(c.name, "id");
                        assert_eq!(c.ordinal, 0);
                    }
                    other => panic!("expected Column, got {other:?}"),
                }
                match *input {
                    PhysicalPlan::Scan {
                        projection,
                        output_schema,
                        ..
                    } => {
                        assert_eq!(projection, Some(vec!["id".to_string()]));
                        assert_eq!(output_schema.columns[0].name, "id");
                    }
                    other => panic!("expected Scan inside Project, got {other:?}"),
                }
            }
            other => panic!("expected Project, got {other:?}"),
        }
    }

    #[test]
    fn push_projection_includes_predicate_columns() {
        // SELECT id FROM users WHERE age > 25
        let id = users_schema().resolve_column("id").unwrap();
        let age = users_schema().resolve_column("age").unwrap();
        let pred = Expr::bin(
            BinaryOp::Gt,
            Expr::column(age),
            Expr::literal(ProximaValue::Int32(25)),
        );
        let scan = lower_to_physical(LogicalNode::Scan {
            table: TableId::new("users"),
            table_schema: users_schema(),
            projected_columns: None,
            predicate: Some(pred),
        });
        let physical = PhysicalPlan::Project {
            input: Box::new(scan),
            outputs: vec![NamedExpr::new("id", Expr::column(id))],
        };
        let result = push_projections(physical).unwrap();
        match result {
            PhysicalPlan::Project { input, outputs } => {
                // The Project's `id` ref should now point at the
                // narrowed schema's index of `id` (0 if id is
                // first; could be 1 if predicate column comes
                // first — depends on order).
                let col = match &outputs[0].expr {
                    Expr::Column(c) => c,
                    other => panic!("expected Column, got {other:?}"),
                };
                assert_eq!(col.name, "id");
                match *input {
                    PhysicalPlan::Scan {
                        projection,
                        predicate,
                        output_schema,
                        ..
                    } => {
                        let p = projection.unwrap();
                        assert!(p.contains(&"id".to_string()));
                        // age must also be projected because the
                        // pushed-down predicate references it.
                        assert!(p.contains(&"age".to_string()));
                        // Rebind: the predicate's `age` column
                        // now uses the narrowed schema's ordinal.
                        let age_idx = output_schema
                            .columns
                            .iter()
                            .position(|c| c.name == "age")
                            .unwrap();
                        match predicate.as_ref().unwrap() {
                            Expr::BinaryOp { left, .. } => match left.as_ref() {
                                Expr::Column(c) => {
                                    assert_eq!(c.ordinal, age_idx);
                                }
                                other => panic!(
                                    "expected Column on left, got {other:?}"
                                ),
                            },
                            other => panic!("expected BinaryOp, got {other:?}"),
                        }
                        // And the Project's `id` ordinal lines up.
                        let id_idx = output_schema
                            .columns
                            .iter()
                            .position(|c| c.name == "id")
                            .unwrap();
                        assert_eq!(col.ordinal, id_idx);
                    }
                    other => panic!("expected Scan, got {other:?}"),
                }
            }
            _ => panic!(),
        }
    }

    // --- End-to-end --------------------------------------------------

    #[test]
    fn full_pipeline_realistic_query() {
        // SELECT name FROM users WHERE id = 42 AND age > 25
        let id = users_schema().resolve_column("id").unwrap();
        let age = users_schema().resolve_column("age").unwrap();
        let name = users_schema().resolve_column("name").unwrap();
        let pred = Expr::bin(
            BinaryOp::And,
            Expr::bin(
                BinaryOp::Eq,
                Expr::column(id),
                Expr::literal(ProximaValue::Int64(42)),
            ),
            Expr::bin(
                BinaryOp::Gt,
                Expr::column(age),
                Expr::literal(ProximaValue::Int32(25)),
            ),
        );
        let logical = LogicalNode::Project {
            input: Box::new(LogicalNode::Filter {
                input: Box::new(users_scan()),
                predicate: pred,
            }),
            outputs: vec![NamedExpr::new("name", Expr::column(name))],
        };
        let planner = Planner::new(cap_full(vec![0]));
        let physical = planner.plan(logical).unwrap();
        // Expected: Project(Scan{...}) with the predicate pushed
        // into the Scan AND the projection narrowed.
        // PK lookup requires the FULL conjunction to be a PK
        // equality — but `id=42 AND age>25` has an extra
        // non-PK conjunct, so PK lookup is skipped and we fall
        // back to FullScan + predicate.
        // All 3 table columns are needed (`name` for projection,
        // `id` + `age` for the pushed-down predicate), so the
        // planner correctly leaves `projection: None` — narrowing
        // is impossible when the requirement set equals the full
        // table.
        match physical {
            PhysicalPlan::Project { input, .. } => match *input {
                PhysicalPlan::Scan {
                    access: ScanAccess::FullScan,
                    predicate: Some(_),
                    projection: None,
                    output_schema,
                    ..
                } => {
                    assert_eq!(output_schema.len(), 3);
                }
                other => panic!("expected Scan with pushed predicate, got {other:?}"),
            },
            other => panic!("expected Project, got {other:?}"),
        }
    }

    #[test]
    fn full_pipeline_pk_lookup_only() {
        // SELECT name FROM users WHERE id = 42
        let id = users_schema().resolve_column("id").unwrap();
        let name = users_schema().resolve_column("name").unwrap();
        let pred = Expr::bin(
            BinaryOp::Eq,
            Expr::column(id),
            Expr::literal(ProximaValue::Int64(42)),
        );
        let logical = LogicalNode::Project {
            input: Box::new(LogicalNode::Filter {
                input: Box::new(users_scan()),
                predicate: pred,
            }),
            outputs: vec![NamedExpr::new("name", Expr::column(name))],
        };
        let planner = Planner::new(cap_full(vec![0]));
        let physical = planner.plan(logical).unwrap();
        match physical {
            PhysicalPlan::Project { input, .. } => match *input {
                PhysicalPlan::Scan {
                    access: ScanAccess::PkLookup { key },
                    ..
                } => {
                    assert_eq!(key.len(), 1);
                }
                other => panic!("expected PkLookup, got {other:?}"),
            },
            _ => panic!(),
        }
    }
}

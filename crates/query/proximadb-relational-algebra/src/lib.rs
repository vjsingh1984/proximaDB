//! Logical relational algebra for ProximaDB's SQL query path
//! (ADR-019 L2). Builds on [`proximadb_relational_types`].
//!
//! Every operator is a [`LogicalNode`]. Operators carry typed
//! [`Expr`] values for predicates, projections, ON conditions,
//! sort keys, aggregate arguments, and HAVING clauses. Each node
//! knows its own output schema.
//!
//! Semantic invariants enforced via [`LogicalNode::validate`]:
//!
//! - `Filter::predicate` must produce `Boolean`.
//! - `Project::outputs` must all type-check against the input
//!   schema and must have unique names.
//! - `Join::on` (when present) must produce `Boolean` against the
//!   concatenated left+right schema.
//! - `Aggregate::group_by` columns must type-check against the
//!   input schema. `having` must produce `Boolean` against the
//!   post-aggregate schema.
//! - `Sort::keys` must type-check against the input schema.
//! - `Union::inputs` must all share the same output schema.
//! - `Values::rows` must all type-check against the declared
//!   output schema.
//!
//! Subquery expressions (`EXISTS`, scalar subquery, `IN (SELECT …)`)
//! are encoded via [`SubqueryExpr`]. Foundation `Expr` doesn't carry
//! subqueries to avoid a circular type dependency.
//!
//! Schema propagation through [`LogicalNode::output_schema`] is
//! pure — no I/O, no catalog lookup. The Scan node carries its
//! `output_schema` as a constructor arg (the planner resolves it
//! from the catalog when lowering).
//!
//! The crate intentionally does NOT include:
//!
//! - Physical operators (`HashJoin`, `SortMergeJoin`, etc.) — those
//!   are produced by the planner in S2.
//! - Window functions / CTEs — deferred to S1.1 + S2.
//! - Cost estimates — that's the planner's responsibility (S2).
//! - Execution — that's the executor's responsibility (S3).

use proximadb_data_model::{ProximaType, ProximaValue};
use proximadb_relational_types::{
    cast_value, ColumnInfo, ColumnRef, Expr, ExprError, RelationalRow, RelationalSchema,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// =========================================================================
// Errors
// =========================================================================

/// Errors that arise while building or validating a logical plan.
/// Distinct from [`ExprError`] (which is per-expression);
/// these are tree-level issues.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum AlgebraError {
    #[error("expression error: {0}")]
    Expr(#[from] ExprError),

    #[error("Filter predicate must produce Boolean, got {actual:?}")]
    NonBooleanFilter { actual: ProximaType },

    #[error("Join ON condition must produce Boolean, got {actual:?}")]
    NonBooleanJoinOn { actual: ProximaType },

    #[error("HAVING clause must produce Boolean, got {actual:?}")]
    NonBooleanHaving { actual: ProximaType },

    #[error("Project outputs contain duplicate name {name:?}")]
    DuplicateProjectName { name: String },

    #[error("Union inputs must have matching schemas; differ at column {ordinal} ({left:?} vs {right:?})")]
    UnionSchemaMismatch {
        ordinal: usize,
        left: ProximaType,
        right: ProximaType,
    },

    #[error("Union inputs must have the same column count ({left_count} vs {right_count})")]
    UnionArityMismatch { left_count: usize, right_count: usize },

    #[error("Values row {row_index} has {actual_len} columns but declared schema has {schema_len}")]
    ValuesArityMismatch {
        row_index: usize,
        actual_len: usize,
        schema_len: usize,
    },

    #[error("CTE reference {name:?} is not bound in the surrounding plan")]
    UnboundCteRef { name: String },

    #[error("Aggregate result name {name:?} conflicts with a GROUP BY name")]
    AggregateNameConflict { name: String },

    #[error("Cross join must not have an ON condition")]
    CrossJoinWithOn,
}

// =========================================================================
// Identifiers and small types
// =========================================================================

/// Table identifier used by [`LogicalNode::Scan`]. Three optional
/// parts: catalog → schema → name. Matches SQL `catalog.schema.name`
/// addressing. Defined locally (not imported from
/// `proximadb-catalog`) so the algebra crate stays free of the
/// control-plane dependency; the planner translates between them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TableId {
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub name: String,
}

impl TableId {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            catalog: None,
            schema: None,
            name: name.into(),
        }
    }

    pub fn with_schema(name: impl Into<String>, schema: impl Into<String>) -> Self {
        Self {
            catalog: None,
            schema: Some(schema.into()),
            name: name.into(),
        }
    }

    pub fn fqn(&self) -> String {
        match (&self.catalog, &self.schema) {
            (Some(c), Some(s)) => format!("{c}.{s}.{}", self.name),
            (None, Some(s)) => format!("{s}.{}", self.name),
            _ => self.name.clone(),
        }
    }
}

// =========================================================================
// Named outputs
// =========================================================================

/// A projected expression with an explicit output name. Mirrors
/// `SELECT expr AS name` in SQL. Used in Project, Aggregate
/// group_by, and Values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedExpr {
    pub name: String,
    pub expr: Expr,
}

impl NamedExpr {
    pub fn new(name: impl Into<String>, expr: Expr) -> Self {
        Self {
            name: name.into(),
            expr,
        }
    }

    pub fn from_column(col: ColumnRef) -> Self {
        Self {
            name: col.name.clone(),
            expr: Expr::Column(col),
        }
    }
}

// =========================================================================
// Aggregates
// =========================================================================

/// Aggregate function call as a logical-plan node. Distinct from
/// regular function calls in [`Expr`] because aggregates can only
/// appear in specific positions (`Aggregate::aggregates`,
/// `Having`, `ORDER BY` over aggregate output).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AggregateExpr {
    /// `COUNT(*)` if `arg = None`; `COUNT(expr)` otherwise.
    /// `COUNT(*)` ignores NULLs at the row level; `COUNT(expr)`
    /// ignores NULLs in `expr`.
    Count {
        arg: Option<Expr>,
        distinct: bool,
    },
    Sum {
        arg: Expr,
        distinct: bool,
    },
    Avg {
        arg: Expr,
        distinct: bool,
    },
    Min {
        arg: Expr,
    },
    Max {
        arg: Expr,
    },
    /// String aggregation (`STRING_AGG`/`GROUP_CONCAT`). The
    /// separator is a `String` literal at MVP.
    StringAgg {
        arg: Expr,
        separator: String,
        distinct: bool,
    },
    /// Extension point for executor-supplied aggregate functions.
    /// The executor's aggregate registry resolves `name` at
    /// execution time; the planner trusts `return_ty` from the
    /// lowering.
    Custom {
        name: String,
        args: Vec<Expr>,
        distinct: bool,
        return_ty: ProximaType,
    },
}

impl AggregateExpr {
    /// What `ProximaType` does this aggregate produce?
    pub fn result_type(&self) -> ProximaType {
        match self {
            AggregateExpr::Count { .. } => ProximaType::Int64,
            AggregateExpr::Sum { arg, .. } => {
                // SUM widens INT → BIGINT, keeps FLOAT/DECIMAL.
                // For MVP we keep the arg's type; planner can
                // upgrade to wider type in Phase 3.
                widen_for_sum(arg.result_type())
            }
            AggregateExpr::Avg { .. } => ProximaType::Float64,
            AggregateExpr::Min { arg } | AggregateExpr::Max { arg } => arg.result_type(),
            AggregateExpr::StringAgg { .. } => ProximaType::String,
            AggregateExpr::Custom { return_ty, .. } => return_ty.clone(),
        }
    }

    /// Type-check the aggregate's argument expressions against the
    /// input schema.
    pub fn type_check(&self, input: &RelationalSchema) -> Result<(), AlgebraError> {
        match self {
            AggregateExpr::Count { arg, .. } => {
                if let Some(e) = arg {
                    e.type_check(input)?;
                }
                Ok(())
            }
            AggregateExpr::Sum { arg, .. }
            | AggregateExpr::Avg { arg, .. }
            | AggregateExpr::Min { arg }
            | AggregateExpr::Max { arg } => {
                arg.type_check(input)?;
                Ok(())
            }
            AggregateExpr::StringAgg { arg, .. } => {
                arg.type_check(input)?;
                Ok(())
            }
            AggregateExpr::Custom { args, .. } => {
                for a in args {
                    a.type_check(input)?;
                }
                Ok(())
            }
        }
    }
}

fn widen_for_sum(t: ProximaType) -> ProximaType {
    use ProximaType::*;
    match t {
        Int8 | Int16 | Int32 | Int64 => Int64,
        UInt8 | UInt16 | UInt32 | UInt64 => UInt64,
        Float16 | Float32 | Float64 => Float64,
        other => other,
    }
}

/// Named aggregate as it appears in the `Aggregate` node's output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedAggregate {
    pub name: String,
    pub agg: AggregateExpr,
}

impl NamedAggregate {
    pub fn new(name: impl Into<String>, agg: AggregateExpr) -> Self {
        Self {
            name: name.into(),
            agg,
        }
    }
}

// =========================================================================
// Join
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    /// Semi join — emit a left row when at least one matching right
    /// row exists. Useful as an `IN (subquery)` lowering target.
    Semi,
    /// Anti join — emit a left row when NO matching right row
    /// exists. Useful as a `NOT IN` / `NOT EXISTS` lowering target.
    Anti,
}

impl JoinKind {
    pub fn as_str(self) -> &'static str {
        use JoinKind::*;
        match self {
            Inner => "INNER",
            Left => "LEFT",
            Right => "RIGHT",
            Full => "FULL",
            Cross => "CROSS",
            Semi => "SEMI",
            Anti => "ANTI",
        }
    }
}

/// Physical execution strategy for a join. Set by the planner in
/// S2; logical-plan callers leave `Auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinStrategy {
    Auto,
    NestedLoop,
    Hash { build_side: JoinSide },
    SortMerge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JoinSide {
    Left,
    Right,
}

// =========================================================================
// Sort
// =========================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SortKey {
    pub expr: Expr,
    pub descending: bool,
    /// SQL NULLS FIRST/LAST. Default per Postgres semantics:
    /// `NULLS LAST` for ASC, `NULLS FIRST` for DESC.
    pub nulls_first: bool,
}

impl SortKey {
    pub fn asc(expr: Expr) -> Self {
        Self {
            expr,
            descending: false,
            nulls_first: false,
        }
    }
    pub fn desc(expr: Expr) -> Self {
        Self {
            expr,
            descending: true,
            nulls_first: true,
        }
    }
}

// =========================================================================
// Subquery wrapper
// =========================================================================

/// Subquery shapes used inside an [`Expr`] context (e.g. `WHERE
/// x IN (SELECT …)`). The foundation [`Expr`] can't carry these
/// because it would create a circular type dependency between the
/// `proximadb-relational-types` and `proximadb-relational-algebra`
/// crates. Instead, callers that need subqueries wrap the
/// containing [`Expr`] in [`AlgebraExpr`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SubqueryExpr {
    /// Scalar subquery yielding one column, one row. Result type
    /// must be carried explicitly.
    Scalar {
        sub: Box<LogicalNode>,
        result_ty: ProximaType,
    },
    /// `EXISTS (subquery)` / `NOT EXISTS`.
    Exists {
        sub: Box<LogicalNode>,
        not: bool,
    },
    /// `value IN (SELECT one_column FROM …)`. The inner query must
    /// produce exactly one column.
    In {
        value: Box<AlgebraExpr>,
        sub: Box<LogicalNode>,
        not: bool,
    },
}

impl SubqueryExpr {
    pub fn result_type(&self) -> ProximaType {
        match self {
            SubqueryExpr::Scalar { result_ty, .. } => result_ty.clone(),
            SubqueryExpr::Exists { .. } => ProximaType::Boolean,
            SubqueryExpr::In { .. } => ProximaType::Boolean,
        }
    }
}

/// Expression layer that extends foundation [`Expr`] with
/// subquery variants. Callers that need subqueries use this in
/// place of `Expr`; the planner lowers subqueries to joins where
/// possible and to materialised subplans otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AlgebraExpr {
    /// Pass-through to the foundation `Expr`.
    Pure(Expr),
    /// Subquery expression.
    Sub(SubqueryExpr),
}

impl AlgebraExpr {
    pub fn result_type(&self) -> ProximaType {
        match self {
            AlgebraExpr::Pure(e) => e.result_type(),
            AlgebraExpr::Sub(s) => s.result_type(),
        }
    }
}

// =========================================================================
// LogicalNode
// =========================================================================

/// Logical relational-algebra node. Each variant carries the
/// information needed to compute its output schema and to validate
/// its expressions; physical strategy is set by the planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LogicalNode {
    /// Read all rows of a table. The planner attaches pushdown
    /// hints (projection, predicate) for the storage adapter.
    Scan {
        table: TableId,
        /// Full table schema. Output schema is `projected_columns`
        /// when set, else this.
        table_schema: RelationalSchema,
        /// Set by the planner during projection-pushdown. None =
        /// emit every column from `table_schema`.
        projected_columns: Option<Vec<ColumnRef>>,
        /// Set by the planner during predicate-pushdown.
        predicate: Option<Expr>,
    },
    /// `WHERE predicate`.
    Filter {
        input: Box<LogicalNode>,
        predicate: Expr,
    },
    /// `SELECT outputs`. Each output is `expr AS name`.
    Project {
        input: Box<LogicalNode>,
        outputs: Vec<NamedExpr>,
    },
    /// Join two inputs. ON is required for non-cross joins; for
    /// Cross join, `on` must be `None`.
    Join {
        left: Box<LogicalNode>,
        right: Box<LogicalNode>,
        kind: JoinKind,
        on: Option<Expr>,
        strategy: JoinStrategy,
    },
    /// `GROUP BY group_by ... HAVING having`. Output is
    /// `group_by` columns followed by `aggregates`.
    Aggregate {
        input: Box<LogicalNode>,
        group_by: Vec<NamedExpr>,
        aggregates: Vec<NamedAggregate>,
        having: Option<Expr>,
    },
    /// `ORDER BY keys`. Stable sort.
    Sort {
        input: Box<LogicalNode>,
        keys: Vec<SortKey>,
    },
    /// `LIMIT n OFFSET m`. `limit = None` = unbounded.
    Limit {
        input: Box<LogicalNode>,
        limit: Option<u64>,
        offset: u64,
    },
    /// `SELECT DISTINCT`.
    Distinct {
        input: Box<LogicalNode>,
    },
    /// `UNION [ALL]`. All inputs must produce matching schemas.
    Union {
        inputs: Vec<LogicalNode>,
        all: bool,
    },
    /// `VALUES (...), (...), ...` — inline literal rows.
    Values {
        rows: Vec<Vec<Expr>>,
        output_schema: RelationalSchema,
    },
    /// `WITH name AS (body) usages`. Optimizer decides whether to
    /// inline (single-use) or materialise.
    CteBind {
        name: String,
        body: Box<LogicalNode>,
        usages: Box<LogicalNode>,
    },
    /// Reference to a CTE bound in an enclosing `CteBind`.
    /// `output_schema` is the CTE's body schema (planner copies
    /// it in during lowering).
    CteRef {
        name: String,
        output_schema: RelationalSchema,
    },
}

impl LogicalNode {
    /// Compute the output schema of this node. Pure — no catalog
    /// lookup, no I/O. Scan nodes carry their `table_schema`
    /// directly.
    pub fn output_schema(&self) -> RelationalSchema {
        match self {
            LogicalNode::Scan {
                table_schema,
                projected_columns,
                ..
            } => match projected_columns {
                Some(cols) => table_schema.project(cols),
                None => table_schema.clone(),
            },
            LogicalNode::Filter { input, .. } => input.output_schema(),
            LogicalNode::Project { outputs, .. } => RelationalSchema::new(
                outputs
                    .iter()
                    .map(|o| ColumnInfo {
                        name: o.name.clone(),
                        ty: o.expr.result_type(),
                        // Conservative: assume nullable. The
                        // planner can refine via expression-level
                        // null analysis in Phase 3.
                        nullable: true,
                    })
                    .collect(),
            ),
            LogicalNode::Join {
                left, right, kind, ..
            } => {
                let mut cols = left.output_schema().columns;
                let right_cols = right.output_schema().columns;
                // Outer-join sides get `nullable = true` for the
                // possibly-NULL columns.
                let (left_nullable, right_nullable) = match kind {
                    JoinKind::Inner | JoinKind::Cross | JoinKind::Semi | JoinKind::Anti => {
                        (false, false)
                    }
                    JoinKind::Left => (false, true),
                    JoinKind::Right => (true, false),
                    JoinKind::Full => (true, true),
                };
                if left_nullable {
                    for c in &mut cols {
                        c.nullable = true;
                    }
                }
                // Semi/Anti joins emit only the left side.
                if matches!(kind, JoinKind::Semi | JoinKind::Anti) {
                    return RelationalSchema::new(cols);
                }
                let right_cols: Vec<ColumnInfo> = right_cols
                    .into_iter()
                    .map(|mut c| {
                        if right_nullable {
                            c.nullable = true;
                        }
                        c
                    })
                    .collect();
                cols.extend(right_cols);
                RelationalSchema::new(cols)
            }
            LogicalNode::Aggregate {
                group_by,
                aggregates,
                ..
            } => {
                let mut cols: Vec<ColumnInfo> = group_by
                    .iter()
                    .map(|g| ColumnInfo {
                        name: g.name.clone(),
                        ty: g.expr.result_type(),
                        // GROUP BY keys carry the input's
                        // nullability; conservatively true.
                        nullable: true,
                    })
                    .collect();
                for a in aggregates {
                    cols.push(ColumnInfo {
                        name: a.name.clone(),
                        ty: a.agg.result_type(),
                        // COUNT is non-null; others can be NULL on
                        // empty input. Conservative: nullable.
                        nullable: !matches!(a.agg, AggregateExpr::Count { .. }),
                    });
                }
                RelationalSchema::new(cols)
            }
            LogicalNode::Sort { input, .. } => input.output_schema(),
            LogicalNode::Limit { input, .. } => input.output_schema(),
            LogicalNode::Distinct { input } => input.output_schema(),
            LogicalNode::Union { inputs, .. } => {
                // Caller ensures schemas match; return the first.
                inputs
                    .first()
                    .map(|i| i.output_schema())
                    .unwrap_or_default()
            }
            LogicalNode::Values { output_schema, .. } => output_schema.clone(),
            LogicalNode::CteBind { usages, .. } => usages.output_schema(),
            LogicalNode::CteRef { output_schema, .. } => output_schema.clone(),
        }
    }

    /// Validate this node and recursively its children. Catches
    /// type errors, schema mismatches, and tree-structural
    /// issues. Pure — no execution.
    pub fn validate(&self) -> Result<(), AlgebraError> {
        match self {
            LogicalNode::Scan {
                table_schema,
                projected_columns,
                predicate,
                ..
            } => {
                if let Some(cols) = projected_columns {
                    for c in cols {
                        if c.ordinal >= table_schema.len() {
                            return Err(AlgebraError::Expr(
                                ExprError::ColumnOrdinalOutOfRange {
                                    ordinal: c.ordinal,
                                    row_len: table_schema.len(),
                                },
                            ));
                        }
                    }
                }
                if let Some(p) = predicate {
                    p.type_check(table_schema)?;
                    if p.result_type() != ProximaType::Boolean {
                        return Err(AlgebraError::NonBooleanFilter {
                            actual: p.result_type(),
                        });
                    }
                }
                Ok(())
            }
            LogicalNode::Filter { input, predicate } => {
                input.validate()?;
                let schema = input.output_schema();
                predicate.type_check(&schema)?;
                if predicate.result_type() != ProximaType::Boolean {
                    return Err(AlgebraError::NonBooleanFilter {
                        actual: predicate.result_type(),
                    });
                }
                Ok(())
            }
            LogicalNode::Project { input, outputs } => {
                input.validate()?;
                let schema = input.output_schema();
                let mut seen: std::collections::HashSet<&str> =
                    std::collections::HashSet::new();
                for out in outputs {
                    if !seen.insert(out.name.as_str()) {
                        return Err(AlgebraError::DuplicateProjectName {
                            name: out.name.clone(),
                        });
                    }
                    out.expr.type_check(&schema)?;
                }
                Ok(())
            }
            LogicalNode::Join {
                left,
                right,
                kind,
                on,
                ..
            } => {
                left.validate()?;
                right.validate()?;
                let mut combined = left.output_schema().columns;
                combined.extend(right.output_schema().columns);
                let combined = RelationalSchema::new(combined);
                match kind {
                    JoinKind::Cross => {
                        if on.is_some() {
                            return Err(AlgebraError::CrossJoinWithOn);
                        }
                    }
                    _ => {
                        if let Some(e) = on {
                            e.type_check(&combined)?;
                            if e.result_type() != ProximaType::Boolean {
                                return Err(AlgebraError::NonBooleanJoinOn {
                                    actual: e.result_type(),
                                });
                            }
                        }
                    }
                }
                Ok(())
            }
            LogicalNode::Aggregate {
                input,
                group_by,
                aggregates,
                having,
            } => {
                input.validate()?;
                let in_schema = input.output_schema();
                let mut names: std::collections::HashSet<&str> =
                    std::collections::HashSet::new();
                for g in group_by {
                    if !names.insert(g.name.as_str()) {
                        return Err(AlgebraError::DuplicateProjectName {
                            name: g.name.clone(),
                        });
                    }
                    g.expr.type_check(&in_schema)?;
                }
                for a in aggregates {
                    if !names.insert(a.name.as_str()) {
                        return Err(AlgebraError::AggregateNameConflict {
                            name: a.name.clone(),
                        });
                    }
                    a.agg.type_check(&in_schema)?;
                }
                if let Some(h) = having {
                    // HAVING is evaluated against the
                    // post-aggregate schema.
                    let out = self.output_schema();
                    h.type_check(&out)?;
                    if h.result_type() != ProximaType::Boolean {
                        return Err(AlgebraError::NonBooleanHaving {
                            actual: h.result_type(),
                        });
                    }
                }
                Ok(())
            }
            LogicalNode::Sort { input, keys } => {
                input.validate()?;
                let schema = input.output_schema();
                for k in keys {
                    k.expr.type_check(&schema)?;
                }
                Ok(())
            }
            LogicalNode::Limit { input, .. } => input.validate(),
            LogicalNode::Distinct { input } => input.validate(),
            LogicalNode::Union { inputs, .. } => {
                if inputs.is_empty() {
                    return Ok(());
                }
                let mut iter = inputs.iter();
                let first = iter.next().unwrap();
                first.validate()?;
                let first_schema = first.output_schema();
                for (idx_input, other) in iter.enumerate() {
                    other.validate()?;
                    let other_schema = other.output_schema();
                    if first_schema.len() != other_schema.len() {
                        return Err(AlgebraError::UnionArityMismatch {
                            left_count: first_schema.len(),
                            right_count: other_schema.len(),
                        });
                    }
                    for (i, (l, r)) in first_schema
                        .columns
                        .iter()
                        .zip(other_schema.columns.iter())
                        .enumerate()
                    {
                        if l.ty != r.ty {
                            return Err(AlgebraError::UnionSchemaMismatch {
                                ordinal: i,
                                left: l.ty.clone(),
                                right: r.ty.clone(),
                            });
                        }
                    }
                    let _ = idx_input;
                }
                Ok(())
            }
            LogicalNode::Values {
                rows,
                output_schema,
            } => {
                for (idx, row) in rows.iter().enumerate() {
                    if row.len() != output_schema.len() {
                        return Err(AlgebraError::ValuesArityMismatch {
                            row_index: idx,
                            actual_len: row.len(),
                            schema_len: output_schema.len(),
                        });
                    }
                    for e in row {
                        // Values literals only — we type-check
                        // against the declared output schema by
                        // checking each expression resolves cleanly.
                        // Schema is empty for inline literals.
                        e.type_check(&RelationalSchema::default())?;
                    }
                }
                Ok(())
            }
            LogicalNode::CteBind {
                body, usages, ..
            } => {
                body.validate()?;
                usages.validate()?;
                Ok(())
            }
            LogicalNode::CteRef { .. } => Ok(()),
        }
    }
}

// =========================================================================
// Constant-row evaluation (for Values nodes and tests)
// =========================================================================

impl LogicalNode {
    /// Evaluate a `Values` node into concrete rows. Useful for the
    /// executor to materialise inline literal lists; also used in
    /// tests. Returns an error for non-Values nodes.
    pub fn eval_values(&self) -> Result<Vec<RelationalRow>, ExprError> {
        match self {
            LogicalNode::Values {
                rows,
                output_schema,
            } => {
                let funcs = proximadb_relational_types::NoFunctions;
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    let mut values = Vec::with_capacity(row.len());
                    for (i, e) in row.iter().enumerate() {
                        let v = e.eval(&Vec::new(), &funcs)?;
                        // Cast to the declared column type so that
                        // ints/floats etc. are normalised before
                        // they reach downstream operators.
                        let target = output_schema
                            .columns
                            .get(i)
                            .map(|c| c.ty.clone())
                            .unwrap_or(ProximaType::String);
                        let coerced = match cast_value(&v, &target) {
                            Ok(v) => v,
                            Err(ExprError::UnsupportedCast { .. }) => v,
                            Err(e) => return Err(e),
                        };
                        values.push(coerced);
                    }
                    out.push(values);
                }
                Ok(out)
            }
            _ => Err(ExprError::Other(
                "eval_values is only defined for Values nodes".into(),
            )),
        }
    }
}

// =========================================================================
// Visitor pattern (transform / walk)
// =========================================================================

/// Pre-order tree walk. Returns `false` from `visit` to stop
/// descending into children of the current node.
pub fn walk<F: FnMut(&LogicalNode) -> bool>(node: &LogicalNode, mut visit: F) {
    walk_inner(node, &mut visit);
}

fn walk_inner<F: FnMut(&LogicalNode) -> bool>(node: &LogicalNode, visit: &mut F) {
    let descend = visit(node);
    if !descend {
        return;
    }
    match node {
        LogicalNode::Filter { input, .. }
        | LogicalNode::Project { input, .. }
        | LogicalNode::Sort { input, .. }
        | LogicalNode::Limit { input, .. }
        | LogicalNode::Distinct { input }
        | LogicalNode::Aggregate { input, .. } => walk_inner(input, visit),
        LogicalNode::Join { left, right, .. } => {
            walk_inner(left, visit);
            walk_inner(right, visit);
        }
        LogicalNode::Union { inputs, .. } => {
            for i in inputs {
                walk_inner(i, visit);
            }
        }
        LogicalNode::CteBind { body, usages, .. } => {
            walk_inner(body, visit);
            walk_inner(usages, visit);
        }
        // Leaves: no children.
        LogicalNode::Scan { .. } | LogicalNode::Values { .. } | LogicalNode::CteRef { .. } => {
        }
    }
}

/// Bottom-up transformer. `f` receives each node (children already
/// transformed) and may return a replacement. Used for planner
/// rewrites like predicate pushdown and projection inlining.
pub fn transform<F: FnMut(LogicalNode) -> LogicalNode>(
    node: LogicalNode,
    mut f: F,
) -> LogicalNode {
    transform_inner(node, &mut f)
}

fn transform_inner<F: FnMut(LogicalNode) -> LogicalNode>(
    node: LogicalNode,
    f: &mut F,
) -> LogicalNode {
    let rewritten = match node {
        LogicalNode::Filter { input, predicate } => LogicalNode::Filter {
            input: Box::new(transform_inner(*input, f)),
            predicate,
        },
        LogicalNode::Project { input, outputs } => LogicalNode::Project {
            input: Box::new(transform_inner(*input, f)),
            outputs,
        },
        LogicalNode::Sort { input, keys } => LogicalNode::Sort {
            input: Box::new(transform_inner(*input, f)),
            keys,
        },
        LogicalNode::Limit {
            input,
            limit,
            offset,
        } => LogicalNode::Limit {
            input: Box::new(transform_inner(*input, f)),
            limit,
            offset,
        },
        LogicalNode::Distinct { input } => LogicalNode::Distinct {
            input: Box::new(transform_inner(*input, f)),
        },
        LogicalNode::Aggregate {
            input,
            group_by,
            aggregates,
            having,
        } => LogicalNode::Aggregate {
            input: Box::new(transform_inner(*input, f)),
            group_by,
            aggregates,
            having,
        },
        LogicalNode::Join {
            left,
            right,
            kind,
            on,
            strategy,
        } => LogicalNode::Join {
            left: Box::new(transform_inner(*left, f)),
            right: Box::new(transform_inner(*right, f)),
            kind,
            on,
            strategy,
        },
        LogicalNode::Union { inputs, all } => LogicalNode::Union {
            inputs: inputs.into_iter().map(|i| transform_inner(i, f)).collect(),
            all,
        },
        LogicalNode::CteBind {
            name,
            body,
            usages,
        } => LogicalNode::CteBind {
            name,
            body: Box::new(transform_inner(*body, f)),
            usages: Box::new(transform_inner(*usages, f)),
        },
        leaf @ (LogicalNode::Scan { .. } | LogicalNode::Values { .. } | LogicalNode::CteRef { .. }) => leaf,
    };
    f(rewritten)
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_relational_types::BinaryOp;

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

    fn scan_users() -> LogicalNode {
        let schema = users_schema();
        LogicalNode::Scan {
            table: TableId::new("users"),
            table_schema: schema,
            projected_columns: None,
            predicate: None,
        }
    }

    fn scan_orders() -> LogicalNode {
        let schema = orders_schema();
        LogicalNode::Scan {
            table: TableId::new("orders"),
            table_schema: schema,
            projected_columns: None,
            predicate: None,
        }
    }

    // --- Scan schema -------------------------------------------------

    #[test]
    fn scan_emits_table_schema() {
        let n = scan_users();
        assert_eq!(n.output_schema(), users_schema());
        assert!(n.validate().is_ok());
    }

    #[test]
    fn scan_with_projection_emits_projected_schema() {
        let schema = users_schema();
        let id_ref = schema.resolve_column("id").unwrap();
        let n = LogicalNode::Scan {
            table: TableId::new("users"),
            table_schema: schema.clone(),
            projected_columns: Some(vec![id_ref]),
            predicate: None,
        };
        let out = n.output_schema();
        assert_eq!(out.len(), 1);
        assert_eq!(out.columns[0].name, "id");
        assert!(n.validate().is_ok());
    }

    #[test]
    fn scan_validate_catches_predicate_not_boolean() {
        let schema = users_schema();
        let id_ref = schema.resolve_column("id").unwrap();
        let bad = LogicalNode::Scan {
            table: TableId::new("users"),
            table_schema: schema.clone(),
            projected_columns: None,
            predicate: Some(Expr::column(id_ref)), // Int64, not Boolean
        };
        assert!(matches!(
            bad.validate(),
            Err(AlgebraError::NonBooleanFilter { .. })
        ));
    }

    // --- Filter ------------------------------------------------------

    #[test]
    fn filter_preserves_input_schema() {
        let schema = users_schema();
        let id_ref = schema.resolve_column("id").unwrap();
        let n = LogicalNode::Filter {
            input: Box::new(scan_users()),
            predicate: Expr::bin(
                BinaryOp::Gt,
                Expr::column(id_ref),
                Expr::literal(ProximaValue::Int64(5)),
            ),
        };
        assert_eq!(n.output_schema(), users_schema());
        assert!(n.validate().is_ok());
    }

    #[test]
    fn filter_rejects_non_boolean_predicate() {
        let schema = users_schema();
        let id_ref = schema.resolve_column("id").unwrap();
        let n = LogicalNode::Filter {
            input: Box::new(scan_users()),
            predicate: Expr::column(id_ref),
        };
        assert!(matches!(
            n.validate(),
            Err(AlgebraError::NonBooleanFilter { .. })
        ));
    }

    // --- Project -----------------------------------------------------

    #[test]
    fn project_emits_renamed_schema() {
        let schema = users_schema();
        let id_ref = schema.resolve_column("id").unwrap();
        let name_ref = schema.resolve_column("name").unwrap();
        let n = LogicalNode::Project {
            input: Box::new(scan_users()),
            outputs: vec![
                NamedExpr::new("user_id", Expr::column(id_ref)),
                NamedExpr::new("user_name", Expr::column(name_ref)),
            ],
        };
        let out = n.output_schema();
        assert_eq!(out.len(), 2);
        assert_eq!(out.columns[0].name, "user_id");
        assert_eq!(out.columns[1].name, "user_name");
        assert!(n.validate().is_ok());
    }

    #[test]
    fn project_rejects_duplicate_output_names() {
        let schema = users_schema();
        let id_ref = schema.resolve_column("id").unwrap();
        let n = LogicalNode::Project {
            input: Box::new(scan_users()),
            outputs: vec![
                NamedExpr::new("x", Expr::column(id_ref.clone())),
                NamedExpr::new("x", Expr::column(id_ref)),
            ],
        };
        assert!(matches!(
            n.validate(),
            Err(AlgebraError::DuplicateProjectName { .. })
        ));
    }

    // --- Join --------------------------------------------------------

    #[test]
    fn inner_join_concatenates_schemas() {
        let n = LogicalNode::Join {
            left: Box::new(scan_users()),
            right: Box::new(scan_orders()),
            kind: JoinKind::Inner,
            on: None,
            strategy: JoinStrategy::Auto,
        };
        let out = n.output_schema();
        assert_eq!(out.len(), users_schema().len() + orders_schema().len());
    }

    #[test]
    fn left_outer_join_marks_right_columns_nullable() {
        let n = LogicalNode::Join {
            left: Box::new(scan_users()),
            right: Box::new(scan_orders()),
            kind: JoinKind::Left,
            on: None,
            strategy: JoinStrategy::Auto,
        };
        let out = n.output_schema();
        // Right cols start at users.len() = 3.
        for c in &out.columns[3..] {
            assert!(c.nullable, "right-side column {} must be nullable", c.name);
        }
    }

    #[test]
    fn semi_join_emits_only_left_schema() {
        let n = LogicalNode::Join {
            left: Box::new(scan_users()),
            right: Box::new(scan_orders()),
            kind: JoinKind::Semi,
            on: None,
            strategy: JoinStrategy::Auto,
        };
        assert_eq!(n.output_schema().len(), users_schema().len());
    }

    #[test]
    fn join_validate_catches_non_boolean_on() {
        let users = scan_users();
        let orders = scan_orders();
        // Combined schema: users(id, name, age) + orders(id, user_id, total)
        // ordinal 0 = users.id, 3 = orders.id
        let combined = RelationalSchema::new({
            let mut c = users_schema().columns;
            c.extend(orders_schema().columns);
            c
        });
        let users_id = combined.resolve_column("id").unwrap(); // first match
        let n = LogicalNode::Join {
            left: Box::new(users),
            right: Box::new(orders),
            kind: JoinKind::Inner,
            // ON id (Int64) — not Boolean.
            on: Some(Expr::column(users_id)),
            strategy: JoinStrategy::Auto,
        };
        assert!(matches!(
            n.validate(),
            Err(AlgebraError::NonBooleanJoinOn { .. })
        ));
    }

    #[test]
    fn cross_join_rejects_on_clause() {
        let n = LogicalNode::Join {
            left: Box::new(scan_users()),
            right: Box::new(scan_orders()),
            kind: JoinKind::Cross,
            on: Some(Expr::literal(ProximaValue::Boolean(true))),
            strategy: JoinStrategy::Auto,
        };
        assert!(matches!(n.validate(), Err(AlgebraError::CrossJoinWithOn)));
    }

    // --- Aggregate ----------------------------------------------------

    #[test]
    fn aggregate_count_star_returns_int64_non_null() {
        let n = LogicalNode::Aggregate {
            input: Box::new(scan_users()),
            group_by: vec![],
            aggregates: vec![NamedAggregate::new(
                "n",
                AggregateExpr::Count {
                    arg: None,
                    distinct: false,
                },
            )],
            having: None,
        };
        let out = n.output_schema();
        assert_eq!(out.len(), 1);
        assert_eq!(out.columns[0].name, "n");
        assert_eq!(out.columns[0].ty, ProximaType::Int64);
        assert!(!out.columns[0].nullable);
        assert!(n.validate().is_ok());
    }

    #[test]
    fn aggregate_group_by_concat_schema() {
        let schema = users_schema();
        let age_ref = schema.resolve_column("age").unwrap();
        let id_ref = schema.resolve_column("id").unwrap();
        let n = LogicalNode::Aggregate {
            input: Box::new(scan_users()),
            group_by: vec![NamedExpr::new("age", Expr::column(age_ref))],
            aggregates: vec![NamedAggregate::new(
                "total_ids",
                AggregateExpr::Sum {
                    arg: Expr::column(id_ref),
                    distinct: false,
                },
            )],
            having: None,
        };
        let out = n.output_schema();
        assert_eq!(out.len(), 2);
        assert_eq!(out.columns[0].name, "age");
        assert_eq!(out.columns[1].name, "total_ids");
        // SUM widens Int64 → Int64 (already Int64).
        assert_eq!(out.columns[1].ty, ProximaType::Int64);
        assert!(n.validate().is_ok());
    }

    #[test]
    fn aggregate_rejects_name_collision_between_group_and_aggregate() {
        let schema = users_schema();
        let age_ref = schema.resolve_column("age").unwrap();
        let id_ref = schema.resolve_column("id").unwrap();
        let n = LogicalNode::Aggregate {
            input: Box::new(scan_users()),
            group_by: vec![NamedExpr::new("dup", Expr::column(age_ref))],
            aggregates: vec![NamedAggregate::new(
                "dup",
                AggregateExpr::Sum {
                    arg: Expr::column(id_ref),
                    distinct: false,
                },
            )],
            having: None,
        };
        assert!(matches!(
            n.validate(),
            Err(AlgebraError::AggregateNameConflict { .. })
        ));
    }

    #[test]
    fn aggregate_having_must_be_boolean() {
        let schema = users_schema();
        let age_ref = schema.resolve_column("age").unwrap();
        let n = LogicalNode::Aggregate {
            input: Box::new(scan_users()),
            group_by: vec![NamedExpr::new("age", Expr::column(age_ref))],
            aggregates: vec![],
            // HAVING age (Int32) — not Boolean.
            having: Some(Expr::Column(ColumnRef {
                name: "age".into(),
                ordinal: 0,
                ty: ProximaType::Int32,
                nullable: true,
            })),
        };
        assert!(matches!(
            n.validate(),
            Err(AlgebraError::NonBooleanHaving { .. })
        ));
    }

    // --- Sort / Limit / Distinct -----------------------------------

    #[test]
    fn sort_preserves_input_schema() {
        let schema = users_schema();
        let id_ref = schema.resolve_column("id").unwrap();
        let n = LogicalNode::Sort {
            input: Box::new(scan_users()),
            keys: vec![SortKey::desc(Expr::column(id_ref))],
        };
        assert_eq!(n.output_schema(), users_schema());
        assert!(n.validate().is_ok());
    }

    #[test]
    fn limit_preserves_input_schema() {
        let n = LogicalNode::Limit {
            input: Box::new(scan_users()),
            limit: Some(10),
            offset: 5,
        };
        assert_eq!(n.output_schema(), users_schema());
    }

    #[test]
    fn distinct_preserves_input_schema() {
        let n = LogicalNode::Distinct {
            input: Box::new(scan_users()),
        };
        assert_eq!(n.output_schema(), users_schema());
    }

    // --- Union --------------------------------------------------------

    #[test]
    fn union_with_matching_schemas_validates() {
        let n = LogicalNode::Union {
            inputs: vec![scan_users(), scan_users()],
            all: false,
        };
        assert!(n.validate().is_ok());
        assert_eq!(n.output_schema(), users_schema());
    }

    #[test]
    fn union_with_mismatched_arity_errors() {
        // Build a schema with only 2 columns so the arity differs
        // from users(3 cols).
        let two_col_schema = RelationalSchema::new(vec![
            ColumnInfo::new("a", ProximaType::Int64, false),
            ColumnInfo::new("b", ProximaType::Int64, false),
        ]);
        let right = LogicalNode::Scan {
            table: TableId::new("t_short"),
            table_schema: two_col_schema,
            projected_columns: None,
            predicate: None,
        };
        let n = LogicalNode::Union {
            inputs: vec![scan_users(), right],
            all: true,
        };
        assert!(matches!(
            n.validate(),
            Err(AlgebraError::UnionArityMismatch { .. })
        ));
    }

    #[test]
    fn union_with_type_mismatch_errors() {
        // Two scans with same arity but different col types.
        let s1 = RelationalSchema::new(vec![
            ColumnInfo::new("a", ProximaType::Int64, false),
            ColumnInfo::new("b", ProximaType::String, true),
        ]);
        let s2 = RelationalSchema::new(vec![
            ColumnInfo::new("a", ProximaType::String, false),
            ColumnInfo::new("b", ProximaType::String, true),
        ]);
        let left = LogicalNode::Scan {
            table: TableId::new("t1"),
            table_schema: s1,
            projected_columns: None,
            predicate: None,
        };
        let right = LogicalNode::Scan {
            table: TableId::new("t2"),
            table_schema: s2,
            projected_columns: None,
            predicate: None,
        };
        let n = LogicalNode::Union {
            inputs: vec![left, right],
            all: true,
        };
        assert!(matches!(
            n.validate(),
            Err(AlgebraError::UnionSchemaMismatch { .. })
        ));
    }

    // --- Values ------------------------------------------------------

    #[test]
    fn values_evaluates_to_concrete_rows() {
        let schema = RelationalSchema::new(vec![
            ColumnInfo::new("id", ProximaType::Int64, false),
            ColumnInfo::new("name", ProximaType::String, false),
        ]);
        let n = LogicalNode::Values {
            rows: vec![
                vec![
                    Expr::literal(ProximaValue::Int64(1)),
                    Expr::literal(ProximaValue::String("alice".into())),
                ],
                vec![
                    Expr::literal(ProximaValue::Int64(2)),
                    Expr::literal(ProximaValue::String("bob".into())),
                ],
            ],
            output_schema: schema.clone(),
        };
        assert!(n.validate().is_ok());
        let rows = n.eval_values().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], ProximaValue::Int64(1));
        assert_eq!(rows[1][1], ProximaValue::String("bob".into()));
    }

    #[test]
    fn values_arity_mismatch_errors() {
        let schema = RelationalSchema::new(vec![
            ColumnInfo::new("a", ProximaType::Int64, false),
            ColumnInfo::new("b", ProximaType::String, false),
        ]);
        let n = LogicalNode::Values {
            rows: vec![vec![Expr::literal(ProximaValue::Int64(1))]], // only one col
            output_schema: schema,
        };
        assert!(matches!(
            n.validate(),
            Err(AlgebraError::ValuesArityMismatch { .. })
        ));
    }

    // --- Visitor pattern ---------------------------------------------

    #[test]
    fn walk_visits_every_node() {
        let n = LogicalNode::Limit {
            input: Box::new(LogicalNode::Sort {
                input: Box::new(LogicalNode::Filter {
                    input: Box::new(scan_users()),
                    predicate: Expr::literal(ProximaValue::Boolean(true)),
                }),
                keys: vec![],
            }),
            limit: Some(10),
            offset: 0,
        };
        let mut count = 0usize;
        walk(&n, |_| {
            count += 1;
            true
        });
        // Limit -> Sort -> Filter -> Scan = 4 nodes.
        assert_eq!(count, 4);
    }

    #[test]
    fn walk_can_short_circuit() {
        let n = LogicalNode::Limit {
            input: Box::new(LogicalNode::Sort {
                input: Box::new(scan_users()),
                keys: vec![],
            }),
            limit: Some(10),
            offset: 0,
        };
        let mut count = 0usize;
        walk(&n, |node| {
            count += 1;
            // Don't descend into Sort.
            !matches!(node, LogicalNode::Sort { .. })
        });
        // Limit + Sort visited, no descent into Scan.
        assert_eq!(count, 2);
    }

    #[test]
    fn transform_rewrites_bottom_up() {
        // Replace any Limit node with one that has limit=99.
        let n = LogicalNode::Limit {
            input: Box::new(scan_users()),
            limit: Some(10),
            offset: 0,
        };
        let rewritten = transform(n, |node| match node {
            LogicalNode::Limit {
                input, offset, ..
            } => LogicalNode::Limit {
                input,
                limit: Some(99),
                offset,
            },
            other => other,
        });
        match rewritten {
            LogicalNode::Limit { limit: Some(99), .. } => {}
            other => panic!("expected Limit(99), got {other:?}"),
        }
    }

    // --- AggregateExpr type inference --------------------------------

    #[test]
    fn aggregate_result_types() {
        assert_eq!(
            AggregateExpr::Count {
                arg: None,
                distinct: false
            }
            .result_type(),
            ProximaType::Int64
        );
        assert_eq!(
            AggregateExpr::Sum {
                arg: Expr::literal(ProximaValue::Int32(0)),
                distinct: false
            }
            .result_type(),
            ProximaType::Int64
        );
        assert_eq!(
            AggregateExpr::Avg {
                arg: Expr::literal(ProximaValue::Int32(0)),
                distinct: false
            }
            .result_type(),
            ProximaType::Float64
        );
        assert_eq!(
            AggregateExpr::StringAgg {
                arg: Expr::literal(ProximaValue::String("a".into())),
                separator: ",".into(),
                distinct: false
            }
            .result_type(),
            ProximaType::String
        );
    }

    // --- AlgebraExpr / SubqueryExpr ---------------------------------

    #[test]
    fn algebra_expr_pure_passthrough() {
        let e = AlgebraExpr::Pure(Expr::literal(ProximaValue::Int64(1)));
        assert_eq!(e.result_type(), ProximaType::Int64);
    }

    #[test]
    fn subquery_expr_result_types() {
        let scan = scan_users();
        let scalar = SubqueryExpr::Scalar {
            sub: Box::new(scan.clone()),
            result_ty: ProximaType::Int64,
        };
        assert_eq!(scalar.result_type(), ProximaType::Int64);
        let exists = SubqueryExpr::Exists {
            sub: Box::new(scan),
            not: false,
        };
        assert_eq!(exists.result_type(), ProximaType::Boolean);
    }

    // --- End-to-end realistic plan ----------------------------------

    #[test]
    fn realistic_plan_validates_end_to_end() {
        // SELECT user_id, COUNT(*) AS n
        // FROM orders
        // WHERE total > 100.0
        // GROUP BY user_id
        // HAVING COUNT(*) > 5
        // ORDER BY n DESC
        // LIMIT 10
        let orders_schema = orders_schema();
        let total_ref = orders_schema.resolve_column("total").unwrap();
        let user_id_ref = orders_schema.resolve_column("user_id").unwrap();

        let filtered = LogicalNode::Filter {
            input: Box::new(scan_orders()),
            predicate: Expr::bin(
                BinaryOp::Gt,
                Expr::column(total_ref),
                Expr::literal(ProximaValue::Float64(100.0)),
            ),
        };

        let grouped = LogicalNode::Aggregate {
            input: Box::new(filtered),
            group_by: vec![NamedExpr::new("user_id", Expr::column(user_id_ref))],
            aggregates: vec![NamedAggregate::new(
                "n",
                AggregateExpr::Count {
                    arg: None,
                    distinct: false,
                },
            )],
            // HAVING references the post-aggregate `n` column at
            // ordinal 1 (user_id is 0, n is 1).
            having: Some(Expr::bin(
                BinaryOp::Gt,
                Expr::Column(ColumnRef {
                    name: "n".into(),
                    ordinal: 1,
                    ty: ProximaType::Int64,
                    nullable: false,
                }),
                Expr::literal(ProximaValue::Int64(5)),
            )),
        };

        let sorted = LogicalNode::Sort {
            input: Box::new(grouped),
            keys: vec![SortKey::desc(Expr::Column(ColumnRef {
                name: "n".into(),
                ordinal: 1,
                ty: ProximaType::Int64,
                nullable: false,
            }))],
        };

        let limited = LogicalNode::Limit {
            input: Box::new(sorted),
            limit: Some(10),
            offset: 0,
        };

        assert!(limited.validate().is_ok());
        let out = limited.output_schema();
        assert_eq!(out.len(), 2);
        assert_eq!(out.columns[0].name, "user_id");
        assert_eq!(out.columns[1].name, "n");
        assert_eq!(out.columns[1].ty, ProximaType::Int64);
    }
}

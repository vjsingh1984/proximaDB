#![allow(clippy::doc_lazy_continuation)]
// cosmetic: newer clippy lint on pre-existing doc list-rendering; no functional impact
// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Canonical cross-engine function contract + registry
//!
//! ProximaDB runs queries on several physical engines — the hand-rolled **Volcano**
//! executor (OLTP / strong-freshness), **DataFusion** (OLAP/MPP), and the specialized
//! vector/ANN engines — over one shared logical plane
//! (`proximadb_relational_algebra::LogicalNode` + `proximadb_relational_types::Expr`).
//!
//! Following the pattern of mature engines (PostgreSQL `pg_proc`, Calcite
//! `SqlOperatorTable`, Trino's function SPI, DataFusion's own `FunctionRegistry`, Velox's
//! shared registry), a function here is a **registry object** — a name + kind + typed
//! signature + volatility + an *engine-neutral* implementation over [`ProximaValue`] — that
//! the logical plane references and each physical engine BINDS natively:
//!
//! * **Volcano** binds via [`proximadb_relational_types::FunctionRegistry`] (this crate
//!   implements it for [`ProximaFunctionRegistry`]), evaluating `ProximaValue → ProximaValue`
//!   row-at-a-time — right for low-latency OLTP.
//! * **DataFusion** binds by wrapping each kernel as a `ScalarUDF` (or attaching a native
//!   vectorized Arrow kernel) at the adapter layer in `src/datafusion/` — right for
//!   vectorized OLAP. (That adapter lives in the root crate so this foundation crate stays
//!   free of any DataFusion/Arrow dependency.)
//!
//! Definition is single-sourced; execution is native per engine. This is the
//! "bridge/adapter between both" done once at the definition layer rather than as a lossy
//! per-call runtime conversion.
//!
//! ## Scope (F1)
//! Scalar functions + the registry + the Volcano binding. Aggregates (UDAF) and table
//! functions (UDTF) extend the same registry in later phases; `CREATE FUNCTION` + an
//! xCatalog function catalog (durable authority) is the eventual source of user kernels.

use std::sync::Arc;

use dashmap::DashMap;
use proximadb_data_model::{ProximaType, ProximaValue, TimeUnit as DmTimeUnit};
use proximadb_relational_types::{Expr, ExprError, FunctionRegistry, RelationalRow};

/// What kind of function a registry entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKind {
    /// Row → value (e.g. `upper`, `abs`).
    Scalar,
    /// Set of rows → value (e.g. `sum`, custom aggregates) — F3.
    Aggregate,
    /// Parameters → a table (e.g. `vector_search`) — F4, DataFusion-side.
    Table,
}

/// Optimizer hint about determinism (mirrors PostgreSQL's volatility classes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Volatility {
    /// Same inputs always yield the same output; safe to constant-fold.
    Immutable,
    /// Stable within a single query/statement.
    Stable,
    /// May differ on every call (e.g. `random`, `now`).
    Volatile,
}

/// A function's typed signature in the canonical [`ProximaType`] system. Engines convert
/// to/from their own type systems (Arrow for DataFusion) at the adapter boundary.
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    /// Canonical (lower-cased) function name.
    pub name: String,
    /// Declared argument types. Empty when `variadic` (uniform-typed varargs).
    pub arg_types: Vec<ProximaType>,
    /// Accepts a variable number of arguments.
    pub variadic: bool,
    /// Declared return type (used to fill `Expr::FuncCall.return_ty` at lowering).
    pub return_ty: ProximaType,
    /// Determinism class.
    pub volatility: Volatility,
}

/// The engine-neutral scalar implementation: already-evaluated args → result. Receives
/// `ProximaValue::Null` args unchanged (kernels propagate NULL per SQL semantics).
pub type ScalarFn = dyn Fn(&[ProximaValue]) -> Result<ProximaValue, ExprError> + Send + Sync;

/// A registered scalar function: its signature + its canonical kernel.
#[derive(Clone)]
pub struct ScalarFunctionDef {
    /// Typed signature.
    pub signature: FunctionSignature,
    /// Engine-neutral row-wise implementation.
    pub kernel: Arc<ScalarFn>,
}

impl std::fmt::Debug for ScalarFunctionDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScalarFunctionDef")
            .field("signature", &self.signature)
            .finish_non_exhaustive()
    }
}

/// Per-group aggregate state. The engine drives it: `update` folds each input row's argument
/// values; `finalize` produces the group's value (NULL for an empty group, per SQL). `state`
/// + `merge` exist for partitioned execution (DataFusion): a partial accumulator exposes its
/// state as values, and a coordinator folds peers' states back in. NULL handling is the
/// accumulator's responsibility (standard SQL aggregates skip NULL args).
///
/// Single-node Volcano uses only `update` + `finalize` (one accumulator per group, fed every
/// row); `state`/`merge` are the DataFusion partial-aggregation seam.
///
/// `Send + Sync` so the DataFusion `Accumulator` adapter (which requires both) can wrap it;
/// accumulators are never shared across threads in practice (one per group).
pub trait AggregateAccumulator: Send + Sync {
    /// Fold one input row's evaluated argument values into the accumulator.
    fn update(&mut self, args: &[ProximaValue]) -> Result<(), ExprError>;

    /// This accumulator's partial state, as canonical values (for cross-partition merge).
    fn state(&self) -> Vec<ProximaValue>;

    /// Fold another accumulator's partial `state` (from [`AggregateAccumulator::state`]) in.
    fn merge(&mut self, state: &[ProximaValue]) -> Result<(), ExprError>;

    /// The group's final aggregate value.
    fn finalize(&self) -> ProximaValue;
}

/// Factory for an aggregate's per-group accumulators. Engine-neutral, like [`ScalarFn`].
pub trait AggregateKernel: Send + Sync {
    /// Create a fresh accumulator for one group.
    fn new_accumulator(&self) -> Box<dyn AggregateAccumulator>;

    /// Canonical types of the partial state exposed by [`AggregateAccumulator::state`], in
    /// order. DataFusion needs these declared up-front to wire partitioned aggregation; the
    /// single-node Volcano path ignores them.
    fn state_types(&self) -> Vec<ProximaType>;
}

/// A registered aggregate function: its signature (`kind = Aggregate`) + its accumulator
/// factory. The same definition is bound by Volcano (custom accumulator) and DataFusion
/// (`AggregateUDF` adapter).
#[derive(Clone)]
pub struct AggregateFunctionDef {
    /// Typed signature.
    pub signature: FunctionSignature,
    /// Engine-neutral accumulator factory.
    pub kernel: Arc<dyn AggregateKernel>,
}

impl std::fmt::Debug for AggregateFunctionDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AggregateFunctionDef")
            .field("signature", &self.signature)
            .finish_non_exhaustive()
    }
}

/// The canonical function registry. Built once at startup with builtins and (later) user
/// functions from the xCatalog function catalog; read concurrently by every engine.
///
/// `DashMap`-backed (mirroring `proximadb_rank_core::BlueprintFactory`) so runtime
/// registration (`CREATE FUNCTION`, F5) needs no `&mut self`.
#[derive(Default)]
pub struct ProximaFunctionRegistry {
    scalars: DashMap<String, Arc<ScalarFunctionDef>>,
    aggregates: DashMap<String, Arc<AggregateFunctionDef>>,
}

impl ProximaFunctionRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry preloaded with the builtin scalar + aggregate functions.
    pub fn with_builtins() -> Self {
        let reg = Self::new();
        register_builtin_scalars(&reg);
        register_builtin_aggregates(&reg);
        reg
    }

    /// Register (or replace) a scalar function under its (lower-cased) name.
    pub fn register_scalar(&self, def: ScalarFunctionDef) {
        let key = def.signature.name.to_ascii_lowercase();
        self.scalars.insert(key, Arc::new(def));
    }

    /// Register the same scalar under an alias name (e.g. `ucase` → `upper`).
    ///
    /// The target lookup is cloned out and its `DashMap` read-guard dropped
    /// *before* the alias insert. Holding the `get` `Ref` (a shard read-lock)
    /// across `insert` (a shard write-lock) self-deadlocks the calling thread
    /// whenever `alias` and `target` hash to the same shard — and DashMap's
    /// hasher is random-seeded per process, so that collision (and the hang) was
    /// intermittent (~1-2%/process). This was the root cause of the rare
    /// `native_volcano_*` / SQL-function CI hang: the builtin-registry `LazyLock`
    /// init wedged here (confirmed by stack sample), riding to nextest's 120s
    /// slow-timeout. The registry init is on the server's first-SQL-function path
    /// too, so this is a latent production hazard, not only a test flake.
    pub fn alias_scalar(&self, alias: &str, target: &str) {
        let target_def = self
            .scalars
            .get(&target.to_ascii_lowercase())
            .map(|def| def.clone());
        if let Some(def) = target_def {
            self.scalars.insert(alias.to_ascii_lowercase(), def);
        }
    }

    /// Look up a scalar function by name (case-insensitive).
    pub fn lookup_scalar(&self, name: &str) -> Option<Arc<ScalarFunctionDef>> {
        self.scalars
            .get(&name.to_ascii_lowercase())
            .map(|e| e.clone())
    }

    /// Number of registered scalar functions (incl. aliases).
    pub fn scalar_count(&self) -> usize {
        self.scalars.len()
    }

    /// All registered scalar definitions (incl. alias entries, which share the same
    /// `Arc<ScalarFunctionDef>` as their target). Used by engine adapters that bind every
    /// registry function into a native runtime (e.g. the DataFusion `ScalarUDF` adapter).
    pub fn scalar_defs(&self) -> Vec<Arc<ScalarFunctionDef>> {
        self.scalars.iter().map(|e| e.value().clone()).collect()
    }

    /// Register (or replace) an aggregate function under its (lower-cased) name.
    pub fn register_aggregate(&self, def: AggregateFunctionDef) {
        let key = def.signature.name.to_ascii_lowercase();
        self.aggregates.insert(key, Arc::new(def));
    }

    /// Look up an aggregate function by name (case-insensitive).
    pub fn lookup_aggregate(&self, name: &str) -> Option<Arc<AggregateFunctionDef>> {
        self.aggregates
            .get(&name.to_ascii_lowercase())
            .map(|e| e.clone())
    }

    /// Number of registered aggregate functions.
    pub fn aggregate_count(&self) -> usize {
        self.aggregates.len()
    }

    /// All registered aggregate definitions (for engine adapters that bind every aggregate
    /// into a native runtime, e.g. the DataFusion `AggregateUDF` adapter).
    pub fn aggregate_defs(&self) -> Vec<Arc<AggregateFunctionDef>> {
        self.aggregates.iter().map(|e| e.value().clone()).collect()
    }
}

/// The process-wide builtin scalar registry. Builtin functions are global (every query
/// has them), like PostgreSQL's builtins; per-query/per-tenant *user* functions
/// (`CREATE FUNCTION`, F5) are injected separately. Both the Volcano executor and the SQL
/// frontend resolve against this single source.
pub fn builtins() -> &'static ProximaFunctionRegistry {
    static BUILTINS: std::sync::LazyLock<ProximaFunctionRegistry> =
        std::sync::LazyLock::new(ProximaFunctionRegistry::with_builtins);
    &BUILTINS
}

/// Volcano binding: the relational executor dispatches `Expr::FuncCall` through this.
impl FunctionRegistry for ProximaFunctionRegistry {
    fn dispatch(
        &self,
        name: &str,
        args: &[ProximaValue],
    ) -> Option<Result<ProximaValue, ExprError>> {
        let def = self.lookup_scalar(name)?;
        Some((def.kernel)(args))
    }
}

// =========================================================================
// Builtin scalar kernels (engine-neutral, NULL-propagating)
// =========================================================================

fn any_null(args: &[ProximaValue]) -> bool {
    args.iter().any(|v| matches!(v, ProximaValue::Null))
}

fn check_arity(name: &str, args: &[ProximaValue], n: usize) -> Result<(), ExprError> {
    if args.len() != n {
        return Err(ExprError::WrongFunctionArity {
            name: name.to_string(),
            expected: n,
            got: args.len(),
        });
    }
    Ok(())
}

fn as_str<'a>(name: &str, v: &'a ProximaValue) -> Result<&'a str, ExprError> {
    match v {
        ProximaValue::String(s) | ProximaValue::Symbol(s) => Ok(s.as_str()),
        other => Err(ExprError::Other(format!(
            "{name}: expected a string argument, got {other:?}"
        ))),
    }
}

fn as_f64(name: &str, v: &ProximaValue) -> Result<f64, ExprError> {
    match v {
        ProximaValue::Float64(f) => Ok(*f),
        ProximaValue::Float32(f) => Ok(*f as f64),
        ProximaValue::Int64(i) => Ok(*i as f64),
        ProximaValue::Int32(i) => Ok(*i as f64),
        ProximaValue::Int16(i) => Ok(*i as f64),
        ProximaValue::Int8(i) => Ok(*i as f64),
        ProximaValue::UInt64(i) => Ok(*i as f64),
        ProximaValue::UInt32(i) => Ok(*i as f64),
        other => Err(ExprError::Other(format!(
            "{name}: expected a numeric argument, got {other:?}"
        ))),
    }
}

fn as_i64(name: &str, v: &ProximaValue) -> Result<i64, ExprError> {
    match v {
        ProximaValue::Int64(i) => Ok(*i),
        ProximaValue::Int32(i) => Ok(*i as i64),
        ProximaValue::Int16(i) => Ok(*i as i64),
        ProximaValue::Int8(i) => Ok(*i as i64),
        ProximaValue::UInt64(i) => i64::try_from(*i)
            .map_err(|_| ExprError::Other(format!("{name}: integer argument out of range"))),
        ProximaValue::UInt32(i) => Ok(*i as i64),
        other => Err(ExprError::Other(format!(
            "{name}: expected an integer argument, got {other:?}"
        ))),
    }
}

fn is_integer(v: &ProximaValue) -> bool {
    matches!(
        v,
        ProximaValue::Int8(_)
            | ProximaValue::Int16(_)
            | ProximaValue::Int32(_)
            | ProximaValue::Int64(_)
            | ProximaValue::UInt32(_)
            | ProximaValue::UInt64(_)
    )
}

fn check_arity_range(
    name: &str,
    args: &[ProximaValue],
    min: usize,
    max: usize,
) -> Result<(), ExprError> {
    if args.len() < min || args.len() > max {
        return Err(ExprError::WrongFunctionArity {
            name: name.to_string(),
            expected: min,
            got: args.len(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Civil-date arithmetic (Howard Hinnant's algorithms) — dependency-free
// helpers for the temporal kernels. `Date(i32)` is days since 1970-01-01;
// `Timestamp(i64, unit)` is ticks since the epoch in `unit`.
// ---------------------------------------------------------------------------

/// (year, month, day) from days since 1970-01-01 (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since 1970-01-01 from a civil (year, month, day).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Ticks-per-second for a canonical [`TimeUnit`].
fn unit_per_second(unit: DmTimeUnit) -> i64 {
    match unit {
        DmTimeUnit::Second => 1,
        DmTimeUnit::Millisecond => 1_000,
        DmTimeUnit::Microsecond => 1_000_000,
        DmTimeUnit::Nanosecond => 1_000_000_000,
    }
}

/// Decompose a temporal ProximaValue into (days since epoch, seconds within
/// the day as f64 including fractional part). `Date` has no time component.
fn temporal_parts(name: &str, v: &ProximaValue) -> Result<(i64, f64), ExprError> {
    match v {
        ProximaValue::Date(days) => Ok((*days as i64, 0.0)),
        ProximaValue::Timestamp(ticks, unit) | ProximaValue::TimestampTz(ticks, unit) => {
            let per_sec = unit_per_second(*unit);
            let per_day = per_sec * 86_400;
            let days = ticks.div_euclid(per_day);
            let rem = ticks.rem_euclid(per_day);
            Ok((days, rem as f64 / per_sec as f64))
        }
        other => Err(ExprError::Other(format!(
            "{name}: expected a date/timestamp argument, got {other:?}"
        ))),
    }
}

/// Register the builtin scalar function set (the canonical names the SQL frontend and the
/// DataFusion adapter both resolve against).
pub fn register_builtin_scalars(reg: &ProximaFunctionRegistry) {
    fn def(
        name: &str,
        return_ty: ProximaType,
        kernel: impl Fn(&[ProximaValue]) -> Result<ProximaValue, ExprError> + Send + Sync + 'static,
    ) -> ScalarFunctionDef {
        ScalarFunctionDef {
            signature: FunctionSignature {
                name: name.to_string(),
                arg_types: Vec::new(),
                variadic: false,
                return_ty,
                volatility: Volatility::Immutable,
            },
            kernel: Arc::new(kernel),
        }
    }

    // upper / lower
    reg.register_scalar(def("upper", ProximaType::String, |args| {
        check_arity("upper", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::String(
            as_str("upper", &args[0])?.to_uppercase(),
        ))
    }));
    reg.register_scalar(def("lower", ProximaType::String, |args| {
        check_arity("lower", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::String(
            as_str("lower", &args[0])?.to_lowercase(),
        ))
    }));

    // length (character count)
    reg.register_scalar(def("length", ProximaType::Int64, |args| {
        check_arity("length", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::Int64(
            as_str("length", &args[0])?.chars().count() as i64,
        ))
    }));

    // abs — preserves integer type, else Float64
    reg.register_scalar(def("abs", ProximaType::Float64, |args| {
        check_arity("abs", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(match &args[0] {
            ProximaValue::Int64(i) => ProximaValue::Int64(i.abs()),
            ProximaValue::Int32(i) => ProximaValue::Int32(i.abs()),
            v => ProximaValue::Float64(as_f64("abs", v)?.abs()),
        })
    }));

    // ceil / floor / sqrt → Float64
    reg.register_scalar(def("ceil", ProximaType::Float64, |args| {
        check_arity("ceil", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::Float64(as_f64("ceil", &args[0])?.ceil()))
    }));
    reg.register_scalar(def("floor", ProximaType::Float64, |args| {
        check_arity("floor", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::Float64(as_f64("floor", &args[0])?.floor()))
    }));
    reg.register_scalar(def("sqrt", ProximaType::Float64, |args| {
        check_arity("sqrt", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::Float64(as_f64("sqrt", &args[0])?.sqrt()))
    }));

    // concat — variadic strings
    reg.register_scalar(ScalarFunctionDef {
        signature: FunctionSignature {
            name: "concat".to_string(),
            arg_types: Vec::new(),
            variadic: true,
            return_ty: ProximaType::String,
            volatility: Volatility::Immutable,
        },
        kernel: Arc::new(|args| {
            if any_null(args) {
                return Ok(ProximaValue::Null);
            }
            let mut out = String::new();
            for a in args {
                out.push_str(as_str("concat", a)?);
            }
            Ok(ProximaValue::String(out))
        }),
    });

    // JSON extraction — the engine-neutral counterpart of the DataFusion Arrow UDFs in
    // src/datafusion/udf.rs, so JSON-path projections/filters answer on the native
    // (Volcano) route too, and the frontend learns their return types (ADR-043 Inv 3).
    //   * json_extract       (`->`)  → the sub-value AS Json (structured), so downstream
    //                                  comparisons coerce by its natural type, cast-free.
    //   * json_extract_text  (`->>`) → the sub-value AS plain text (strings unquoted).
    // These names are also in datafusion::registry_udf::DATAFUSION_NATIVE_SCALARS so the
    // F2 adapter does NOT bind them — the typed Arrow UDFs stay authoritative on OLAP.
    reg.register_scalar(def("json_extract", ProximaType::Json, |args| {
        json_extract_native("json_extract", args, false)
    }));
    reg.register_scalar(def("json_extract_text", ProximaType::String, |args| {
        json_extract_native("json_extract_text", args, true)
    }));

    // ------------------------------------------------------------------
    // ANSI tranche 1 (TD-OLAP-5 P1b): string / math / conditional /
    // temporal scalars, PostgreSQL semantics. One ProximaValue kernel each
    // (ADR-039 invariant 4); the DataFusion route keeps its own vectorized
    // natives for these names (see DATAFUSION_NATIVE_SCALARS) — these
    // kernels make the same names answer on the native/Volcano route.
    // ------------------------------------------------------------------

    // trim family — 1-arg strips whitespace; 2-arg (btrim/ltrim/rtrim) strips
    // any character in the given set, per Postgres.
    fn trim_set(chars: Option<&str>) -> impl Fn(char) -> bool + '_ {
        move |c: char| match chars {
            Some(set) => set.contains(c),
            None => c.is_whitespace(),
        }
    }
    reg.register_scalar(def("trim", ProximaType::String, |args| {
        check_arity("trim", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::String(
            as_str("trim", &args[0])?.trim().to_string(),
        ))
    }));
    reg.register_scalar(def("btrim", ProximaType::String, |args| {
        check_arity_range("btrim", args, 1, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("btrim", &args[0])?;
        let set = args.get(1).map(|v| as_str("btrim", v)).transpose()?;
        Ok(ProximaValue::String(
            s.trim_matches(trim_set(set)).to_string(),
        ))
    }));
    reg.register_scalar(def("ltrim", ProximaType::String, |args| {
        check_arity_range("ltrim", args, 1, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("ltrim", &args[0])?;
        let set = args.get(1).map(|v| as_str("ltrim", v)).transpose()?;
        Ok(ProximaValue::String(
            s.trim_start_matches(trim_set(set)).to_string(),
        ))
    }));
    reg.register_scalar(def("rtrim", ProximaType::String, |args| {
        check_arity_range("rtrim", args, 1, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("rtrim", &args[0])?;
        let set = args.get(1).map(|v| as_str("rtrim", v)).transpose()?;
        Ok(ProximaValue::String(
            s.trim_end_matches(trim_set(set)).to_string(),
        ))
    }));

    // replace(string, from, to)
    reg.register_scalar(def("replace", ProximaType::String, |args| {
        check_arity("replace", args, 3)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("replace", &args[0])?;
        let from = as_str("replace", &args[1])?;
        let to = as_str("replace", &args[2])?;
        Ok(ProximaValue::String(if from.is_empty() {
            s.to_string()
        } else {
            s.replace(from, to)
        }))
    }));

    // strpos(string, substring) — 1-based char position, 0 when absent.
    reg.register_scalar(def("strpos", ProximaType::Int64, |args| {
        check_arity("strpos", args, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("strpos", &args[0])?;
        let sub = as_str("strpos", &args[1])?;
        let pos = match s.find(sub) {
            Some(byte_ix) => s[..byte_ix].chars().count() as i64 + 1,
            None => 0,
        };
        Ok(ProximaValue::Int64(pos))
    }));

    // substr(string, start [, count]) — 1-based, Postgres clamping semantics
    // (start may be <= 0; the window [start, start+count) intersects the string).
    reg.register_scalar(def("substr", ProximaType::String, |args| {
        check_arity_range("substr", args, 2, 3)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("substr", &args[0])?;
        let start = as_i64("substr", &args[1])?;
        let count = args.get(2).map(|v| as_i64("substr", v)).transpose()?;
        if let Some(c) = count
            && c < 0
        {
            return Err(ExprError::Other(
                "substr: negative substring length not allowed".to_string(),
            ));
        }
        let end = count.map(|c| start.saturating_add(c)); // exclusive, 1-based
        let from = start.max(1);
        let out: String = match end {
            Some(e) if e <= from => String::new(),
            Some(e) => s
                .chars()
                .skip((from - 1) as usize)
                .take((e - from) as usize)
                .collect(),
            None => s.chars().skip((from - 1) as usize).collect(),
        };
        Ok(ProximaValue::String(out))
    }));

    // lpad / rpad (string, length [, fill=' ']) — truncates when longer.
    fn pad(
        name: &'static str,
        left: bool,
        args: &[ProximaValue],
    ) -> Result<ProximaValue, ExprError> {
        check_arity_range(name, args, 2, 3)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str(name, &args[0])?;
        let len = as_i64(name, &args[1])?.max(0) as usize;
        let fill = args
            .get(1 + 1)
            .map(|v| as_str(name, v))
            .transpose()?
            .unwrap_or(" ");
        let cur = s.chars().count();
        if cur >= len {
            return Ok(ProximaValue::String(s.chars().take(len).collect()));
        }
        if fill.is_empty() {
            return Ok(ProximaValue::String(s.to_string()));
        }
        let padding: String = fill.chars().cycle().take(len - cur).collect();
        Ok(ProximaValue::String(if left {
            format!("{padding}{s}")
        } else {
            format!("{s}{padding}")
        }))
    }
    reg.register_scalar(def("lpad", ProximaType::String, |args| {
        pad("lpad", true, args)
    }));
    reg.register_scalar(def("rpad", ProximaType::String, |args| {
        pad("rpad", false, args)
    }));

    // left / right (string, n) — negative n drops from the other end (PG).
    reg.register_scalar(def("left", ProximaType::String, |args| {
        check_arity("left", args, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("left", &args[0])?;
        let n = as_i64("left", &args[1])?;
        let total = s.chars().count() as i64;
        let take = if n >= 0 { n } else { (total + n).max(0) };
        Ok(ProximaValue::String(
            s.chars().take(take as usize).collect(),
        ))
    }));
    reg.register_scalar(def("right", ProximaType::String, |args| {
        check_arity("right", args, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("right", &args[0])?;
        let n = as_i64("right", &args[1])?;
        let total = s.chars().count() as i64;
        let skip = if n >= 0 { (total - n).max(0) } else { -n };
        Ok(ProximaValue::String(
            s.chars().skip(skip as usize).collect(),
        ))
    }));

    // repeat(string, n) / reverse(string)
    reg.register_scalar(def("repeat", ProximaType::String, |args| {
        check_arity("repeat", args, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("repeat", &args[0])?;
        let n = as_i64("repeat", &args[1])?.max(0);
        if s.len().saturating_mul(n as usize) > 64 * 1024 * 1024 {
            return Err(ExprError::Other("repeat: result too large".to_string()));
        }
        Ok(ProximaValue::String(s.repeat(n as usize)))
    }));
    reg.register_scalar(def("reverse", ProximaType::String, |args| {
        check_arity("reverse", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::String(
            as_str("reverse", &args[0])?.chars().rev().collect(),
        ))
    }));

    // split_part(string, delimiter, n) — 1-based field; out of range → ''.
    // Negative n counts from the end (PG 14+).
    reg.register_scalar(def("split_part", ProximaType::String, |args| {
        check_arity("split_part", args, 3)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("split_part", &args[0])?;
        let delim = as_str("split_part", &args[1])?;
        let n = as_i64("split_part", &args[2])?;
        if n == 0 {
            return Err(ExprError::Other(
                "split_part: field position must not be zero".to_string(),
            ));
        }
        if delim.is_empty() {
            // PG: with an empty delimiter the string is a single field.
            let hit = n == 1 || n == -1;
            return Ok(ProximaValue::String(if hit {
                s.to_string()
            } else {
                String::new()
            }));
        }
        let parts: Vec<&str> = s.split(delim).collect();
        let ix = if n > 0 {
            (n - 1) as usize
        } else {
            match parts.len() as i64 + n {
                neg if neg < 0 => return Ok(ProximaValue::String(String::new())),
                ix => ix as usize,
            }
        };
        Ok(ProximaValue::String(
            parts.get(ix).copied().unwrap_or("").to_string(),
        ))
    }));

    // initcap — capitalize the first letter of every word (PG: words are
    // sequences of alphanumeric characters).
    reg.register_scalar(def("initcap", ProximaType::String, |args| {
        check_arity("initcap", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let s = as_str("initcap", &args[0])?;
        let mut out = String::with_capacity(s.len());
        let mut at_word_start = true;
        for c in s.chars() {
            if c.is_alphanumeric() {
                if at_word_start {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                at_word_start = false;
            } else {
                out.push(c);
                at_word_start = true;
            }
        }
        Ok(ProximaValue::String(out))
    }));

    // starts_with / ends_with → Boolean
    reg.register_scalar(def("starts_with", ProximaType::Boolean, |args| {
        check_arity("starts_with", args, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::Boolean(
            as_str("starts_with", &args[0])?.starts_with(as_str("starts_with", &args[1])?),
        ))
    }));
    reg.register_scalar(def("ends_with", ProximaType::Boolean, |args| {
        check_arity("ends_with", args, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::Boolean(
            as_str("ends_with", &args[0])?.ends_with(as_str("ends_with", &args[1])?),
        ))
    }));

    // ascii(string) — code point of the first character (0 for '').
    // chr(n) — the character with code point n.
    reg.register_scalar(def("ascii", ProximaType::Int64, |args| {
        check_arity("ascii", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::Int64(
            as_str("ascii", &args[0])?
                .chars()
                .next()
                .map(|c| c as i64)
                .unwrap_or(0),
        ))
    }));
    reg.register_scalar(def("chr", ProximaType::String, |args| {
        check_arity("chr", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let n = as_i64("chr", &args[0])?;
        let c = u32::try_from(n)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| ExprError::Other(format!("chr: {n} is not a valid code point")))?;
        Ok(ProximaValue::String(c.to_string()))
    }));

    // round(x [, digits]) / trunc(x [, digits]) — half-away-from-zero (PG).
    fn scale_op(
        name: &'static str,
        op: impl Fn(f64) -> f64 + Copy,
        args: &[ProximaValue],
    ) -> Result<ProximaValue, ExprError> {
        check_arity_range(name, args, 1, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let x = as_f64(name, &args[0])?;
        let digits = args
            .get(1)
            .map(|v| as_i64(name, v))
            .transpose()?
            .unwrap_or(0);
        let factor = 10f64.powi(digits.clamp(-300, 300) as i32);
        Ok(ProximaValue::Float64(op(x * factor) / factor))
    }
    reg.register_scalar(def("round", ProximaType::Float64, |args| {
        scale_op("round", f64::round, args)
    }));
    reg.register_scalar(def("trunc", ProximaType::Float64, |args| {
        scale_op("trunc", f64::trunc, args)
    }));

    // power / mod / sign / exp / ln / log10 / pi
    reg.register_scalar(def("power", ProximaType::Float64, |args| {
        check_arity("power", args, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::Float64(
            as_f64("power", &args[0])?.powf(as_f64("power", &args[1])?),
        ))
    }));
    reg.register_scalar(def("mod", ProximaType::Float64, |args| {
        check_arity("mod", args, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        if is_integer(&args[0]) && is_integer(&args[1]) {
            let a = as_i64("mod", &args[0])?;
            let b = as_i64("mod", &args[1])?;
            if b == 0 {
                return Err(ExprError::Other("mod: division by zero".to_string()));
            }
            // PG mod: sign follows the dividend (Rust % agrees).
            Ok(ProximaValue::Int64(a % b))
        } else {
            let a = as_f64("mod", &args[0])?;
            let b = as_f64("mod", &args[1])?;
            Ok(ProximaValue::Float64(a % b))
        }
    }));
    reg.register_scalar(def("sign", ProximaType::Float64, |args| {
        check_arity("sign", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        if is_integer(&args[0]) {
            Ok(ProximaValue::Int64(as_i64("sign", &args[0])?.signum()))
        } else {
            let x = as_f64("sign", &args[0])?;
            Ok(ProximaValue::Float64(if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }))
        }
    }));
    reg.register_scalar(def("exp", ProximaType::Float64, |args| {
        check_arity("exp", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        Ok(ProximaValue::Float64(as_f64("exp", &args[0])?.exp()))
    }));
    reg.register_scalar(def("ln", ProximaType::Float64, |args| {
        check_arity("ln", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let x = as_f64("ln", &args[0])?;
        if x <= 0.0 {
            return Err(ExprError::Other(
                "ln: cannot take logarithm of a non-positive number".to_string(),
            ));
        }
        Ok(ProximaValue::Float64(x.ln()))
    }));
    reg.register_scalar(def("log10", ProximaType::Float64, |args| {
        check_arity("log10", args, 1)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let x = as_f64("log10", &args[0])?;
        if x <= 0.0 {
            return Err(ExprError::Other(
                "log10: cannot take logarithm of a non-positive number".to_string(),
            ));
        }
        Ok(ProximaValue::Float64(x.log10()))
    }));
    reg.register_scalar(def("pi", ProximaType::Float64, |args| {
        check_arity("pi", args, 0)?;
        Ok(ProximaValue::Float64(std::f64::consts::PI))
    }));

    // greatest / least — variadic; PG semantics: NULL args are IGNORED,
    // NULL only when every arg is NULL. All-numeric compares numerically;
    // all-string compares lexically; mixed types error.
    fn extremum(
        name: &'static str,
        want_max: bool,
        args: &[ProximaValue],
    ) -> Result<ProximaValue, ExprError> {
        let live: Vec<&ProximaValue> = args
            .iter()
            .filter(|v| !matches!(v, ProximaValue::Null))
            .collect();
        if live.is_empty() {
            return Ok(ProximaValue::Null);
        }
        let all_numeric = live.iter().all(|v| as_f64(name, v).is_ok());
        if all_numeric {
            let mut best = live[0];
            let mut best_key = as_f64(name, best)?;
            for v in &live[1..] {
                let key = as_f64(name, v)?;
                if (want_max && key > best_key) || (!want_max && key < best_key) {
                    best = v;
                    best_key = key;
                }
            }
            return Ok((*best).clone());
        }
        let all_strings = live.iter().all(|v| as_str(name, v).is_ok());
        if all_strings {
            let mut best = live[0];
            for v in &live[1..] {
                let (a, b) = (as_str(name, v)?, as_str(name, best)?);
                if (want_max && a > b) || (!want_max && a < b) {
                    best = v;
                }
            }
            return Ok((*best).clone());
        }
        Err(ExprError::Other(format!(
            "{name}: arguments must be all-numeric or all-string"
        )))
    }
    reg.register_scalar(ScalarFunctionDef {
        signature: FunctionSignature {
            name: "greatest".to_string(),
            arg_types: Vec::new(),
            variadic: true,
            return_ty: ProximaType::Float64,
            volatility: Volatility::Immutable,
        },
        kernel: Arc::new(|args| extremum("greatest", true, args)),
    });
    reg.register_scalar(ScalarFunctionDef {
        signature: FunctionSignature {
            name: "least".to_string(),
            arg_types: Vec::new(),
            variadic: true,
            return_ty: ProximaType::Float64,
            volatility: Volatility::Immutable,
        },
        kernel: Arc::new(|args| extremum("least", false, args)),
    });

    // date_part(field, date|timestamp) → Float64, PG field names. The SQL
    // `EXTRACT(field FROM x)` syntax lowers to this name in the frontend.
    reg.register_scalar(def("date_part", ProximaType::Float64, |args| {
        check_arity("date_part", args, 2)?;
        if any_null(args) {
            return Ok(ProximaValue::Null);
        }
        let field = as_str("date_part", &args[0])?.to_ascii_lowercase();
        let (days, secs) = temporal_parts("date_part", &args[1])?;
        let (y, m, d) = civil_from_days(days);
        let out = match field.as_str() {
            "year" | "years" => y as f64,
            "month" | "months" => m as f64,
            "day" | "days" => d as f64,
            "quarter" => ((m - 1) / 3 + 1) as f64,
            // PG dow: 0 = Sunday. 1970-01-01 was a Thursday (4).
            "dow" => ((days + 4).rem_euclid(7)) as f64,
            "isodow" => (((days + 3).rem_euclid(7)) + 1) as f64,
            "doy" => (days - days_from_civil(y, 1, 1) + 1) as f64,
            "hour" | "hours" => (secs / 3600.0).floor(),
            "minute" | "minutes" => ((secs % 3600.0) / 60.0).floor(),
            "second" | "seconds" => secs % 60.0,
            "epoch" => days as f64 * 86_400.0 + secs,
            other => {
                return Err(ExprError::Other(format!(
                    "date_part: unsupported field '{other}'"
                )));
            }
        };
        Ok(ProximaValue::Float64(out))
    }));

    // date_trunc(field, timestamp|date) → Timestamp (microseconds).
    reg.register_scalar(def(
        "date_trunc",
        ProximaType::Timestamp(DmTimeUnit::Microsecond),
        |args| {
            check_arity("date_trunc", args, 2)?;
            if any_null(args) {
                return Ok(ProximaValue::Null);
            }
            let field = as_str("date_trunc", &args[0])?.to_ascii_lowercase();
            let (days, secs) = temporal_parts("date_trunc", &args[1])?;
            let (y, m, _d) = civil_from_days(days);
            let whole = secs as i64; // whole seconds within the day
            let (out_days, out_secs) = match field.as_str() {
                "year" | "years" => (days_from_civil(y, 1, 1), 0),
                "quarter" => (days_from_civil(y, ((m - 1) / 3) * 3 + 1, 1), 0),
                "month" | "months" => (days_from_civil(y, m, 1), 0),
                // ISO week: truncate to Monday.
                "week" => (days - (days + 3).rem_euclid(7), 0),
                "day" | "days" => (days, 0),
                "hour" | "hours" => (days, whole - whole % 3600),
                "minute" | "minutes" => (days, whole - whole % 60),
                "second" | "seconds" => (days, whole),
                other => {
                    return Err(ExprError::Other(format!(
                        "date_trunc: unsupported field '{other}'"
                    )));
                }
            };
            let micros = (out_days * 86_400 + out_secs) * 1_000_000;
            Ok(ProximaValue::Timestamp(micros, DmTimeUnit::Microsecond))
        },
    ));

    // Aliases (kept consistent with the DataFusion-side lowering).
    reg.alias_scalar("ucase", "upper");
    reg.alias_scalar("lcase", "lower");
    reg.alias_scalar("char_length", "length");
    reg.alias_scalar("character_length", "length");
    reg.alias_scalar("ceiling", "ceil");
    // Postgres `json_extract_path_text(doc, key)` is the text-extract form.
    reg.alias_scalar("json_extract_path_text", "json_extract_text");
    // ANSI tranche 1 aliases.
    reg.alias_scalar("substring", "substr");
    reg.alias_scalar("pow", "power");
    // Postgres single-arg `log(x)` is base-10.
    reg.alias_scalar("log", "log10");
    reg.alias_scalar("position", "strpos");
}

/// Extract `key` from a JSON document/array (engine-neutral; mirrors `extract_one` in
/// `src/datafusion/udf.rs`). `arg0` arrives as a parsed `Json` value (a relational JSON
/// column, the first extraction) or as JSON text (`String`, when chained —
/// `doc->'a'->>'b'` lowers to `json_extract_text(json_extract(doc,'a'), 'b')`). `as_text`
/// returns string leaves unquoted; `->` returns the sub-value AS `Json`. Any miss (bad
/// key, non-container, unparseable, JSON null) yields `Null`.
fn json_extract_native(
    name: &str,
    args: &[ProximaValue],
    as_text: bool,
) -> Result<ProximaValue, ExprError> {
    check_arity(name, args, 2)?;
    if any_null(args) {
        return Ok(ProximaValue::Null);
    }
    let key = as_str(name, &args[1])?;
    let parsed: serde_json::Value = match &args[0] {
        ProximaValue::Json(v) | ProximaValue::Jsonb(v) => v.clone(),
        ProximaValue::String(s) | ProximaValue::Symbol(s) => match serde_json::from_str(s) {
            Ok(v) => v,
            Err(_) => return Ok(ProximaValue::Null),
        },
        other => {
            return Err(ExprError::Other(format!(
                "{name}: expected a JSON or string argument, got {other:?}"
            )));
        }
    };
    let child = match &parsed {
        serde_json::Value::Object(map) => map.get(key),
        serde_json::Value::Array(arr) => key.parse::<usize>().ok().and_then(|i| arr.get(i)),
        _ => None,
    };
    let Some(child) = child else {
        return Ok(ProximaValue::Null);
    };
    if as_text {
        Ok(match child {
            serde_json::Value::String(s) => ProximaValue::String(s.clone()),
            serde_json::Value::Null => ProximaValue::Null,
            other => ProximaValue::String(other.to_string()),
        })
    } else {
        Ok(ProximaValue::Json(child.clone()))
    }
}

// =========================================================================
// Builtin aggregate kernels (engine-neutral, NULL-skipping)
// =========================================================================

/// `product(x)` — the running product of all non-NULL numeric args. NULL over an empty group.
/// A simple, non-native (not COUNT/SUM/AVG/MIN/MAX) demonstrator of the aggregate framework
/// that exercises update / state / merge / finalize.
struct ProductAccumulator {
    running: Option<f64>,
}

impl AggregateAccumulator for ProductAccumulator {
    fn update(&mut self, args: &[ProximaValue]) -> Result<(), ExprError> {
        let v = args.first().ok_or(ExprError::WrongFunctionArity {
            name: "product".to_string(),
            expected: 1,
            got: 0,
        })?;
        if matches!(v, ProximaValue::Null) {
            return Ok(());
        }
        let n = as_f64("product", v)?;
        self.running = Some(self.running.unwrap_or(1.0) * n);
        Ok(())
    }

    fn state(&self) -> Vec<ProximaValue> {
        // An empty partial state is represented as NULL so merge stays the identity.
        match self.running {
            Some(p) => vec![ProximaValue::Float64(p)],
            None => vec![ProximaValue::Null],
        }
    }

    fn merge(&mut self, state: &[ProximaValue]) -> Result<(), ExprError> {
        match state.first() {
            Some(ProximaValue::Null) | None => Ok(()),
            Some(other) => {
                let p = as_f64("product", other)?;
                self.running = Some(self.running.unwrap_or(1.0) * p);
                Ok(())
            }
        }
    }

    fn finalize(&self) -> ProximaValue {
        match self.running {
            Some(p) => ProximaValue::Float64(p),
            None => ProximaValue::Null,
        }
    }
}

struct ProductKernel;

impl AggregateKernel for ProductKernel {
    fn new_accumulator(&self) -> Box<dyn AggregateAccumulator> {
        Box::new(ProductAccumulator { running: None })
    }

    fn state_types(&self) -> Vec<ProximaType> {
        vec![ProximaType::Float64] // the running product
    }
}

/// Register the builtin aggregate functions.
pub fn register_builtin_aggregates(reg: &ProximaFunctionRegistry) {
    reg.register_aggregate(AggregateFunctionDef {
        signature: FunctionSignature {
            name: "product".to_string(),
            arg_types: vec![ProximaType::Float64],
            variadic: false,
            return_ty: ProximaType::Float64,
            volatility: Volatility::Immutable,
        },
        kernel: Arc::new(ProductKernel),
    });
}

// =========================================================================
// User functions (F5): SQL-expression-bodied scalar functions
// =========================================================================

/// Build a scalar function whose body is an engine-neutral [`Expr`] over its parameters —
/// parameter `i` is referenced inside the body as column ordinal `i`. At call time the argument
/// values are bound positionally to those ordinals and the body is evaluated through the SAME
/// `Expr` evaluator + registry the engines already use.
///
/// This is how a SQL-language `CREATE FUNCTION double(x) RETURNS BIGINT AS 'x * 2'` body executes
/// (F5): the DDL handler lowers the body SQL into an `Expr` against the parameter scope, then calls
/// this. The resulting kernel runs *identically* on both engines — Volcano dispatches it directly
/// (F1b), DataFusion wraps it via the `ScalarUDF` adapter (F2) — so a user function is defined once
/// and served everywhere, no per-engine reimplementation. Nested calls in the body resolve against
/// the global [`builtins`] registry (scalars, aggregates, and other user functions).
///
/// NOTE: this reuses the existing native machinery rather than a slow bespoke interpreter — the
/// body's standard functions still take each engine's native fast path at lowering time.
pub fn sql_bodied_scalar(
    name: &str,
    arg_types: Vec<ProximaType>,
    return_ty: ProximaType,
    body: Expr,
) -> ScalarFunctionDef {
    let body = Arc::new(body);
    ScalarFunctionDef {
        signature: FunctionSignature {
            name: name.to_string(),
            arg_types,
            variadic: false,
            return_ty,
            volatility: Volatility::Immutable,
        },
        kernel: Arc::new(move |args: &[ProximaValue]| {
            // The args ARE the parameter values, positionally == column ordinals 0..n.
            let row: RelationalRow = args.to_vec();
            body.eval(&row, builtins())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> ProximaFunctionRegistry {
        ProximaFunctionRegistry::with_builtins()
    }

    #[test]
    fn json_extract_native_handles_json_and_text_inputs() {
        use ProximaValue as V;
        let doc = V::Json(serde_json::json!({"price": 10, "title": "alpha"}));
        // `->` returns the structured sub-value AS Json (for cast-free coercion).
        let price = json_extract_native(
            "json_extract",
            &[doc.clone(), V::String("price".into())],
            false,
        )
        .unwrap();
        assert!(matches!(price, V::Json(serde_json::Value::Number(_))));
        // `->>` returns unquoted text.
        let title = json_extract_native(
            "json_extract_text",
            &[doc.clone(), V::String("title".into())],
            true,
        )
        .unwrap();
        assert_eq!(title, V::String("alpha".into()));
        // Chained: arg0 arrives as JSON TEXT.
        let chained = json_extract_native(
            "json_extract_text",
            &[V::String(r#"{"k":"v"}"#.into()), V::String("k".into())],
            true,
        )
        .unwrap();
        assert_eq!(chained, V::String("v".into()));
        // Missing key → Null (not an error).
        let missing =
            json_extract_native("json_extract", &[doc, V::String("nope".into())], false).unwrap();
        assert_eq!(missing, V::Null);
    }

    #[test]
    fn json_functions_are_registered_with_typed_returns() {
        let r = reg();
        assert_eq!(
            r.lookup_scalar("json_extract").unwrap().signature.return_ty,
            ProximaType::Json
        );
        assert_eq!(
            r.lookup_scalar("json_extract_text")
                .unwrap()
                .signature
                .return_ty,
            ProximaType::String
        );
        // alias resolves to the text-extract form
        assert!(r.lookup_scalar("json_extract_path_text").is_some());
    }

    #[test]
    fn dispatch_scalar_builtins() {
        let r = reg();
        // upper / lower / length
        assert_eq!(
            r.dispatch("upper", &[ProximaValue::String("aB".into())])
                .unwrap()
                .unwrap(),
            ProximaValue::String("AB".into())
        );
        assert_eq!(
            r.dispatch("LOWER", &[ProximaValue::String("aB".into())])
                .unwrap()
                .unwrap(),
            ProximaValue::String("ab".into())
        );
        assert_eq!(
            r.dispatch("length", &[ProximaValue::String("abc".into())])
                .unwrap()
                .unwrap(),
            ProximaValue::Int64(3)
        );
    }

    #[test]
    fn dispatch_numeric_and_concat() {
        let r = reg();
        assert_eq!(
            r.dispatch("abs", &[ProximaValue::Int64(-5)])
                .unwrap()
                .unwrap(),
            ProximaValue::Int64(5)
        );
        assert_eq!(
            r.dispatch("sqrt", &[ProximaValue::Float64(9.0)])
                .unwrap()
                .unwrap(),
            ProximaValue::Float64(3.0)
        );
        assert_eq!(
            r.dispatch(
                "concat",
                &[
                    ProximaValue::String("a".into()),
                    ProximaValue::String("b".into()),
                    ProximaValue::String("c".into())
                ]
            )
            .unwrap()
            .unwrap(),
            ProximaValue::String("abc".into())
        );
    }

    #[test]
    fn null_propagates_and_aliases_resolve() {
        let r = reg();
        assert_eq!(
            r.dispatch("upper", &[ProximaValue::Null]).unwrap().unwrap(),
            ProximaValue::Null
        );
        // alias ucase == upper
        assert_eq!(
            r.dispatch("ucase", &[ProximaValue::String("x".into())])
                .unwrap()
                .unwrap(),
            ProximaValue::String("X".into())
        );
    }

    /// Regression: `alias_scalar` must not hold the target's `DashMap` read-guard
    /// across the alias `insert` — that self-deadlocks when both keys hash to the
    /// same shard (random per process, so it was a ~1-2% intermittent hang). Many
    /// aliases over one target make a same-shard collision near-certain, so this
    /// would wedge a thread WITHOUT the fix and completes instantly WITH it.
    #[test]
    fn alias_scalar_is_reentrancy_safe_under_shard_collisions() {
        let r = ProximaFunctionRegistry::default();
        r.register_scalar(ScalarFunctionDef {
            signature: FunctionSignature {
                name: "base".to_string(),
                arg_types: vec![ProximaType::String],
                variadic: false,
                return_ty: ProximaType::String,
                volatility: Volatility::Immutable,
            },
            kernel: Arc::new(|args: &[ProximaValue]| Ok(args[0].clone())),
        });
        // Enough aliases that two are overwhelmingly likely to share a shard.
        for i in 0..256 {
            r.alias_scalar(&format!("base_alias_{i}"), "base");
        }
        assert_eq!(r.scalar_count(), 257);
        assert!(r.lookup_scalar("base_alias_200").is_some());
        // Aliasing a missing target is a no-op (still no guard held across insert).
        r.alias_scalar("orphan", "does_not_exist");
        assert!(r.lookup_scalar("orphan").is_none());
    }

    #[test]
    fn unknown_function_is_none_and_arity_errors() {
        let r = reg();
        assert!(r.dispatch("no_such_fn", &[]).is_none());
        let err = r
            .dispatch(
                "upper",
                &[
                    ProximaValue::String("a".into()),
                    ProximaValue::String("b".into()),
                ],
            )
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            err,
            ExprError::WrongFunctionArity {
                expected: 1,
                got: 2,
                ..
            }
        ));
    }

    #[test]
    fn product_aggregate_update_finalize_and_null_skip() {
        let def = reg().lookup_aggregate("PRODUCT").expect("product builtin");
        assert_eq!(def.signature.return_ty, ProximaType::Float64);
        let mut acc = def.kernel.new_accumulator();
        // empty group → NULL
        assert_eq!(acc.finalize(), ProximaValue::Null);
        // 2 * 3 * 4 = 24, NULL skipped
        acc.update(&[ProximaValue::Float64(2.0)]).unwrap();
        acc.update(&[ProximaValue::Null]).unwrap();
        acc.update(&[ProximaValue::Int64(3)]).unwrap();
        acc.update(&[ProximaValue::Float64(4.0)]).unwrap();
        assert_eq!(acc.finalize(), ProximaValue::Float64(24.0));
    }

    #[test]
    fn product_aggregate_merges_partial_states() {
        let def = reg().lookup_aggregate("product").unwrap();
        // partition A: 2 * 5 = 10
        let mut a = def.kernel.new_accumulator();
        a.update(&[ProximaValue::Float64(2.0)]).unwrap();
        a.update(&[ProximaValue::Float64(5.0)]).unwrap();
        // partition B: 3
        let mut b = def.kernel.new_accumulator();
        b.update(&[ProximaValue::Float64(3.0)]).unwrap();
        // coordinator folds B's state into A → 10 * 3 = 30
        a.merge(&b.state()).unwrap();
        assert_eq!(a.finalize(), ProximaValue::Float64(30.0));
        // merging an empty (NULL) state is the identity
        let empty = def.kernel.new_accumulator();
        a.merge(&empty.state()).unwrap();
        assert_eq!(a.finalize(), ProximaValue::Float64(30.0));
    }

    #[test]
    fn sql_bodied_scalar_evaluates_arithmetic_body() {
        use proximadb_relational_types::{BinaryOp, ColumnRef};
        // F5: double(x) AS 'x * 2' — body Expr over parameter ordinal 0.
        let body = Expr::bin(
            BinaryOp::Mul,
            Expr::column(ColumnRef {
                name: "x".into(),
                ordinal: 0,
                ty: ProximaType::Int64,
                nullable: false,
            }),
            Expr::literal(ProximaValue::Int64(2)),
        );
        let r = reg();
        r.register_scalar(sql_bodied_scalar(
            "double",
            vec![ProximaType::Int64],
            ProximaType::Int64,
            body,
        ));
        assert_eq!(
            r.dispatch("double", &[ProximaValue::Int64(21)])
                .unwrap()
                .unwrap(),
            ProximaValue::Int64(42)
        );
    }

    #[test]
    fn sql_bodied_scalar_body_calls_builtin() {
        use proximadb_relational_types::ColumnRef;
        // F5: myabs(x) AS 'abs(x)' — the body's nested call resolves through the builtin
        // registry, proving user functions compose with the rest of the function surface.
        let body = Expr::FuncCall {
            name: "abs".into(),
            args: vec![Expr::column(ColumnRef {
                name: "x".into(),
                ordinal: 0,
                ty: ProximaType::Float64,
                nullable: false,
            })],
            return_ty: ProximaType::Float64,
        };
        let r = reg();
        r.register_scalar(sql_bodied_scalar(
            "myabs",
            vec![ProximaType::Float64],
            ProximaType::Float64,
            body,
        ));
        assert_eq!(
            r.dispatch("myabs", &[ProximaValue::Float64(-3.5)])
                .unwrap()
                .unwrap(),
            ProximaValue::Float64(3.5)
        );
    }

    // -----------------------------------------------------------------
    // ANSI tranche 1 (TD-OLAP-5 P1b)
    // -----------------------------------------------------------------

    fn s(v: &str) -> ProximaValue {
        ProximaValue::String(v.to_string())
    }
    fn i(v: i64) -> ProximaValue {
        ProximaValue::Int64(v)
    }
    fn f(v: f64) -> ProximaValue {
        ProximaValue::Float64(v)
    }
    fn call(r: &ProximaFunctionRegistry, name: &str, args: &[ProximaValue]) -> ProximaValue {
        r.dispatch(name, args)
            .unwrap_or_else(|| panic!("{name} not registered"))
            .unwrap_or_else(|e| panic!("{name} errored: {e:?}"))
    }

    /// The ANSI coverage ratchet: the builtin scalar surface only grows.
    /// Raise the floor when adding functions; NEVER lower it (mirrors the
    /// TPC conformance ratchets, mandate #10).
    #[test]
    fn ansi_scalar_coverage_ratchet() {
        let r = reg();
        assert!(
            r.scalar_count() >= 52,
            "builtin scalar surface regressed: {} < 52 (10 base + 32 tranche-1 + 10 aliases)",
            r.scalar_count()
        );
        for name in [
            // tranche 1 — string
            "trim",
            "btrim",
            "ltrim",
            "rtrim",
            "replace",
            "strpos",
            "substr",
            "lpad",
            "rpad",
            "left",
            "right",
            "repeat",
            "reverse",
            "split_part",
            "initcap",
            "starts_with",
            "ends_with",
            "ascii",
            "chr",
            // tranche 1 — math
            "round",
            "trunc",
            "power",
            "mod",
            "sign",
            "exp",
            "ln",
            "log10",
            "pi",
            // tranche 1 — conditional + temporal
            "greatest",
            "least",
            "date_part",
            "date_trunc",
            // tranche 1 — aliases
            "substring",
            "pow",
            "log",
            "position",
        ] {
            assert!(
                r.lookup_scalar(name).is_some(),
                "ratcheted builtin '{name}' missing"
            );
        }
    }

    #[test]
    fn trim_family_pg_semantics() {
        let r = reg();
        assert_eq!(call(&r, "trim", &[s("  hi  ")]), s("hi"));
        assert_eq!(call(&r, "btrim", &[s("xxhixx"), s("x")]), s("hi"));
        assert_eq!(call(&r, "ltrim", &[s("  hi  ")]), s("hi  "));
        assert_eq!(call(&r, "rtrim", &[s("xyhixy"), s("yx")]), s("xyhi"));
        assert_eq!(call(&r, "trim", &[ProximaValue::Null]), ProximaValue::Null);
    }

    #[test]
    fn string_kernels_pg_semantics() {
        let r = reg();
        assert_eq!(
            call(&r, "replace", &[s("abcabc"), s("b"), s("XY")]),
            s("aXYcaXYc")
        );
        assert_eq!(call(&r, "strpos", &[s("high"), s("ig")]), i(2));
        assert_eq!(call(&r, "strpos", &[s("high"), s("zz")]), i(0));
        // substr: PG clamping — negative start intersects the window.
        assert_eq!(call(&r, "substr", &[s("alphabet"), i(3)]), s("phabet"));
        assert_eq!(call(&r, "substr", &[s("alphabet"), i(3), i(2)]), s("ph"));
        assert_eq!(call(&r, "substr", &[s("alphabet"), i(-2), i(5)]), s("al"));
        assert_eq!(call(&r, "substring", &[s("alphabet"), i(3), i(2)]), s("ph"));
        assert_eq!(call(&r, "lpad", &[s("hi"), i(5), s("xy")]), s("xyxhi"));
        assert_eq!(call(&r, "lpad", &[s("hi"), i(1)]), s("h"), "lpad truncates");
        assert_eq!(call(&r, "rpad", &[s("hi"), i(4)]), s("hi  "));
        assert_eq!(call(&r, "left", &[s("hello"), i(2)]), s("he"));
        assert_eq!(call(&r, "left", &[s("hello"), i(-2)]), s("hel"));
        assert_eq!(call(&r, "right", &[s("hello"), i(2)]), s("lo"));
        assert_eq!(call(&r, "right", &[s("hello"), i(-2)]), s("llo"));
        assert_eq!(call(&r, "repeat", &[s("ab"), i(3)]), s("ababab"));
        assert_eq!(call(&r, "reverse", &[s("abc")]), s("cba"));
        assert_eq!(call(&r, "split_part", &[s("a,b,c"), s(","), i(2)]), s("b"));
        assert_eq!(call(&r, "split_part", &[s("a,b,c"), s(","), i(-1)]), s("c"));
        assert_eq!(call(&r, "split_part", &[s("a,b,c"), s(","), i(9)]), s(""));
        assert_eq!(
            call(&r, "initcap", &[s("hello WORLD-of sql")]),
            s("Hello World-Of Sql")
        );
        assert_eq!(
            call(&r, "starts_with", &[s("alphabet"), s("alph")]),
            ProximaValue::Boolean(true)
        );
        assert_eq!(
            call(&r, "ends_with", &[s("alphabet"), s("bet")]),
            ProximaValue::Boolean(true)
        );
        assert_eq!(call(&r, "ascii", &[s("Abc")]), i(65));
        assert_eq!(call(&r, "ascii", &[s("")]), i(0));
        assert_eq!(call(&r, "chr", &[i(65)]), s("A"));
    }

    #[test]
    fn math_kernels_pg_semantics() {
        let r = reg();
        assert_eq!(call(&r, "round", &[f(2.5)]), f(3.0), "half away from zero");
        assert_eq!(call(&r, "round", &[f(-2.5)]), f(-3.0));
        assert_eq!(call(&r, "round", &[f(1.2345), i(2)]), f(1.23));
        assert_eq!(call(&r, "trunc", &[f(1.999)]), f(1.0));
        assert_eq!(call(&r, "trunc", &[f(-1.999)]), f(-1.0));
        assert_eq!(call(&r, "power", &[f(2.0), f(10.0)]), f(1024.0));
        assert_eq!(call(&r, "pow", &[f(2.0), f(3.0)]), f(8.0));
        assert_eq!(call(&r, "mod", &[i(7), i(3)]), i(1));
        assert_eq!(call(&r, "mod", &[i(-7), i(3)]), i(-1), "sign of dividend");
        assert!(r.dispatch("mod", &[i(1), i(0)]).unwrap().is_err());
        assert_eq!(call(&r, "sign", &[i(-9)]), i(-1));
        assert_eq!(call(&r, "sign", &[f(0.0)]), f(0.0));
        assert_eq!(call(&r, "exp", &[f(0.0)]), f(1.0));
        assert_eq!(call(&r, "ln", &[f(1.0)]), f(0.0));
        assert!(r.dispatch("ln", &[f(0.0)]).unwrap().is_err());
        assert_eq!(call(&r, "log10", &[f(1000.0)]), f(3.0));
        assert_eq!(call(&r, "log", &[f(100.0)]), f(2.0), "PG log = base 10");
        assert_eq!(call(&r, "pi", &[]), f(std::f64::consts::PI));
    }

    #[test]
    fn greatest_least_ignore_nulls() {
        let r = reg();
        assert_eq!(
            call(&r, "greatest", &[i(1), ProximaValue::Null, i(7), f(3.5)]),
            i(7)
        );
        assert_eq!(
            call(&r, "least", &[s("pear"), s("apple"), ProximaValue::Null]),
            s("apple")
        );
        assert_eq!(
            call(&r, "greatest", &[ProximaValue::Null, ProximaValue::Null]),
            ProximaValue::Null,
            "NULL only when all args are NULL"
        );
        assert!(r.dispatch("greatest", &[i(1), s("x")]).unwrap().is_err());
    }

    #[test]
    fn date_part_and_trunc_civil_arithmetic() {
        let r = reg();
        // 2000-03-01 was a Wednesday (PG dow: Sunday=0 → 3).
        let date = ProximaValue::Date(days_from_civil(2000, 3, 1) as i32);
        assert_eq!(call(&r, "date_part", &[s("year"), date.clone()]), f(2000.0));
        assert_eq!(call(&r, "date_part", &[s("month"), date.clone()]), f(3.0));
        assert_eq!(call(&r, "date_part", &[s("day"), date.clone()]), f(1.0));
        assert_eq!(call(&r, "date_part", &[s("quarter"), date.clone()]), f(1.0));
        assert_eq!(call(&r, "date_part", &[s("dow"), date.clone()]), f(3.0));
        assert_eq!(
            call(&r, "date_part", &[s("doy"), date.clone()]),
            f(61.0),
            "2000 is a leap year"
        );
        // Epoch day zero: 1970-01-01, a Thursday.
        let epoch = ProximaValue::Date(0);
        assert_eq!(call(&r, "date_part", &[s("dow"), epoch.clone()]), f(4.0));
        assert_eq!(call(&r, "date_part", &[s("epoch"), epoch]), f(0.0));

        // Timestamp: 2021-06-15 12:34:56 UTC (microseconds).
        let secs = days_from_civil(2021, 6, 15) * 86_400 + 12 * 3600 + 34 * 60 + 56;
        let ts = ProximaValue::Timestamp(secs * 1_000_000, DmTimeUnit::Microsecond);
        assert_eq!(call(&r, "date_part", &[s("hour"), ts.clone()]), f(12.0));
        assert_eq!(call(&r, "date_part", &[s("minute"), ts.clone()]), f(34.0));
        assert_eq!(call(&r, "date_part", &[s("second"), ts.clone()]), f(56.0));

        // date_trunc to month → 2021-06-01 00:00:00.
        let truncated = call(&r, "date_trunc", &[s("month"), ts.clone()]);
        let expected = days_from_civil(2021, 6, 1) * 86_400 * 1_000_000;
        assert_eq!(
            truncated,
            ProximaValue::Timestamp(expected, DmTimeUnit::Microsecond)
        );
        // date_trunc to hour keeps the day, zeroes minutes/seconds.
        let by_hour = call(&r, "date_trunc", &[s("hour"), ts]);
        let expected_hour = (days_from_civil(2021, 6, 15) * 86_400 + 12 * 3600) * 1_000_000;
        assert_eq!(
            by_hour,
            ProximaValue::Timestamp(expected_hour, DmTimeUnit::Microsecond)
        );
        // Unsupported field is a clear error, not a wrong answer.
        assert!(
            r.dispatch("date_part", &[s("fortnight"), ProximaValue::Date(0)])
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn civil_date_roundtrip() {
        // Round-trip across leap years, century boundaries, and the epoch.
        for days in [-719_468i64, -1, 0, 1, 10_957, 11_016, 18_993, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "roundtrip for {days}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
    }
}

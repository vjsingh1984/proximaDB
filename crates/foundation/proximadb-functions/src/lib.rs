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
use proximadb_data_model::{ProximaType, ProximaValue};
use proximadb_relational_types::{ExprError, FunctionRegistry};

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

/// The canonical function registry. Built once at startup with builtins and (later) user
/// functions from the xCatalog function catalog; read concurrently by every engine.
///
/// `DashMap`-backed (mirroring `proximadb_rank_core::BlueprintFactory`) so runtime
/// registration (`CREATE FUNCTION`, F5) needs no `&mut self`.
#[derive(Default)]
pub struct ProximaFunctionRegistry {
    scalars: DashMap<String, Arc<ScalarFunctionDef>>,
}

impl ProximaFunctionRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry preloaded with the builtin scalar functions.
    pub fn with_builtins() -> Self {
        let reg = Self::new();
        register_builtin_scalars(&reg);
        reg
    }

    /// Register (or replace) a scalar function under its (lower-cased) name.
    pub fn register_scalar(&self, def: ScalarFunctionDef) {
        let key = def.signature.name.to_ascii_lowercase();
        self.scalars.insert(key, Arc::new(def));
    }

    /// Register the same scalar under an alias name (e.g. `ucase` → `upper`).
    pub fn alias_scalar(&self, alias: &str, target: &str) {
        if let Some(def) = self.scalars.get(&target.to_ascii_lowercase()) {
            self.scalars.insert(alias.to_ascii_lowercase(), def.clone());
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

    // Aliases (kept consistent with the DataFusion-side lowering).
    reg.alias_scalar("ucase", "upper");
    reg.alias_scalar("lcase", "lower");
    reg.alias_scalar("char_length", "length");
    reg.alias_scalar("character_length", "length");
    reg.alias_scalar("ceiling", "ceil");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> ProximaFunctionRegistry {
        ProximaFunctionRegistry::with_builtins()
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
}

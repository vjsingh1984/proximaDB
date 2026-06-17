//! Profile validation + inheritance resolution.

use crate::spec::{PhaseSpec, RankProfileSpec};
use proximadb_rank_core::{BlueprintFactory, QueryContext, RankError, RankResult};
use proximadb_rank_expr::ExprBlueprint;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Maximum depth of an inheritance chain. Beyond this we assume the user
/// has a structural problem rather than a legitimate need.
pub const MAX_INHERITANCE_DEPTH: usize = 8;

/// Validate a (fully-resolved) profile against a [`BlueprintFactory`].
///
/// Checks: expressions parse and lower (which catches unknown features
/// and built-in arity errors); `rerank_count ≤ heap_size`; budgets
/// strictly positive if specified; profile has at least a first phase.
///
/// Does not resolve inheritance — call [`resolve_inheritance`] first.
pub fn validate(spec: &RankProfileSpec, factory: &Arc<BlueprintFactory>) -> RankResult<()> {
    if spec.first_phase.is_none() {
        return Err(RankError::InvalidProfile(format!(
            "profile '{}': must have at least a first_phase",
            spec.name
        )));
    }
    let bp = ExprBlueprint::new(factory.clone());
    let qctx = QueryContext::default();

    if let Some(p) = &spec.first_phase {
        check_phase("first_phase", p, &bp, &qctx, &spec.name)?;
    }
    if let Some(p) = &spec.second_phase {
        check_phase("second_phase", p, &bp, &qctx, &spec.name)?;
    }

    if let (Some(fp), Some(sp)) = (&spec.first_phase, &spec.second_phase)
        && let (Some(heap), Some(rerank)) = (fp.heap_size, sp.rerank_count)
        && rerank > heap
    {
        return Err(RankError::InvalidProfile(format!(
            "profile '{}': second_phase.rerank_count ({}) must be <= first_phase.heap_size ({})",
            spec.name, rerank, heap
        )));
    }

    for (label, b) in [
        ("first_max_us", spec.budget.first_max_us),
        ("second_max_us", spec.budget.second_max_us),
        ("global_max_us", spec.budget.global_max_us),
    ] {
        if matches!(b, Some(v) if v == 0) {
            return Err(RankError::InvalidProfile(format!(
                "profile '{}': budget.{label} must be > 0 when set",
                spec.name
            )));
        }
    }

    // Match-feature + summary-feature expressions parse, too. We lower
    // them through the same path; the resulting executors are discarded.
    for (label, list) in [
        ("match_features", &spec.match_features),
        ("summary_features", &spec.summary_features),
    ] {
        for expr in list {
            bp.compile_str(expr, &qctx).map_err(|e| {
                RankError::InvalidProfile(format!(
                    "profile '{}': {label} '{expr}' did not validate: {e}",
                    spec.name
                ))
            })?;
        }
    }

    // User-defined functions are persisted but not yet wired into the VM
    // (R-9 will). Still parse the expressions so a bad function bodies
    // surfaces at profile-create time.
    for f in &spec.functions {
        bp.compile_str(&f.expression, &qctx).map_err(|e| {
            RankError::InvalidProfile(format!(
                "profile '{}': function '{}' did not validate: {e}",
                spec.name, f.name
            ))
        })?;
    }

    Ok(())
}

fn check_phase(
    label: &str,
    phase: &PhaseSpec,
    bp: &ExprBlueprint,
    qctx: &QueryContext,
    profile_name: &str,
) -> RankResult<()> {
    bp.compile_str(&phase.expression, qctx).map_err(|e| {
        RankError::InvalidProfile(format!(
            "profile '{profile_name}': {label} expression did not validate: {e}"
        ))
    })?;
    if let Some(h) = phase.heap_size
        && h == 0
    {
        return Err(RankError::InvalidProfile(format!(
            "profile '{profile_name}': {label}.heap_size must be > 0"
        )));
    }
    if let Some(r) = phase.rerank_count
        && r == 0
    {
        return Err(RankError::InvalidProfile(format!(
            "profile '{profile_name}': {label}.rerank_count must be > 0"
        )));
    }
    Ok(())
}

/// Walk the `inherits` chain and merge parent fields into `spec`.
///
/// Merge rules (child overrides parent):
/// - `first_phase`/`second_phase`/`global_phase`: child wins if `Some`.
/// - `match_features`/`summary_features`: child wins if non-empty.
/// - `budget`: per-field merge — child wins per-field if `Some`.
/// - `functions` and `constants`: union by name; child overrides parent
///   for matching names; remaining parent entries are inherited.
///
/// Cycle detection and a hard cap at [`MAX_INHERITANCE_DEPTH`].
pub fn resolve_inheritance(
    spec: RankProfileSpec,
    known: &HashMap<String, RankProfileSpec>,
) -> RankResult<RankProfileSpec> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut cur = spec;
    visited.insert(cur.name.clone());
    let mut depth = 0;
    while let Some(parent_name) = cur.inherits.clone() {
        if !visited.insert(parent_name.clone()) {
            return Err(RankError::InvalidProfile(format!(
                "inheritance cycle detected involving '{parent_name}'"
            )));
        }
        depth += 1;
        if depth > MAX_INHERITANCE_DEPTH {
            return Err(RankError::InvalidProfile(format!(
                "inheritance chain for '{}' exceeds max depth {MAX_INHERITANCE_DEPTH}",
                cur.name
            )));
        }
        let parent = known.get(&parent_name).ok_or_else(|| {
            RankError::InvalidProfile(format!(
                "profile '{}': parent '{parent_name}' not found",
                cur.name
            ))
        })?;
        cur = merge(cur, parent.clone());
    }
    cur.inherits = None;
    Ok(cur)
}

fn merge(child: RankProfileSpec, parent: RankProfileSpec) -> RankProfileSpec {
    use crate::spec::{ConstantSpec, FunctionSpec, PhaseBudgetSpec};

    fn merge_budget(c: PhaseBudgetSpec, p: PhaseBudgetSpec) -> PhaseBudgetSpec {
        PhaseBudgetSpec {
            first_max_us: c.first_max_us.or(p.first_max_us),
            second_max_us: c.second_max_us.or(p.second_max_us),
            global_max_us: c.global_max_us.or(p.global_max_us),
        }
    }

    fn merge_by_name<T, F>(child: Vec<T>, parent: Vec<T>, name_of: F) -> Vec<T>
    where
        F: Fn(&T) -> &str,
    {
        let mut out: Vec<T> = Vec::new();
        let mut child_names: HashSet<String> = HashSet::new();
        for item in child {
            child_names.insert(name_of(&item).to_string());
            out.push(item);
        }
        for item in parent {
            if !child_names.contains(name_of(&item)) {
                out.push(item);
            }
        }
        out
    }

    RankProfileSpec {
        name: child.name,
        // Continue walking — current chain step's parent (`parent.inherits`)
        // becomes the next iteration's lookup target.
        inherits: parent.inherits.clone(),
        description: child.description.or(parent.description),
        first_phase: child.first_phase.or(parent.first_phase),
        second_phase: child.second_phase.or(parent.second_phase),
        global_phase: child.global_phase.or(parent.global_phase),
        match_features: if child.match_features.is_empty() {
            parent.match_features
        } else {
            child.match_features
        },
        summary_features: if child.summary_features.is_empty() {
            parent.summary_features
        } else {
            child.summary_features
        },
        budget: merge_budget(child.budget, parent.budget),
        functions: merge_by_name::<FunctionSpec, _>(child.functions, parent.functions, |f| &f.name),
        constants: merge_by_name::<ConstantSpec, _>(child.constants, parent.constants, |c| &c.name),
        version: child.version,
        created_at_ms: child.created_at_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ConstantSpec, FunctionSpec, PhaseBudgetSpec, PhaseSpec};
    use proximadb_rank_features::register_builtins;

    fn factory() -> Arc<BlueprintFactory> {
        let f = Arc::new(BlueprintFactory::new());
        register_builtins(&f);
        f
    }

    fn minimal(name: &str) -> RankProfileSpec {
        let mut s = RankProfileSpec::new(name);
        s.first_phase = Some(PhaseSpec {
            expression: "bm25(\"title\")".into(),
            heap_size: Some(100),
            rerank_count: None,
            batch_size: None,
        });
        s
    }

    // ---------------- validate() ----------------

    #[test]
    fn validate_accepts_minimal_profile() {
        let f = factory();
        validate(&minimal("ok"), &f).unwrap();
    }

    #[test]
    fn validate_rejects_missing_first_phase() {
        let f = factory();
        let s = RankProfileSpec::new("bad");
        match validate(&s, &f) {
            Err(RankError::InvalidProfile(msg)) => assert!(msg.contains("first_phase")),
            other => panic!("expected InvalidProfile: {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_unknown_feature() {
        let f = factory();
        let mut s = minimal("x");
        s.first_phase.as_mut().unwrap().expression = "frobnicate()".into();
        match validate(&s, &f) {
            Err(RankError::InvalidProfile(msg)) => assert!(msg.contains("frobnicate")),
            other => panic!("expected InvalidProfile referencing the bad feature: {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_rerank_count_gt_heap_size() {
        let f = factory();
        let mut s = minimal("x");
        s.first_phase.as_mut().unwrap().heap_size = Some(50);
        s.second_phase = Some(PhaseSpec {
            expression: "bm25(\"title\")".into(),
            heap_size: None,
            rerank_count: Some(100),
            batch_size: None,
        });
        match validate(&s, &f) {
            Err(RankError::InvalidProfile(msg)) => {
                assert!(msg.contains("rerank_count"));
                assert!(msg.contains("heap_size"));
            }
            other => panic!("expected InvalidProfile, got: {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_zero_budget() {
        let f = factory();
        let mut s = minimal("x");
        s.budget.first_max_us = Some(0);
        match validate(&s, &f) {
            Err(RankError::InvalidProfile(msg)) => assert!(msg.contains("first_max_us")),
            other => panic!("expected InvalidProfile: {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_zero_heap_size() {
        let f = factory();
        let mut s = minimal("x");
        s.first_phase.as_mut().unwrap().heap_size = Some(0);
        match validate(&s, &f) {
            Err(RankError::InvalidProfile(msg)) => assert!(msg.contains("heap_size")),
            other => panic!("expected InvalidProfile: {other:?}"),
        }
    }

    #[test]
    fn validate_checks_match_features() {
        let f = factory();
        let mut s = minimal("x");
        s.match_features = vec!["unknown_feature()".into()];
        match validate(&s, &f) {
            Err(RankError::InvalidProfile(msg)) => assert!(msg.contains("match_features")),
            other => panic!("expected InvalidProfile: {other:?}"),
        }
    }

    #[test]
    fn validate_checks_function_bodies() {
        let f = factory();
        let mut s = minimal("x");
        s.functions = vec![FunctionSpec {
            name: "bad".into(),
            args: vec![],
            expression: "non_existent()".into(),
        }];
        match validate(&s, &f) {
            Err(RankError::InvalidProfile(msg)) => assert!(msg.contains("function 'bad'")),
            other => panic!("expected InvalidProfile: {other:?}"),
        }
    }

    // ---------------- resolve_inheritance() ----------------

    #[test]
    fn inheritance_single_chain() {
        let parent = RankProfileSpec {
            name: "parent".into(),
            description: Some("from parent".into()),
            first_phase: Some(PhaseSpec {
                expression: "bm25(\"x\")".into(),
                heap_size: Some(500),
                rerank_count: None,
                batch_size: None,
            }),
            budget: PhaseBudgetSpec {
                first_max_us: Some(1000),
                ..Default::default()
            },
            constants: vec![ConstantSpec {
                name: "w".into(),
                value: 0.5,
            }],
            ..RankProfileSpec::new("parent")
        };
        let child = RankProfileSpec {
            name: "child".into(),
            inherits: Some("parent".into()),
            // Override description.
            description: Some("from child".into()),
            // Don't override first_phase; should inherit.
            // Override budget.first_max_us; inherit nothing else.
            budget: PhaseBudgetSpec {
                first_max_us: Some(2000),
                ..Default::default()
            },
            ..RankProfileSpec::new("child")
        };
        let known = HashMap::from([("parent".into(), parent.clone())]);
        let resolved = resolve_inheritance(child, &known).unwrap();
        assert_eq!(resolved.description.as_deref(), Some("from child"));
        assert!(resolved.first_phase.is_some());
        let fp = resolved.first_phase.as_ref().unwrap();
        assert_eq!(fp.heap_size, Some(500));
        assert_eq!(resolved.budget.first_max_us, Some(2000));
        assert_eq!(resolved.constants.len(), 1);
        assert_eq!(resolved.constants[0].name, "w");
        // `inherits` is cleared on the resolved profile.
        assert!(resolved.inherits.is_none());
    }

    #[test]
    fn inheritance_rejects_cycle() {
        let a = RankProfileSpec {
            name: "a".into(),
            inherits: Some("b".into()),
            ..RankProfileSpec::new("a")
        };
        let b = RankProfileSpec {
            name: "b".into(),
            inherits: Some("a".into()),
            ..RankProfileSpec::new("b")
        };
        let known = HashMap::from([("a".into(), a.clone()), ("b".into(), b.clone())]);
        match resolve_inheritance(a, &known) {
            Err(RankError::InvalidProfile(msg)) => assert!(msg.contains("cycle")),
            other => panic!("expected cycle error: {other:?}"),
        }
    }

    #[test]
    fn inheritance_rejects_missing_parent() {
        let child = RankProfileSpec {
            name: "x".into(),
            inherits: Some("missing".into()),
            ..RankProfileSpec::new("x")
        };
        let known = HashMap::new();
        match resolve_inheritance(child, &known) {
            Err(RankError::InvalidProfile(msg)) => assert!(msg.contains("missing")),
            other => panic!("expected error: {other:?}"),
        }
    }

    #[test]
    fn inheritance_rejects_too_deep() {
        // Build a chain longer than MAX_INHERITANCE_DEPTH.
        let mut known = HashMap::new();
        for i in 0..(MAX_INHERITANCE_DEPTH + 3) {
            let name = format!("p{i}");
            let parent = if i + 1 < MAX_INHERITANCE_DEPTH + 3 {
                Some(format!("p{}", i + 1))
            } else {
                None
            };
            known.insert(
                name.clone(),
                RankProfileSpec {
                    name: name.clone(),
                    inherits: parent,
                    ..RankProfileSpec::new(&name)
                },
            );
        }
        let root = known.get("p0").unwrap().clone();
        match resolve_inheritance(root, &known) {
            Err(RankError::InvalidProfile(msg)) => assert!(msg.contains("max depth")),
            other => panic!("expected depth error: {other:?}"),
        }
    }

    #[test]
    fn inheritance_constants_merged_by_name() {
        let parent = RankProfileSpec {
            name: "p".into(),
            constants: vec![
                ConstantSpec {
                    name: "a".into(),
                    value: 1.0,
                },
                ConstantSpec {
                    name: "b".into(),
                    value: 2.0,
                },
            ],
            ..RankProfileSpec::new("p")
        };
        let child = RankProfileSpec {
            name: "c".into(),
            inherits: Some("p".into()),
            constants: vec![ConstantSpec {
                name: "b".into(),
                value: 99.0,
            }],
            ..RankProfileSpec::new("c")
        };
        let known = HashMap::from([("p".into(), parent)]);
        let resolved = resolve_inheritance(child, &known).unwrap();
        assert_eq!(resolved.constants.len(), 2);
        let b = resolved.constants.iter().find(|c| c.name == "b").unwrap();
        assert_eq!(b.value, 99.0, "child constant must override parent");
        let a = resolved.constants.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.value, 1.0);
    }
}

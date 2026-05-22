// Predicate normalizer — produces the canonical (col, op, val) triples
// that `plan_cache::digest_predicates` consumes.
//
// `plan_cache`'s digest is intentionally order-sensitive so accidental
// permutations don't accidentally collide. This module ships the
// canonicalizer that callers run before digesting: it stringifies each
// (column, op, value) and sorts the resulting triples lexicographically.
// After normalization the digest is order-independent for any caller that
// uses this helper — which is the contract the v2 records.rs handler
// wants. Callers that need order-sensitive digesting (rare; intentional)
// skip this normalizer and pass their triples directly.

use super::{Predicate, PredicateOp, PredicateValue};

/// Canonical triple: column, op label, value representation.
pub type Triple = (String, String, String);

/// Convert one `PredicateOp` to its canonical lowercase label.
pub fn op_label(op: &PredicateOp) -> &'static str {
    match op {
        PredicateOp::Eq => "eq",
        PredicateOp::Ne => "ne",
        PredicateOp::Lt => "lt",
        PredicateOp::Le => "le",
        PredicateOp::Gt => "gt",
        PredicateOp::Ge => "ge",
        PredicateOp::Like => "like",
        PredicateOp::In => "in",
        PredicateOp::IsNull => "is_null",
        PredicateOp::IsNotNull => "is_not_null",
        PredicateOp::Between => "between",
    }
}

/// Convert one `PredicateValue` to a stable string representation.
///
/// Floats use `{:?}` rather than `{}` so `1.0` and `1.00` collapse to the
/// same digest input. Lists serialize as `[v1,v2,v3]` after recursive
/// normalization so an `IN ('a','b')` query digests the same regardless
/// of element order — list-equality semantics are set-like.
pub fn value_repr(value: &PredicateValue) -> String {
    match value {
        PredicateValue::Null => "null".to_string(),
        PredicateValue::Bool(b) => b.to_string(),
        PredicateValue::Int(i) => i.to_string(),
        PredicateValue::Float(f) => {
            // Canonical float: f64's Debug representation gives us "1.0"
            // for 1.0 and "1.5" for 1.5. NaN collapses to a fixed token
            // so two NaN predicates hash identically.
            if f.is_nan() {
                "f64:nan".to_string()
            } else if f.is_infinite() {
                if *f > 0.0 { "f64:+inf".into() } else { "f64:-inf".into() }
            } else {
                format!("{:?}", f)
            }
        }
        PredicateValue::String(s) => format!("\"{s}\""),
        PredicateValue::List(items) => {
            let mut reprs: Vec<String> = items.iter().map(value_repr).collect();
            reprs.sort();
            format!("[{}]", reprs.join(","))
        }
    }
}

/// Normalize one predicate into a triple.
pub fn normalize_one(p: &Predicate) -> Triple {
    (p.column.clone(), op_label(p.op_ref()).to_string(), value_repr(&p.value))
}

/// Helper trait extension so `Predicate.op` can be borrowed without making
/// it `Copy` (we have no control over the upstream definition).
trait PredicateExt {
    fn op_ref(&self) -> &PredicateOp;
}

impl PredicateExt for Predicate {
    fn op_ref(&self) -> &PredicateOp {
        &self.op
    }
}

/// Normalize a slice of predicates and sort the triples so two equivalent
/// predicate sets — even in different orders — produce the same digest.
pub fn normalize(predicates: &[Predicate]) -> Vec<Triple> {
    let mut out: Vec<Triple> = predicates.iter().map(normalize_one).collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::cache::plan_cache::digest_predicates;

    fn p(col: &str, op: PredicateOp, val: PredicateValue) -> Predicate {
        Predicate { column: col.into(), op, value: val }
    }

    #[test]
    fn op_label_covers_every_variant() {
        // Pin the labels so a downstream consumer can rely on the wire
        // names not silently changing under it.
        assert_eq!(op_label(&PredicateOp::Eq), "eq");
        assert_eq!(op_label(&PredicateOp::Ne), "ne");
        assert_eq!(op_label(&PredicateOp::Lt), "lt");
        assert_eq!(op_label(&PredicateOp::Le), "le");
        assert_eq!(op_label(&PredicateOp::Gt), "gt");
        assert_eq!(op_label(&PredicateOp::Ge), "ge");
        assert_eq!(op_label(&PredicateOp::Like), "like");
        assert_eq!(op_label(&PredicateOp::In), "in");
        assert_eq!(op_label(&PredicateOp::IsNull), "is_null");
        assert_eq!(op_label(&PredicateOp::IsNotNull), "is_not_null");
        assert_eq!(op_label(&PredicateOp::Between), "between");
    }

    #[test]
    fn value_repr_covers_scalar_variants() {
        assert_eq!(value_repr(&PredicateValue::Null), "null");
        assert_eq!(value_repr(&PredicateValue::Bool(true)), "true");
        assert_eq!(value_repr(&PredicateValue::Bool(false)), "false");
        assert_eq!(value_repr(&PredicateValue::Int(42)), "42");
        // Strings carry their quotes so "1" doesn't collide with the int.
        assert_eq!(value_repr(&PredicateValue::String("hi".into())), "\"hi\"");
    }

    #[test]
    fn integer_one_does_not_collide_with_string_one() {
        let i = value_repr(&PredicateValue::Int(1));
        let s = value_repr(&PredicateValue::String("1".into()));
        assert_ne!(i, s, "int 1 must digest differently from string \"1\"");
    }

    #[test]
    fn float_special_values_collapse_to_fixed_tokens() {
        assert_eq!(value_repr(&PredicateValue::Float(f64::NAN)), "f64:nan");
        assert_eq!(value_repr(&PredicateValue::Float(f64::INFINITY)), "f64:+inf");
        assert_eq!(value_repr(&PredicateValue::Float(f64::NEG_INFINITY)), "f64:-inf");
    }

    #[test]
    fn float_one_and_one_dot_zero_collapse() {
        // Both produce "1.0" via the Debug formatter.
        let a = value_repr(&PredicateValue::Float(1.0));
        let b = value_repr(&PredicateValue::Float(1.000));
        assert_eq!(a, b);
    }

    #[test]
    fn list_value_repr_is_order_independent() {
        let a = PredicateValue::List(vec![
            PredicateValue::String("a".into()),
            PredicateValue::String("b".into()),
            PredicateValue::String("c".into()),
        ]);
        let b = PredicateValue::List(vec![
            PredicateValue::String("c".into()),
            PredicateValue::String("a".into()),
            PredicateValue::String("b".into()),
        ]);
        assert_eq!(value_repr(&a), value_repr(&b));
    }

    #[test]
    fn list_value_repr_distinguishes_distinct_elements() {
        let a = PredicateValue::List(vec![
            PredicateValue::String("a".into()),
            PredicateValue::String("b".into()),
        ]);
        let c = PredicateValue::List(vec![
            PredicateValue::String("a".into()),
            PredicateValue::String("c".into()),
        ]);
        assert_ne!(value_repr(&a), value_repr(&c));
    }

    #[test]
    fn normalize_sorts_triples() {
        let preds = vec![
            p("z", PredicateOp::Eq, PredicateValue::String("v".into())),
            p("a", PredicateOp::Eq, PredicateValue::String("v".into())),
            p("m", PredicateOp::Eq, PredicateValue::String("v".into())),
        ];
        let triples = normalize(&preds);
        // Sorted by first element of each triple (column).
        assert_eq!(triples[0].0, "a");
        assert_eq!(triples[1].0, "m");
        assert_eq!(triples[2].0, "z");
    }

    #[test]
    fn normalize_makes_digest_order_independent() {
        let a = vec![
            p("x", PredicateOp::Eq, PredicateValue::Int(1)),
            p("y", PredicateOp::Eq, PredicateValue::Int(2)),
        ];
        let b = vec![
            p("y", PredicateOp::Eq, PredicateValue::Int(2)),
            p("x", PredicateOp::Eq, PredicateValue::Int(1)),
        ];
        let da = digest_predicates(&normalize(&a));
        let db = digest_predicates(&normalize(&b));
        assert_eq!(da, db, "normalize() must produce order-independent digest");
    }

    #[test]
    fn empty_predicates_normalize_to_empty_vec() {
        let triples = normalize(&[]);
        assert!(triples.is_empty());
        // Digest of empty triples is stable.
        assert_eq!(digest_predicates(&triples), digest_predicates(&[]));
    }

    #[test]
    fn distinct_predicates_have_distinct_digests() {
        let a = vec![p("x", PredicateOp::Eq, PredicateValue::Int(1))];
        let b = vec![p("x", PredicateOp::Eq, PredicateValue::Int(2))];
        assert_ne!(
            digest_predicates(&normalize(&a)),
            digest_predicates(&normalize(&b))
        );
    }

    #[test]
    fn distinct_ops_have_distinct_digests() {
        let a = vec![p("x", PredicateOp::Eq, PredicateValue::Int(1))];
        let b = vec![p("x", PredicateOp::Lt, PredicateValue::Int(1))];
        assert_ne!(
            digest_predicates(&normalize(&a)),
            digest_predicates(&normalize(&b))
        );
    }

    #[test]
    fn list_predicate_normalizes_and_digests() {
        let a = vec![p(
            "x",
            PredicateOp::In,
            PredicateValue::List(vec![PredicateValue::Int(1), PredicateValue::Int(2)]),
        )];
        let b = vec![p(
            "x",
            PredicateOp::In,
            PredicateValue::List(vec![PredicateValue::Int(2), PredicateValue::Int(1)]),
        )];
        // In-list semantics are set-like; both digest the same.
        assert_eq!(
            digest_predicates(&normalize(&a)),
            digest_predicates(&normalize(&b))
        );
    }

    #[test]
    fn null_value_is_distinct_from_string_null() {
        let null = value_repr(&PredicateValue::Null);
        let s = value_repr(&PredicateValue::String("null".into()));
        assert_ne!(null, s);
    }
}

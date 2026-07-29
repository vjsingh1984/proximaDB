// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! End-to-end ABAC enforcement: resolve → compile → `admits_with_security`.
//!
//! This test module proves the full chain Phase 1 (compile bridge) + Phase 2
//! (evaluation) without wiring into the DML scan path. It is the TDD proof that
//! a `dept=eng` subject sees only `dept=eng` rows — the canonical ABAC scenario.

use crate::core::search::{ComparisonOperator, FilterExpression};
use crate::security::rls::filter_lattice::admits_with_security;
use proximadb_abac::{
    AttributeAuthority, AttributeBinding, AuthorizedReadContext, InMemoryAttributeAuthority,
    InMemoryPolicyEpochs, InMemoryPredicateObjectStore, PredicateObjectStore,
    compile_security_filter,
};
use proximadb_catalog::fc_metamodel::{AttrValue, Effect, PolicyBinding, Scope, SubjectId, Target};
use serde_json::json;

const TENANT: u64 = 7;
const NS: u16 = 3;
const TABLE: u32 = 200;

/// The canonical end-to-end: a subject with `dept=eng` filtering to only
/// `dept=eng` rows through the full resolve → compile → evaluate chain.
#[test]
fn dept_eng_subject_sees_only_dept_eng_rows() {
    // --- Set up the substrate ---
    let mut authority = InMemoryAttributeAuthority::new();
    authority.upsert(
        AttributeBinding::new("alice", TENANT).with_attr("dept", AttrValue::Str("eng".into())),
    );
    authority.upsert(
        AttributeBinding::new("bob", TENANT).with_attr("dept", AttrValue::Str("hr".into())),
    );

    let mut store = InMemoryPredicateObjectStore::new();
    // Predicate object 42: the subject's dept must equal the row's dept.
    // (Simplified: in a real system, this predicate is derived from the
    //  policy rule; here we store the already-lowered FilterExpression.)
    store.register(
        42,
        FilterExpression::Comparison {
            field: "dept".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("eng"), // alice's dept
        },
    );

    let bindings = vec![PolicyBinding {
        object_id: 1,
        tenant_stable_id: TENANT,
        scope: Scope::Table(TABLE),
        effect: Effect::Permit,
        predicate_ref: Some(42),
        field_mask: None,
    }];
    let epochs = InMemoryPolicyEpochs::new();
    let target = Target {
        namespace: NS,
        table: TABLE,
        column: None,
    };

    // --- Resolve alice ---
    let ctx = AuthorizedReadContext::resolve(
        &authority,
        &epochs,
        &bindings,
        &SubjectId("alice".into()),
        TENANT,
        target,
    )
    .admitted()
    .expect("alice is bound and permitted");

    // --- Compile the security filter from the resolved refs ---
    let security = compile_security_filter(ctx.row_predicate_refs(), &store);

    // --- Evaluate: alice (dept=eng) should see only dept=eng rows ---
    let rows = [
        (json!({"dept": "eng", "name": "carol"}), true),
        (json!({"dept": "hr", "name": "dave"}), false),
        (json!({"dept": "eng", "name": "eve"}), true),
        (json!({"dept": "legal", "name": "frank"}), false),
    ];

    for (row, expected_admit) in rows {
        let resolve = |f: &str| row.get(f).cloned();
        let admitted = admits_with_security(None, security.as_ref(), &resolve);
        assert_eq!(
            admitted, expected_admit,
            "alice (dept=eng) vs row {:?}: expected admit={expected_admit}",
            row
        );
    }
}

/// Fail-closed: a subject with no policy binding is denied (ReadDecision::Deny).
#[test]
fn unbound_subject_is_denied() {
    let authority = InMemoryAttributeAuthority::new();
    let epochs = InMemoryPolicyEpochs::new();
    let bindings: Vec<PolicyBinding> = vec![]; // no bindings → no applicable policy

    let decision = AuthorizedReadContext::resolve(
        &authority,
        &epochs,
        &bindings,
        &SubjectId("mallory".into()),
        TENANT,
        Target {
            namespace: NS,
            table: TABLE,
            column: None,
        },
    );

    // No applicable binding → deny (fail-closed default).
    assert!(
        decision.is_deny(),
        "an unbound subject must be denied, not silently admitted"
    );
}

/// Fail-closed: a policy referencing a missing predicate object denies.
#[test]
fn missing_predicate_ref_denies_all_rows() {
    let mut authority = InMemoryAttributeAuthority::new();
    authority.upsert(
        AttributeBinding::new("alice", TENANT).with_attr("dept", AttrValue::Str("eng".into())),
    );

    let store = InMemoryPredicateObjectStore::new(); // empty — ref 999 is missing

    let bindings = vec![PolicyBinding {
        object_id: 1,
        tenant_stable_id: TENANT,
        scope: Scope::Table(TABLE),
        effect: Effect::Permit,
        predicate_ref: Some(999),
        field_mask: None,
    }];
    let epochs = InMemoryPolicyEpochs::new();
    let ctx = AuthorizedReadContext::resolve(
        &authority,
        &epochs,
        &bindings,
        &SubjectId("alice".into()),
        TENANT,
        Target {
            namespace: NS,
            table: TABLE,
            column: None,
        },
    )
    .admitted()
    .expect("alice is bound");

    // The compiled filter is unsatisfiable (missing ref → deny-all).
    let security = compile_security_filter(ctx.row_predicate_refs(), &store);
    let row = json!({"dept": "eng"});
    let resolve = |f: &str| row.get(f).cloned();
    assert!(
        !admits_with_security(None, security.as_ref(), &resolve),
        "a missing predicate ref must deny every row, not admit"
    );
}

/// Different subjects with different attributes get different row sets.
#[test]
fn different_subjects_see_different_rows() {
    let mut authority = InMemoryAttributeAuthority::new();
    authority.upsert(
        AttributeBinding::new("alice", TENANT).with_attr("dept", AttrValue::Str("eng".into())),
    );
    authority.upsert(
        AttributeBinding::new("bob", TENANT).with_attr("dept", AttrValue::Str("hr".into())),
    );

    let mut store = InMemoryPredicateObjectStore::new();
    // Predicate for alice (dept=eng)
    store.register(
        42,
        FilterExpression::Comparison {
            field: "dept".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("eng"),
        },
    );
    // Predicate for bob (dept=hr)
    store.register(
        43,
        FilterExpression::Comparison {
            field: "dept".to_string(),
            operator: ComparisonOperator::Equals,
            value: json!("hr"),
        },
    );

    let bindings_alice = vec![PolicyBinding {
        object_id: 1,
        tenant_stable_id: TENANT,
        scope: Scope::Table(TABLE),
        effect: Effect::Permit,
        predicate_ref: Some(42),
        field_mask: None,
    }];
    let bindings_bob = vec![PolicyBinding {
        object_id: 2,
        tenant_stable_id: TENANT,
        scope: Scope::Table(TABLE),
        effect: Effect::Permit,
        predicate_ref: Some(43),
        field_mask: None,
    }];
    let epochs = InMemoryPolicyEpochs::new();
    let target = Target {
        namespace: NS,
        table: TABLE,
        column: None,
    };

    let ctx_alice = AuthorizedReadContext::resolve(
        &authority,
        &epochs,
        &bindings_alice,
        &SubjectId("alice".into()),
        TENANT,
        target,
    )
    .admitted()
    .expect("alice");
    let ctx_bob = AuthorizedReadContext::resolve(
        &authority,
        &epochs,
        &bindings_bob,
        &SubjectId("bob".into()),
        TENANT,
        target,
    )
    .admitted()
    .expect("bob");

    let sec_alice = compile_security_filter(ctx_alice.row_predicate_refs(), &store);
    let sec_bob = compile_security_filter(ctx_bob.row_predicate_refs(), &store);

    let eng_row = json!({"dept": "eng"});
    let hr_row = json!({"dept": "hr"});

    // Alice sees eng, not hr.
    assert!(admits_with_security(
        None,
        sec_alice.as_ref(),
        &|f: &str| eng_row.get(f).cloned()
    ));
    assert!(!admits_with_security(
        None,
        sec_alice.as_ref(),
        &|f: &str| hr_row.get(f).cloned()
    ));

    // Bob sees hr, not eng.
    assert!(!admits_with_security(None, sec_bob.as_ref(), &|f: &str| {
        eng_row.get(f).cloned()
    }));
    assert!(admits_with_security(None, sec_bob.as_ref(), &|f: &str| {
        hr_row.get(f).cloned()
    }));
}

// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! The ABAC-to-scan-predicate adapter: turns compiled `ObjectId` refs into a
//! per-row `Fn(&ProximaRecord) -> bool` that `scan_records_filtered` ANDs into
//! its existing `RecordScanPredicate`.
//!
//! This is the **scan-path integration point** (FA-c Phase 2b). It compiles the
//! `AuthorizedReadContext`'s `row_predicate_refs` once (at request-scope), then
//! the returned closure evaluates each record under the strict 3-valued walker
//! via [`admits_with_security`](super::filter_lattice::admits_with_security).

#[cfg(feature = "abac-policy")]
use crate::core::search::sql_value_filter::proxima_value_to_json;
#[cfg(feature = "abac-policy")]
use crate::security::rls::filter_lattice::admits_with_security;
#[cfg(feature = "abac-policy")]
use proximadb_abac::{PredicateObjectStore, compile_security_filter};
#[cfg(feature = "abac-policy")]
use proximadb_catalog::fc_metamodel::ObjectId;
#[cfg(feature = "abac-policy")]
use proximadb_records::{ProximaRecord, ProximaTreeNode};

/// Compile `refs` into a per-row predicate for `scan_records_filtered`.
///
/// Returns `None` when the refs are empty (no row restriction) or when the
/// compiled expression is `None`. Returns `Some(closure)` that evaluates each
/// `ProximaRecord` under the strict security walker. A missing predicate ref
/// (fail-closed in `compile_security_filter`) yields an unsatisfiable closure
/// that denies every row.
///
/// The closure owns the compiled `FilterExpression`; it is `Send + Sync` and
/// can be passed as a `RecordScanPredicate` to `scan_records_filtered`.
/// FA-c Phase 2c wires this into `scan_records_filtered`'s predicate param; until
/// then it has no production caller (the feature is default-OFF and the function is
/// the integration point the wiring step consumes).
#[cfg(feature = "abac-policy")]
#[allow(dead_code)]
pub fn abac_scan_predicate(
    refs: &[ObjectId],
    store: &dyn PredicateObjectStore,
) -> Option<Box<dyn Fn(&ProximaRecord) -> bool + Send + Sync>> {
    let security = compile_security_filter(refs, store)?;

    Some(Box::new(move |record: &ProximaRecord| {
        // Resolve each field the security expression references from the record's
        // props. Using `proxima_value_to_json` (not the whole-tree map) avoids a
        // per-row HashMap allocation — only the fields the expression actually
        // touches are extracted, lazily.
        let resolve = |field: &str| -> Option<serde_json::Value> {
            record.props.get(field).and_then(|node| match node {
                ProximaTreeNode::Value(pv) => Some(proxima_value_to_json(pv)),
                _ => None,
            })
        };
        admits_with_security(None, Some(&security), &resolve)
    }))
}

#[cfg(all(test, feature = "abac-policy"))]
mod tests {
    use super::*;
    use crate::core::search::{ComparisonOperator, FilterExpression};
    use proximadb_abac::InMemoryPredicateObjectStore;
    use proximadb_data_model::ProximaValue;
    use proximadb_records::{ProximaRecord, ProximaTreeNode};
    use serde_json::json;

    fn record_with(dept: &str) -> ProximaRecord {
        let mut rec = ProximaRecord::default();
        rec.props.insert(
            "dept".to_string(),
            ProximaTreeNode::Value(ProximaValue::String(dept.to_string())),
        );
        rec
    }

    #[test]
    fn predicate_filters_rows_by_dept() {
        let mut store = InMemoryPredicateObjectStore::new();
        store.register(
            42,
            FilterExpression::Comparison {
                field: "dept".to_string(),
                operator: ComparisonOperator::Equals,
                value: json!("eng"),
            },
        );

        let predicate = abac_scan_predicate(&[42], &store).expect("refs non-empty → predicate");

        assert!(predicate(&record_with("eng")), "dept=eng must be admitted");
        assert!(!predicate(&record_with("hr")), "dept=hr must be denied");
    }

    #[test]
    fn empty_refs_produce_no_predicate() {
        let store = InMemoryPredicateObjectStore::new();
        assert!(abac_scan_predicate(&[], &store).is_none());
    }

    #[test]
    fn missing_ref_denies_every_row() {
        let store = InMemoryPredicateObjectStore::new();
        let predicate =
            abac_scan_predicate(&[999], &store).expect("non-empty refs → unsatisfiable predicate");
        assert!(!predicate(&record_with("eng")), "missing ref denies all");
    }
}

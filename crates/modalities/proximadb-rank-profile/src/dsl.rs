//! TOML → [`RankProfileSpec`] parsing.
//!
//! Two surfaces:
//!
//! - [`parse_single`] — body of one profile (no `[rank_profile.NAME]`
//!   header); caller supplies the name. Convenient for programmatic
//!   construction and tests.
//! - [`parse_document`] — multiple `[rank_profile.NAME]` tables in one
//!   string; useful for `rank_profiles.toml` bundles.

use crate::spec::RankProfileSpec;
use proximadb_rank_core::{RankError, RankResult};
use serde::Deserialize;
use std::collections::HashMap;

/// Parse a single profile body, taking the name explicitly. Useful when
/// the caller doesn't want to wrap everything in `[rank_profile.NAME]`.
pub fn parse_single(name: &str, toml_body: &str) -> RankResult<RankProfileSpec> {
    let mut spec: RankProfileSpec = toml::from_str(toml_body)
        .map_err(|e| RankError::InvalidProfile(format!("TOML parse error: {e}")))?;
    // The name field on the body, if any, is overridden by the caller-
    // supplied name to avoid spec/key drift.
    spec.name = name.to_string();
    Ok(spec)
}

/// Parse a TOML document with `[rank_profile.NAME]` headers; returns the
/// map keyed by profile name.
pub fn parse_document(toml_doc: &str) -> RankResult<HashMap<String, RankProfileSpec>> {
    #[derive(Deserialize)]
    struct Document {
        #[serde(default)]
        rank_profile: HashMap<String, RankProfileSpec>,
    }
    let doc: Document = toml::from_str(toml_doc)
        .map_err(|e| RankError::InvalidProfile(format!("TOML parse error: {e}")))?;
    Ok(doc
        .rank_profile
        .into_iter()
        .map(|(name, mut spec)| {
            spec.name = name.clone();
            (name, spec)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::PhaseBudgetSpec;

    // Note on TOML structure: top-level scalar keys (`inherits`,
    // `description`, `match_features`, `summary_features`) MUST come
    // before any `[table]` header — once we enter `[first_phase]`,
    // subsequent key=value lines bind to that table.
    const FULL_PROFILE: &str = r#"
inherits = "default"
description = "Hybrid retrieval + cross-encoder rerank"
match_features = ["bm25(\"title\")", "closeness(\"embedding\")"]
summary_features = ["model('rerank-v3')"]

[first_phase]
expression = "closeness(\"embedding\") * 0.6 + bm25(\"title\") * 0.4"
heap_size = 1000

[second_phase]
expression = "model('rerank-v3', query, summary)"
rerank_count = 100
batch_size = 32

[global_phase]
strategy = "cross_modal"
rerank_count = 50
config = { mmr_lambda = 0.7 }

[budget]
first_max_us = 5000
second_max_us = 50000
global_max_us = 100000

[[functions]]
name = "personalized"
args = ["user_id"]
expression = "attribute(\"user_affinity\")"

[[constants]]
name = "w_bm25"
value = 0.4
"#;

    #[test]
    fn parse_single_round_trips_full_profile() {
        let spec = parse_single("semantic_plus_ce", FULL_PROFILE).unwrap();
        assert_eq!(spec.name, "semantic_plus_ce");
        assert_eq!(spec.inherits.as_deref(), Some("default"));
        assert_eq!(
            spec.description.as_deref(),
            Some("Hybrid retrieval + cross-encoder rerank")
        );
        assert!(spec.first_phase.is_some());
        let fp = spec.first_phase.as_ref().unwrap();
        assert_eq!(fp.heap_size, Some(1000));
        assert!(fp.expression.contains("closeness"));
        let sp = spec.second_phase.as_ref().unwrap();
        assert_eq!(sp.rerank_count, Some(100));
        assert_eq!(sp.batch_size, Some(32));
        let gp = spec.global_phase.as_ref().unwrap();
        assert_eq!(gp.strategy, "cross_modal");
        assert_eq!(gp.rerank_count, Some(50));
        assert_eq!(spec.match_features.len(), 2);
        assert_eq!(spec.summary_features.len(), 1);
        assert_eq!(spec.budget.first_max_us, Some(5000));
        assert_eq!(spec.functions.len(), 1);
        assert_eq!(spec.constants.len(), 1);
        assert_eq!(spec.constants[0].value, 0.4);
    }

    #[test]
    fn parse_single_overrides_name_field() {
        let body = r#"
name = "ignored-on-purpose"
[first_phase]
expression = "1.0"
"#;
        let spec = parse_single("explicit", body).unwrap();
        assert_eq!(spec.name, "explicit");
    }

    #[test]
    fn parse_document_with_multiple_profiles() {
        let doc = r#"
[rank_profile.alpha]
description = "alpha"
[rank_profile.alpha.first_phase]
expression = "1.0"

[rank_profile.beta]
inherits = "alpha"
[rank_profile.beta.first_phase]
expression = "2.0"
"#;
        let m = parse_document(doc).unwrap();
        assert_eq!(m.len(), 2);
        let a = m.get("alpha").unwrap();
        assert_eq!(a.name, "alpha");
        let b = m.get("beta").unwrap();
        assert_eq!(b.inherits.as_deref(), Some("alpha"));
    }

    #[test]
    fn parse_document_handles_empty() {
        let m = parse_document("").unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn parse_invalid_toml_errors() {
        let err = parse_single("x", "[unterminated").unwrap_err();
        match err {
            RankError::InvalidProfile(msg) => assert!(msg.contains("TOML parse error")),
            other => panic!("expected InvalidProfile, got: {other:?}"),
        }
    }

    #[test]
    fn parse_minimum_profile() {
        // The smallest valid profile is just a name (validator may still
        // reject it for lacking a first phase — that's a different concern).
        let spec = parse_single("nameonly", "").unwrap();
        assert_eq!(spec.name, "nameonly");
        assert!(spec.first_phase.is_none());
        assert_eq!(spec.budget, PhaseBudgetSpec::default());
    }
}

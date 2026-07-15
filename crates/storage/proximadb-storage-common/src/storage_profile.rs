//! Per-collection workload storage profile (ADR-061 D1, TD-WLP-1).
//!
//! `StorageProfile` selects the **read-acceleration projection strategy** for a
//! collection — it is deliberately distinct from `CatalogWorkloadProfile`
//! (query-shape → engine routing) per ADR-061 D6, and it never changes the
//! durability contract: canonical `ProximaRecord` + WAL stay the sole durable
//! authority for every profile (ADR-061 D2).
//!
//! Resolution mirrors the `resolve_pax_vector_quant` cascade
//! (`src/storage/engines/sst/flush/mod.rs`): per-collection
//! `workload_profile:append|bulk|churn` tag > env `PROXIMADB_STORAGE_PROFILE` >
//! default `append`. The resolver takes the collection's tag list (`&[String]`)
//! rather than the v1 proto `CollectionConfig` so it adds no v1-proto reference
//! (TD-123 ratchet) — tag extraction happens at the call site, matching
//! `pax_rerank_quant_tag`.

/// Tag prefix encoding the per-collection storage profile on
/// `CollectionConfig.tags` — mirrors the `pax_vector_format:` /
/// `pax_rerank_quant:` tag conventions. Values: `append`, `bulk`, `churn`.
pub const STORAGE_PROFILE_TAG_PREFIX: &str = "workload_profile:";

/// Deployment-wide storage-profile default env. Outranked by the
/// per-collection tag; unset / unrecognized → `AppendBulk`.
pub const STORAGE_PROFILE_ENV: &str = "PROXIMADB_STORAGE_PROFILE";

/// Per-collection storage-projection strategy (ADR-061).
///
/// `AppendBulk` (tag values `append` / `bulk`): durable clustered PAX-LSM
/// segments + VOE directory — today's behaviour, and the default absent any
/// tag/env. `Churn` (tag value `churn`): in-memory mutable ANN index + ORION
/// graph + `oid` fusion for bounded, hot, update-heavy working sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageProfile {
    /// Append-heavy / bulk-load: clustered PAX-LSM projection (default).
    #[default]
    AppendBulk,
    /// Update-heavy code-RAG: in-memory mutable projection (ADR-061 D4).
    Churn,
}

impl StorageProfile {
    /// Whether this profile forces freshness-critical (Strong / delta-merged)
    /// reads regardless of a request's freshness hint (ADR-061 D4, TD-WLP-5).
    ///
    /// `Churn` collections are the agent code-RAG shape: a symbol is re-embedded
    /// and immediately re-queried on a bounded, hot working set, so a stale read
    /// would return the pre-edit neighbours. Their reads therefore always merge
    /// the unflushed WAL/memtable delta — a request's `StaleOk`/`BoundedStale`
    /// hint is overridden to Strong. `AppendBulk` honors the request's hint
    /// (Strong is already the unset default), so this is a no-op there.
    pub fn forces_strong_reads(self) -> bool {
        matches!(self, StorageProfile::Churn)
    }

    /// Stable lowercase label for traces/EXPLAIN (io_trace storage-profile
    /// stamp, TD-WLP-6). Kept separate from `Debug` so external surfaces don't
    /// pin on the derived form.
    pub fn label(self) -> &'static str {
        match self {
            StorageProfile::AppendBulk => "append_bulk",
            StorageProfile::Churn => "churn",
        }
    }

    /// The profile-default radius weight `k` for the read-side centroid prune
    /// (ADR-061 D7 / TD-WLP-6): the `radius_k` a collection of this profile uses
    /// when the request doesn't set one, scoring blocks by `d(q,c) − k·radius`.
    ///
    /// **Currently `0.0` for BOTH profiles (byte-for-byte legacy scoring).**
    /// This is the *evidence-gated-defaults* carve-out of the ADR-061
    /// arm-defaults amendment: the 2026-07-14 SIFT sweep measured `radius_k=1/2`
    /// as a recall *regression* under the sign-bit L0 clustering, and a valid
    /// re-measurement needs the fp32-PCA/IVF compaction re-cluster to actually
    /// engage — which is blocked on the flush→compaction execution wiring
    /// (TD-WLP-7). The seam exists so calibrating `AppendBulk` is a one-line
    /// change once that measurement clears; `Churn` stays `0.0` (memory-resident
    /// path, radius irrelevant).
    pub fn default_radius_k(self) -> f32 {
        match self {
            // Calibration pending TD-WLP-7 re-measurement; DO NOT set non-zero
            // without a measured recall win (ADR-061 evidence-gated defaults).
            StorageProfile::AppendBulk => 0.0,
            StorageProfile::Churn => 0.0,
        }
    }
}

/// Resolve the storage profile for a collection from its tag list.
///
/// Precedence: `workload_profile:` tag (last matching wins; an unrecognized
/// value keeps the prior resolution, parity with `resolve_pax_vector_quant`) >
/// env `PROXIMADB_STORAGE_PROFILE` > default `AppendBulk`.
pub fn resolve_storage_profile(tags: &[String]) -> StorageProfile {
    if let Some(profile) = storage_profile_tag(tags) {
        return profile;
    }
    match std::env::var(STORAGE_PROFILE_ENV)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "churn" => StorageProfile::Churn,
        // `append` / `bulk` / unset / unrecognized → AppendBulk default.
        _ => StorageProfile::AppendBulk,
    }
}

/// Read the per-collection `workload_profile:append|bulk|churn` tag from a tag
/// list. Last matching tag wins; an unrecognized value keeps the prior
/// resolution (parity with `pax_vector_format_tag`). Absent → `None` (defer to
/// env/default).
fn storage_profile_tag(tags: &[String]) -> Option<StorageProfile> {
    let mut latest: Option<StorageProfile> = None;
    for tag in tags {
        if let Some(rest) = tag.strip_prefix(STORAGE_PROFILE_TAG_PREFIX) {
            latest = match rest.trim().to_ascii_lowercase().as_str() {
                "append" | "bulk" => Some(StorageProfile::AppendBulk),
                "churn" => Some(StorageProfile::Churn),
                _ => latest, // unrecognized value: keep prior resolution
            };
        }
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    /// TD-WLP-5: Churn forces Strong reads; AppendBulk honors the request.
    #[test]
    fn test_forces_strong_reads_only_for_churn() {
        assert!(StorageProfile::Churn.forces_strong_reads());
        assert!(!StorageProfile::AppendBulk.forces_strong_reads());
        // Resolved from a churn tag → forces strong.
        assert!(resolve_storage_profile(&tags(&["workload_profile:churn"])).forces_strong_reads());
        // Untagged (AppendBulk default) → does not.
        assert!(!resolve_storage_profile(&[]).forces_strong_reads());
    }

    /// TD-WLP-6: profile → prune defaults + label. `radius_k` is `0.0` for both
    /// profiles today (evidence-gated; calibration pending TD-WLP-7) — this
    /// test pins that intent so a non-zero value can't land without also
    /// updating the measured-lift rationale here.
    #[test]
    fn test_profile_prune_defaults_and_labels() {
        assert_eq!(StorageProfile::AppendBulk.label(), "append_bulk");
        assert_eq!(StorageProfile::Churn.label(), "churn");
        // Evidence-gated: both 0.0 until TD-WLP-7 unblocks a re-measurement.
        assert_eq!(StorageProfile::AppendBulk.default_radius_k(), 0.0);
        assert_eq!(StorageProfile::Churn.default_radius_k(), 0.0);
        // Resolved-from-tag parity.
        assert_eq!(
            resolve_storage_profile(&tags(&["workload_profile:churn"])).default_radius_k(),
            0.0
        );
    }

    /// TD-WLP-1 TDD gate: the tag > env > default cascade, mirroring
    /// `resolve_pax_vector_quant_default_and_precedence`.
    #[test]
    fn test_resolve_storage_profile_tag_env_default_cascade() {
        unsafe {
            std::env::remove_var(STORAGE_PROFILE_ENV);
        }
        // absent → AppendBulk default
        assert_eq!(resolve_storage_profile(&[]), StorageProfile::AppendBulk);
        // `append` and `bulk` tags → AppendBulk
        assert_eq!(
            resolve_storage_profile(&tags(&["workload_profile:append"])),
            StorageProfile::AppendBulk
        );
        assert_eq!(
            resolve_storage_profile(&tags(&["workload_profile:bulk"])),
            StorageProfile::AppendBulk
        );
        // `churn` tag → Churn
        assert_eq!(
            resolve_storage_profile(&tags(&["workload_profile:churn"])),
            StorageProfile::Churn
        );
        // unknown tag value → AppendBulk default (defer to env/default)
        assert_eq!(
            resolve_storage_profile(&tags(&["workload_profile:nonsense"])),
            StorageProfile::AppendBulk
        );
        // unrelated tags are ignored
        assert_eq!(
            resolve_storage_profile(&tags(&["recall_target:0.95"])),
            StorageProfile::AppendBulk
        );
        // last matching tag wins (parity with resolve_pax_vector_quant)
        assert_eq!(
            resolve_storage_profile(&tags(&[
                "workload_profile:append",
                "workload_profile:churn"
            ])),
            StorageProfile::Churn
        );
        // an unrecognized LAST value keeps the prior resolution, not the default
        assert_eq!(
            resolve_storage_profile(&tags(&[
                "workload_profile:churn",
                "workload_profile:nonsense"
            ])),
            StorageProfile::Churn
        );
        // env fallback: no tag + env churn → Churn
        unsafe {
            std::env::set_var(STORAGE_PROFILE_ENV, "churn");
        }
        assert_eq!(resolve_storage_profile(&[]), StorageProfile::Churn);
        // tag beats env: append tag + env churn → AppendBulk
        assert_eq!(
            resolve_storage_profile(&tags(&["workload_profile:append"])),
            StorageProfile::AppendBulk
        );
        // env `append` / `bulk` → AppendBulk; unrecognized env → default
        unsafe {
            std::env::set_var(STORAGE_PROFILE_ENV, "bulk");
        }
        assert_eq!(resolve_storage_profile(&[]), StorageProfile::AppendBulk);
        unsafe {
            std::env::set_var(STORAGE_PROFILE_ENV, "nonsense");
        }
        assert_eq!(resolve_storage_profile(&[]), StorageProfile::AppendBulk);
        unsafe {
            std::env::remove_var(STORAGE_PROFILE_ENV);
        }
    }
}

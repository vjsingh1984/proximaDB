//! Per-collection compaction override (ADR-061 D5, TD-WLP-2; defaults armed
//! since TD-WLP-4 under the pre-GA "arm defaults" directive).
//!
//! Compaction is the re-cluster event (it re-runs the PAX segment writer with
//! the PCA+IVF ordering — `write_pax_segment_compacted`), so `AppendBulk`
//! collections want it ON. Resolution cascade, mirroring
//! [`crate::storage_profile::resolve_storage_profile`]:
//!
//! * env explicitly falsy (`0`/`false`/`off`/`no`) → **hard disable**: the
//!   master kill-switch nothing can override.
//! * `compaction:on|off` tag (last matching wins) → explicit per-collection
//!   operator intent decides.
//! * no tag → armed iff the collection's resolved [`StorageProfile`] is
//!   `AppendBulk` (the default profile — i.e. untagged collections compact at
//!   threshold). `Churn` stays OFF (ADR-061 D4/D5: churn invalidates a
//!   clustering model faster than it pays off).
//!
//! [`StorageProfile`]: crate::storage_profile::StorageProfile

/// Tag prefix for the per-collection compaction arm/disarm override on
/// `CollectionConfig.tags`. Values: `on`/`true`/`1`/`yes`, `off`/`false`/`0`/`no`.
pub const COMPACTION_TAG_PREFIX: &str = "compaction:";

/// Tag prefix for the per-collection L0 segment-count compaction threshold.
/// Value: a positive integer (`l0_threshold:2`). Invalid values keep the prior
/// resolution (parity with the other tag parsers).
pub const COMPACTION_L0_THRESHOLD_TAG_PREFIX: &str = "l0_threshold:";

/// Global compaction gate env (TD-114). Truthy arms every collection
/// (including `Churn`); an explicitly falsy value is the master kill-switch
/// (TD-WLP-2); unset → per-profile default (TD-WLP-4: `AppendBulk` armed,
/// `Churn` off) with the `compaction:` tag as the per-collection override.
pub const L0_COMPACTION_ENABLED_ENV: &str = "PROXIMADB_L0_COMPACTION_ENABLED";

/// Tri-state reading of [`L0_COMPACTION_ENABLED_ENV`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalCompactionGate {
    /// Env truthy: compaction armed for every collection (force-all).
    Armed,
    /// Env unset or unrecognized: tag, then profile default (AppendBulk on).
    Default,
    /// Env explicitly falsy: master kill-switch — nothing may arm.
    HardDisabled,
}

/// Read the tri-state global compaction gate from the env.
pub fn global_compaction_gate() -> GlobalCompactionGate {
    match std::env::var(L0_COMPACTION_ENABLED_ENV)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "1" | "true" | "on" | "yes" => GlobalCompactionGate::Armed,
        "0" | "false" | "off" | "no" => GlobalCompactionGate::HardDisabled,
        _ => GlobalCompactionGate::Default,
    }
}

/// Resolve whether compaction is armed for a collection with `tags`.
///
/// Precedence: global hard-disable > `compaction:on|off` tag (last matching
/// wins) > global env truthy (force-all) > profile default (TD-WLP-4:
/// `AppendBulk` — including every untagged collection — armed; `Churn` off).
pub fn resolve_compaction_armed(tags: &[String]) -> bool {
    match global_compaction_gate() {
        GlobalCompactionGate::HardDisabled => false,
        GlobalCompactionGate::Armed => compaction_tag(tags).unwrap_or(true),
        GlobalCompactionGate::Default => compaction_tag(tags).unwrap_or_else(|| {
            crate::storage_profile::resolve_storage_profile(tags)
                == crate::storage_profile::StorageProfile::AppendBulk
        }),
    }
}

/// Read the per-collection `compaction:on|off` tag (last matching wins; an
/// unrecognized value keeps the prior resolution). Absent → `None` (defer to
/// the global gate). Mirrors `pax_vector_format_tag`.
fn compaction_tag(tags: &[String]) -> Option<bool> {
    let mut latest: Option<bool> = None;
    for tag in tags {
        if let Some(rest) = tag.strip_prefix(COMPACTION_TAG_PREFIX) {
            latest = match rest.trim().to_ascii_lowercase().as_str() {
                "on" | "true" | "1" | "yes" => Some(true),
                "off" | "false" | "0" | "no" => Some(false),
                _ => latest, // unrecognized value: keep prior resolution
            };
        }
    }
    latest
}

/// Resolve the effective L0 compaction threshold for a collection with `tags`:
/// the last valid positive `l0_threshold:N` tag wins; absent/invalid →
/// `default_threshold`.
pub fn resolve_l0_threshold(tags: &[String], default_threshold: usize) -> usize {
    let mut latest = default_threshold;
    for tag in tags {
        if let Some(rest) = tag.strip_prefix(COMPACTION_L0_THRESHOLD_TAG_PREFIX) {
            match rest.trim().parse::<usize>() {
                Ok(n) if n >= 1 => latest = n,
                // invalid / non-positive value: keep prior resolution
                _ => {}
            }
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

    /// TD-WLP-2/TD-WLP-4 cascade: hard-disable > tag (last wins) > env truthy
    /// (force-all) > profile default (AppendBulk armed, Churn off).
    #[test]
    fn test_resolve_compaction_armed_tag_env_default_cascade() {
        unsafe {
            std::env::remove_var(L0_COMPACTION_ENABLED_ENV);
            std::env::remove_var("PROXIMADB_STORAGE_PROFILE");
        }
        assert_eq!(global_compaction_gate(), GlobalCompactionGate::Default);
        // TD-WLP-4 default: untagged (AppendBulk profile) collections ARM.
        assert!(resolve_compaction_armed(&[]));
        assert!(resolve_compaction_armed(&tags(&[
            "workload_profile:append"
        ])));
        assert!(resolve_compaction_armed(&tags(&["workload_profile:bulk"])));
        // Churn profile stays OFF by default (ADR-061 D4/D5).
        assert!(!resolve_compaction_armed(&tags(&[
            "workload_profile:churn"
        ])));
        // Explicit tag beats the profile default in both directions.
        assert!(!resolve_compaction_armed(&tags(&["compaction:off"])));
        assert!(resolve_compaction_armed(&tags(&[
            "workload_profile:churn",
            "compaction:on"
        ])));
        // last matching tag wins
        assert!(resolve_compaction_armed(&tags(&[
            "compaction:off",
            "compaction:on"
        ])));
        // unrecognized value keeps the prior resolution
        assert!(!resolve_compaction_armed(&tags(&[
            "compaction:off",
            "compaction:sideways"
        ])));
        // global truthy force-arms even Churn...
        unsafe {
            std::env::set_var(L0_COMPACTION_ENABLED_ENV, "1");
        }
        assert_eq!(global_compaction_gate(), GlobalCompactionGate::Armed);
        assert!(resolve_compaction_armed(&tags(&["workload_profile:churn"])));
        // ...but an explicit per-collection opt-out still wins
        assert!(!resolve_compaction_armed(&tags(&["compaction:off"])));
        // global hard-disable is the master kill-switch: nothing can arm
        unsafe {
            std::env::set_var(L0_COMPACTION_ENABLED_ENV, "0");
        }
        assert_eq!(global_compaction_gate(), GlobalCompactionGate::HardDisabled);
        assert!(!resolve_compaction_armed(&tags(&["compaction:on"])));
        assert!(!resolve_compaction_armed(&[]));
        unsafe {
            std::env::set_var(L0_COMPACTION_ENABLED_ENV, "false");
        }
        assert!(!resolve_compaction_armed(&tags(&["compaction:on"])));
        unsafe {
            std::env::remove_var(L0_COMPACTION_ENABLED_ENV);
        }
    }

    /// TD-WLP-2: per-collection L0 threshold tag (last valid wins; invalid or
    /// non-positive values keep the prior resolution).
    #[test]
    fn test_resolve_l0_threshold_tag_overrides_default() {
        assert_eq!(resolve_l0_threshold(&[], 5), 5);
        assert_eq!(resolve_l0_threshold(&tags(&["l0_threshold:2"]), 5), 2);
        // last valid wins
        assert_eq!(
            resolve_l0_threshold(&tags(&["l0_threshold:2", "l0_threshold:7"]), 5),
            7
        );
        // invalid / non-positive keep the prior resolution
        assert_eq!(
            resolve_l0_threshold(&tags(&["l0_threshold:2", "l0_threshold:zero"]), 5),
            2
        );
        assert_eq!(resolve_l0_threshold(&tags(&["l0_threshold:0"]), 5), 5);
        assert_eq!(resolve_l0_threshold(&tags(&["l0_threshold:-3"]), 5), 5);
        // unrelated tags are ignored
        assert_eq!(
            resolve_l0_threshold(&tags(&["workload_profile:append"]), 5),
            5
        );
    }
}

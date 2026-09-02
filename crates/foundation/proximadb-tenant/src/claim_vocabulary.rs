// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-TENANT-3: the ONE claim vocabulary shared by every ingress surface.
//!
//! TD-TENANT-1 standardized the tenant-assertion *decision*
//! ([`resolve_tenant_assertion`](crate::resolve_tenant_assertion)), TD-ABAC-6
//! standardized the identity *orchestration*, and ADR-0053 W8 standardized the
//! tier-claim *decision* ([`resolve_tier_claim`](crate::resolve_tier_claim)).
//! None of them standardized the **vocabulary**: the set of wire names each
//! surface reads before handing a value to those primitives. That layer was
//! hand-rolled per surface and diverged — Arrow Flight accepted four tenant
//! spellings while REST and gRPC accepted one, so a claim honored on one port
//! was *silently ignored* on another (no error; fall-through to the credential
//! or default tenant).
//!
//! This module owns the names as data. Lookups are generic over a
//! `Fn(&str) -> Option<&str>` accessor so the same code serves
//! `http::HeaderMap`, `tonic::metadata::MetadataMap`, and pgwire's
//! startup-parameter map **without any of those types entering this foundation
//! crate**.
//!
//! ## Convergence direction: narrow, never widen
//!
//! Teaching REST and gRPC the extra Flight aliases would make
//! previously-ignored headers newly effective on two surfaces — a behaviour
//! change in the riskier direction even though the same
//! [`HeaderTrustPolicy`](crate::HeaderTrustPolicy) governs them. Instead
//! [`TENANT_CLAIM_HEADER`] / [`TIER_CLAIM_HEADER`] are canonical everywhere and
//! Flight keeps accepting [`DEPRECATED_TENANT_CLAIM_ALIASES`], reported through
//! [`ClaimHit::deprecated`] so the surface can warn and the migration becomes
//! observable before anything is removed (TD-TENANT-3 S4).

/// Canonical wire name for the tenant assertion, on every header-carrying
/// surface (REST, gRPC, Arrow Flight).
///
/// HTTP/2 mandates lowercase header names on the wire, and `http::HeaderName`
/// compares case-insensitively — so this single constant is what REST's
/// historical `X-Tenant-ID` spelling already resolved to.
pub const TENANT_CLAIM_HEADER: &str = "x-tenant-id";

/// Canonical wire name for the tier entitlement claim (ADR-0053 W8).
pub const TIER_CLAIM_HEADER: &str = "x-tenant-tier";

/// pgwire startup parameter carrying the tier claim.
///
/// pgwire has no header channel, and the PostgreSQL startup-parameter grammar
/// forbids `-`, so [`TIER_CLAIM_HEADER`] cannot be spelled there. This is a
/// protocol-forced mapping, not drift.
pub const TIER_CLAIM_PG_PARAM: &str = "proximadb_tier";

/// Tenant-claim spellings Arrow Flight accepted before TD-TENANT-3, kept
/// working but deprecated. Callers that pass these to
/// [`tenant_claim_with_legacy_aliases`] receive `deprecated = true` so the
/// surface can emit a migration warning. Slated for removal in TD-TENANT-3 S4
/// once those warnings go quiet.
pub const DEPRECATED_TENANT_CLAIM_ALIASES: &[&str] =
    &["x-proximadb-tenant-id", "tenant-id", "tenant_id"];

/// A resolved claim: the trimmed value plus the wire name it actually arrived
/// under, so the surface can attribute and warn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimHit<'a> {
    /// The claim value, trimmed. Never empty — an empty claim is treated as
    /// absent, matching [`resolve_tenant_assertion`](crate::resolve_tenant_assertion).
    pub value: &'a str,
    /// The wire name this value was read from.
    pub name: &'static str,
    /// Whether `name` is a deprecated alias rather than the canonical name.
    pub deprecated: bool,
}

/// Read the first non-empty value among `names`, trimmed.
fn first_hit<'a, F>(names: &[&'static str], deprecated: bool, lookup: &F) -> Option<ClaimHit<'a>>
where
    F: Fn(&str) -> Option<&'a str>,
{
    names.iter().find_map(|name| {
        lookup(name)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| ClaimHit {
                value,
                name,
                deprecated,
            })
    })
}

/// Tenant claim under the canonical name only — the vocabulary for REST and
/// gRPC, which never accepted the legacy aliases and must not start now.
pub fn tenant_claim<'a, F>(lookup: F) -> Option<ClaimHit<'a>>
where
    F: Fn(&str) -> Option<&'a str>,
{
    first_hit(&[TENANT_CLAIM_HEADER], false, &lookup)
}

/// Tenant claim under the canonical name only — the vocabulary for every
/// header-carrying surface INCLUDING Arrow Flight.
///
/// TD-TENANT-3 S4 item 1 (honoring removed 2026-08-29): the legacy Flight
/// aliases no longer grant a tenant — a client sending only an alias resolves
/// to its credential/default tenant. Detection survives separately via
/// [`legacy_alias_present`] so the deprecation warn and
/// `proximadb_deprecated_claim_uses_total` counter keep firing (now counting
/// *attempts*); the names themselves are deleted at the next release
/// boundary. Flight therefore now uses the same vocabulary fn as REST/gRPC.
pub fn tenant_claim_with_legacy_aliases<'a, F>(lookup: F) -> Option<ClaimHit<'a>>
where
    F: Fn(&str) -> Option<&'a str>,
{
    tenant_claim(lookup)
}

/// Detect the PRESENCE of a deprecated Flight tenant-alias name, without
/// honoring it. Returns the name that was present (trimmed, non-empty) so the
/// caller can warn once and count the attempt. `None` when no legacy name is
/// present.
///
/// This is the signal half of the S4 retirement: the entitlement is gone
/// (see [`tenant_claim_with_legacy_aliases`]) but a legacy-sending client is
/// still observable instead of silently falling to another tenant.
pub fn legacy_alias_present<'a, F>(lookup: F) -> Option<&'static str>
where
    F: Fn(&str) -> Option<&'a str>,
{
    DEPRECATED_TENANT_CLAIM_ALIASES
        .iter()
        .find(|name| lookup(name).map(str::trim).is_some_and(|v| !v.is_empty()))
        .copied()
}

/// Tier entitlement claim under the canonical header name (REST, gRPC, Flight).
///
/// The value is *not* validated here — it stays an opaque operator-supplied id
/// until [`resolve_tier_claim`](crate::resolve_tier_claim) gates it and the
/// stamp normalizes it (TD-TENANT-3 S3).
pub fn tier_claim<'a, F>(lookup: F) -> Option<&'a str>
where
    F: Fn(&str) -> Option<&'a str>,
{
    first_hit(&[TIER_CLAIM_HEADER], false, &lookup).map(|hit| hit.value)
}

/// Tier entitlement claim from a pgwire startup-parameter map.
pub fn tier_claim_pg<'a, F>(lookup: F) -> Option<&'a str>
where
    F: Fn(&str) -> Option<&'a str>,
{
    first_hit(&[TIER_CLAIM_PG_PARAM], false, &lookup).map(|hit| hit.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn map(pairs: &[(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
        pairs.iter().copied().collect()
    }

    /// The accessor a surface adapter supplies.
    fn accessor<'a>(
        m: &'a HashMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> Option<&'static str> + 'a {
        move |name: &str| m.get(name).copied()
    }

    #[test]
    fn canonical_tenant_claim_is_read() {
        let m = map(&[("x-tenant-id", "acme")]);
        let hit = tenant_claim(accessor(&m)).expect("canonical claim");
        assert_eq!(hit.value, "acme");
        assert_eq!(hit.name, TENANT_CLAIM_HEADER);
        assert!(!hit.deprecated);
    }

    #[test]
    fn rest_grpc_vocabulary_ignores_legacy_aliases() {
        // The narrow-not-widen decision: these names must NOT become effective
        // on REST/gRPC just because Flight accepts them.
        for alias in DEPRECATED_TENANT_CLAIM_ALIASES {
            let m = map(&[(alias, "acme")]);
            assert!(
                tenant_claim(accessor(&m)).is_none(),
                "{alias} must not be honored outside Arrow Flight"
            );
        }
    }

    /// S4 item 1 (2026-08-29): honoring removed — a legacy alias alone NEVER
    /// grants a tenant, on any surface. This is the behavior change.
    #[test]
    fn legacy_aliases_no_longer_grant_a_tenant() {
        for alias in DEPRECATED_TENANT_CLAIM_ALIASES {
            let m = map(&[(alias, "acme")]);
            assert!(
                tenant_claim_with_legacy_aliases(accessor(&m)).is_none(),
                "{alias} must not grant a tenant (honoring removed, S4 item 1)"
            );
        }
    }

    /// The signal half: presence is still detected, per name, for the warn +
    /// attempt counter.
    #[test]
    fn legacy_alias_presence_is_detected_per_name() {
        for alias in DEPRECATED_TENANT_CLAIM_ALIASES {
            let m = map(&[(alias, "acme")]);
            assert_eq!(legacy_alias_present(accessor(&m)), Some(*alias));
        }
        // Canonical-only map: no legacy presence.
        let m = map(&[("x-tenant-id", "acme")]);
        assert_eq!(legacy_alias_present(accessor(&m)), None);
        // Blank legacy value: not present.
        let blank = map(&[("tenant_id", "   ")]);
        assert_eq!(legacy_alias_present(accessor(&blank)), None);
    }

    #[test]
    fn canonical_name_still_grants_via_the_flight_vocabulary() {
        let m = map(&[("x-tenant-id", "canonical"), ("tenant_id", "legacy")]);
        let hit = tenant_claim_with_legacy_aliases(accessor(&m)).expect("claim");
        assert_eq!(hit.value, "canonical");
        assert!(!hit.deprecated);
    }

    #[test]
    fn blank_and_whitespace_claims_are_absent() {
        for blank in ["", "   ", "\t"] {
            let m = map(&[("x-tenant-id", blank)]);
            assert!(tenant_claim(accessor(&m)).is_none());
            assert!(tenant_claim_with_legacy_aliases(accessor(&m)).is_none());
        }
    }

    #[test]
    fn values_are_trimmed() {
        let m = map(&[("x-tenant-id", "  acme  ")]);
        assert_eq!(tenant_claim(accessor(&m)).map(|h| h.value), Some("acme"));
    }

    #[test]
    fn tier_claim_uses_the_canonical_header_and_pg_parameter() {
        let headers = map(&[("x-tenant-tier", "enterprise")]);
        assert_eq!(tier_claim(accessor(&headers)), Some("enterprise"));
        // The header spelling is not accepted as a pgwire parameter, and the
        // pgwire spelling is not accepted as a header — each surface reads its
        // own protocol-forced name.
        assert_eq!(tier_claim_pg(accessor(&headers)), None);

        let params = map(&[("proximadb_tier", "pro")]);
        assert_eq!(tier_claim_pg(accessor(&params)), Some("pro"));
        assert_eq!(tier_claim(accessor(&params)), None);
    }

    #[test]
    fn canonical_names_are_lowercase_and_distinct() {
        // HTTP/2 mandates lowercase; a capitalized constant would silently miss
        // on `MetadataMap`, which does not case-normalize on lookup.
        for name in [TENANT_CLAIM_HEADER, TIER_CLAIM_HEADER] {
            assert_eq!(name, name.to_ascii_lowercase(), "{name} must be lowercase");
        }
        assert_ne!(TENANT_CLAIM_HEADER, TIER_CLAIM_HEADER);
        assert!(!DEPRECATED_TENANT_CLAIM_ALIASES.contains(&TENANT_CLAIM_HEADER));
    }
}

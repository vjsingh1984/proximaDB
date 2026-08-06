// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Tenant-assertion trust — the ONE shared primitive every network surface
//! uses to reconcile an *asserted* tenant (REST `X-Tenant-ID` header, gRPC /
//! Arrow Flight `x-tenant-id` metadata, pgwire startup `database` parameter /
//! session variable) against an *authenticated* tenant binding (JWT claim /
//! API-key mapping), under a deployment [`HeaderTrustPolicy`] (TD-TENANT-1).
//!
//! Before this module each surface reconciled independently and drifted:
//! REST rejected mismatches with 403, Arrow Flight hand-rolled the same
//! check, gRPC silently *ignored* a mismatched assertion, and pgwire
//! accepted any assertion. All four now call
//! [`resolve_tenant_assertion`]; the surface maps the result onto its own
//! error vocabulary (HTTP 403 / gRPC `PERMISSION_DENIED` / SQLSTATE 28000).

use serde::{Deserialize, Serialize};

/// Trust policy for a **bare** tenant assertion — a request that asserts a
/// tenant while carrying NO authenticated tenant binding. When a binding
/// exists, assertion≠binding is rejected in every mode; `GatewayOnly`
/// additionally lets an authenticated gateway principal delegate — select an
/// acting tenant via the assertion (the trusted-gateway topology: the gateway
/// authenticates with a service credential and stamps the end user's tenant
/// per request).
///
/// Configured via `[security.tenant] header_trust` in `config.toml`, or the
/// `PROXIMADB_TENANT_HEADER_TRUST` env override applied at server
/// construction. Unset ⇒ `Open` (default-safe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HeaderTrustPolicy {
    /// Accept the bare assertion verbatim. Correct for dev, single-tenant,
    /// and network-isolated trusted-gateway topologies. The default.
    #[default]
    Open,
    /// Reject any request that asserts a tenant without an authenticated
    /// binding. Credential-derived tenants (JWT/API-key) are unaffected.
    AuthenticatedOnly,
    /// Like `AuthenticatedOnly`, but an authenticated **gateway principal**
    /// (see [`AuthenticatedTenantBinding::is_gateway_principal`]) may assert
    /// a different tenant to act on its behalf (delegation).
    GatewayOnly,
}

impl std::fmt::Display for HeaderTrustPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::AuthenticatedOnly => write!(f, "authenticated-only"),
            Self::GatewayOnly => write!(f, "gateway-only"),
        }
    }
}

impl std::str::FromStr for HeaderTrustPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "open" => Ok(Self::Open),
            "authenticated-only" | "authenticated-match" => Ok(Self::AuthenticatedOnly),
            "gateway-only" => Ok(Self::GatewayOnly),
            other => Err(format!(
                "invalid tenant header-trust policy '{other}' \
                 (expected open | authenticated-only | gateway-only)"
            )),
        }
    }
}

impl HeaderTrustPolicy {
    /// The env override key, applied at server construction (never in
    /// constructors, so tests and embedded uses stay hermetic).
    pub const ENV_KEY: &'static str = "PROXIMADB_TENANT_HEADER_TRUST";

    /// Resolve the deployment-effective policy: env override > `configured`
    /// (config.toml `[security.tenant] header_trust`) > `preset` (deployment-
    /// mode default). Returns the policy plus an optional warning to log —
    /// an unparseable env value TIGHTENS to `AuthenticatedOnly` (fail-closed)
    /// instead of silently running whatever the fallback was.
    pub fn effective(preset: Self, configured: Option<Self>) -> (Self, Option<String>) {
        let fallback = configured.unwrap_or(preset);
        match std::env::var(Self::ENV_KEY) {
            Ok(raw) => match raw.parse::<Self>() {
                Ok(policy) => (policy, None),
                Err(e) => (
                    Self::AuthenticatedOnly,
                    Some(format!(
                        "invalid {}: {e}; tightening to authenticated-only",
                        Self::ENV_KEY
                    )),
                ),
            },
            Err(_) => (fallback, None),
        }
    }
}

/// The authenticated tenant binding a surface derived from a verified
/// credential (JWT `tenant_id` claim, API-key→tenant mapping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedTenantBinding {
    /// The tenant the credential is bound to.
    pub tenant_id: String,
    /// Whether the principal is a gateway/operator (first-class
    /// [`GATEWAY_ROLE`]/[`OPERATOR_ROLE`] role marker, or — compat — its
    /// bound tenant is in the deployment's system-tenant list). Only
    /// consulted by [`HeaderTrustPolicy::GatewayOnly`] delegation.
    pub is_gateway_principal: bool,
}

/// How the tenant identity was resolved. The surface maps this onto its own
/// source vocabulary (e.g. REST `TenantIdSource::Header` vs `JwtClaim`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTenantAssertion {
    /// The bare assertion was accepted (`Open`), or a gateway principal
    /// delegated to the asserted tenant (`GatewayOnly`).
    Asserted(String),
    /// The credential's tenant binding won (assertion absent or equal).
    Credential(String),
    /// Nothing asserted and nothing bound — the caller applies its
    /// deployment default (or rejects, if it requires a tenant).
    NoTenant,
}

/// Rejection reasons. Every variant is an isolation event worth an audit log
/// at the surface (`target: "proximadb::tenant_audit"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantAssertionError {
    /// A credentialed principal asserted a DIFFERENT tenant — the masquerade
    /// signature (and not a permitted gateway delegation).
    Mismatch {
        asserted: String,
        authenticated: String,
    },
    /// A bare assertion was rejected by a non-`Open` policy.
    UnauthenticatedAssertionRejected { asserted: String },
}

impl std::fmt::Display for TenantAssertionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mismatch {
                asserted,
                authenticated,
            } => write!(
                f,
                "tenant '{asserted}' does not match authenticated tenant '{authenticated}'"
            ),
            Self::UnauthenticatedAssertionRejected { asserted } => write!(
                f,
                "tenant '{asserted}' asserted without authenticated credentials; this \
                 deployment requires a tenant-bound credential"
            ),
        }
    }
}

/// Reconcile an asserted tenant against an authenticated binding under the
/// deployment policy. Pure — no I/O, no logging; surfaces own the mapping to
/// wire errors and the audit trail.
///
/// Truth table (A = asserted, B = binding):
///
/// | A | B | policy | result |
/// |---|---|--------|--------|
/// | – | – | any    | `NoTenant` |
/// | – | ✓ | any    | `Credential(B)` |
/// | ✓ | – | `Open` | `Asserted(A)` |
/// | ✓ | – | strict | `UnauthenticatedAssertionRejected` |
/// | ✓ | ✓, A==B | any | `Credential(B)` |
/// | ✓ | ✓, A≠B, gateway + `GatewayOnly` | | `Asserted(A)` (delegation) |
/// | ✓ | ✓, A≠B otherwise | any | `Mismatch` |
pub fn resolve_tenant_assertion(
    asserted: Option<&str>,
    binding: Option<&AuthenticatedTenantBinding>,
    policy: HeaderTrustPolicy,
) -> Result<ResolvedTenantAssertion, TenantAssertionError> {
    let asserted = asserted
        .map(str::trim)
        .filter(|tenant_id| !tenant_id.is_empty());

    match (asserted, binding) {
        (None, None) => Ok(ResolvedTenantAssertion::NoTenant),
        (None, Some(binding)) => Ok(ResolvedTenantAssertion::Credential(
            binding.tenant_id.clone(),
        )),
        (Some(asserted), None) => match policy {
            HeaderTrustPolicy::Open => Ok(ResolvedTenantAssertion::Asserted(asserted.to_string())),
            HeaderTrustPolicy::AuthenticatedOnly | HeaderTrustPolicy::GatewayOnly => {
                Err(TenantAssertionError::UnauthenticatedAssertionRejected {
                    asserted: asserted.to_string(),
                })
            }
        },
        (Some(asserted), Some(binding)) => {
            if asserted == binding.tenant_id {
                return Ok(ResolvedTenantAssertion::Credential(
                    binding.tenant_id.clone(),
                ));
            }
            if policy == HeaderTrustPolicy::GatewayOnly && binding.is_gateway_principal {
                return Ok(ResolvedTenantAssertion::Asserted(asserted.to_string()));
            }
            Err(TenantAssertionError::Mismatch {
                asserted: asserted.to_string(),
                authenticated: binding.tenant_id.clone(),
            })
        }
    }
}

// ===========================================================================
// Request identity — the unified tenant+subject+auth-class seam (TD-ABAC-6)
// ===========================================================================
//
// `resolve_tenant_assertion` (above) is already the ONE tenant gate every
// network surface calls (TD-TENANT-1). ABAC enforcement (TD-ABAC-2..5) added a
// parallel *subject* concern and surfaced that each surface re-implements the
// same identity pipeline. This section is the tenant+subject+auth-class
// analogue: a single `ResolvedRequestIdentity` every surface produces, with the
// trust-vs-auth distinction made explicit and auditable. Foundation hosts it
// because tenant/subject ids are plain strings here (no catalog dep); the
// `&str → SubjectId` lift stays at the root ABAC boundary.

/// How a request's identity was established (TD-ABAC-6). Makes the
/// trust-auth-vs-real-auth distinction — previously buried in comments (pgwire is
/// advisory/spoofable; gRPC/REST/Arrow are load-bearing) — auditable in the type
/// system. Informational in the consolidation refactor: it does NOT change
/// enforcement (a future hardening slice may refuse `TrustAsserted` as
/// load-bearing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AuthClass {
    /// A verified credential (API key / JWT / mTLS cert) was resolved by the
    /// `SecurityCoordinator`. Enforcement against this identity is load-bearing.
    Authenticated,
    /// Client-asserted with NO credential (pgwire trust auth, or a dev/embedded
    /// path with no coordinator). Enforcement is advisory — spoofable unless the
    /// deployment's [`HeaderTrustPolicy`] gates the assertion.
    TrustAsserted,
    /// Nothing resolved (no credential, no assertion, anonymous/dev). No subject;
    /// enforcement does not apply.
    #[default]
    Anonymous,
}

/// Canonical authorization-composition decision shared by every data read
/// seam. The authentication class is deliberately carried into the decision
/// for audit provenance, but never weakens enforcement: once an enforcer and a
/// subject are present, the stable tenant policy key is mandatory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadEnforcementComposition {
    Passthrough,
    ResolvePolicy,
    DenyMissingStableId,
}

/// Resolve the structural read-authorization state without depending on an
/// enforcement engine. Keeping this in the identity foundation prevents
/// vector, record, relational, and future modalities from inventing subtly
/// different fail-open cells.
pub const fn read_enforcement_composition(
    enforcer_wired: bool,
    subject_present: bool,
    stable_id_present: bool,
    _auth_class: AuthClass,
) -> ReadEnforcementComposition {
    if !enforcer_wired || !subject_present {
        ReadEnforcementComposition::Passthrough
    } else if !stable_id_present {
        ReadEnforcementComposition::DenyMissingStableId
    } else {
        ReadEnforcementComposition::ResolvePolicy
    }
}

impl std::fmt::Display for AuthClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authenticated => write!(f, "authenticated"),
            Self::TrustAsserted => write!(f, "trust-asserted"),
            Self::Anonymous => write!(f, "anonymous"),
        }
    }
}

/// The resolved identity every network surface produces uniformly (TD-ABAC-6):
/// the effective tenant, the principal (when one was resolved or policy-accepted),
/// and how it was established. Bare strings (foundation can't name the catalog's
/// `SubjectId`); the root `src/security/request_identity` lifts `subject` to
/// `SubjectId` at the ABAC boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRequestIdentity {
    /// The effective tenant after assertion-vs-binding reconciliation.
    pub tenant: String,
    /// The principal id, when one was resolved (credential) or policy-accepted
    /// (bare assertion). `None` ⇒ anonymous ⇒ no subject ⇒ no ABAC enforcement.
    pub subject: Option<String>,
    /// How the identity was established — the auditable trust/auth distinction.
    pub auth_class: AuthClass,
    /// ADR-087: the tenant's ADR-0083 stable u64 (the ABAC policy-lookup key),
    /// stamped ONCE at identity resolution by the wired
    /// [`TenantStableIdResolver`] — never re-derived downstream. `None` = no
    /// resolver wired or the tenant is unminted; the enforcement seam treats
    /// that cell fail-closed (TD-ABAC-11). Catalog bootstrap/provisioning makes
    /// the id total on supported production catalog backends.
    pub tenant_stable_id: Option<u64>,
}

impl ResolvedRequestIdentity {
    /// Stamp the stable id from the resolver (the ADR-087 stamp-once point).
    /// A `None` resolver or an unminted tenant leaves it `None`.
    pub fn stamp_stable_id(mut self, resolver: Option<&dyn TenantStableIdResolver>) -> Self {
        self.tenant_stable_id = resolver.and_then(|r| r.stable_id_of(&self.tenant));
        self
    }
}

/// Reconcile a client-asserted **subject** (no authenticated binding) under the
/// deployment policy — the subject analogue of [`resolve_tenant_assertion`]'s
/// `(Some, None)` bare-assertion arm, factored out so the subject gate is not
/// pgwire-only. `Open` (default) accepts the assertion; `AuthenticatedOnly` /
/// `GatewayOnly` reject it. `None`/empty/`"anonymous"` ⇒ no assertion ⇒ `Ok(None)`.
///
/// On authenticated surfaces the subject comes from the verified credential (no
/// assertion, no gate) — this function is only for the trust-asserted path.
pub fn resolve_subject_assertion(
    asserted: Option<&str>,
    policy: HeaderTrustPolicy,
) -> Result<Option<String>, TenantAssertionError> {
    let asserted = asserted
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "anonymous");
    match asserted {
        None => Ok(None),
        Some(subject) => match policy {
            HeaderTrustPolicy::Open => Ok(Some(subject.to_string())),
            HeaderTrustPolicy::AuthenticatedOnly | HeaderTrustPolicy::GatewayOnly => {
                Err(TenantAssertionError::UnauthenticatedAssertionRejected {
                    asserted: subject.to_string(),
                })
            }
        },
    }
}

/// Role marking a gateway/service principal permitted to delegate tenants
/// under [`HeaderTrustPolicy::GatewayOnly`]. Stamped from credential data
/// (e.g. a `gateway: true` JWT claim) at `UnifiedUserContext` construction.
pub const GATEWAY_ROLE: &str = "gateway";
/// Operator/control-plane role — also a gateway-capable principal.
pub const OPERATOR_ROLE: &str = "operator";

/// Port for resolving a tenant's ADR-031 stable `u64` id at the identity
/// boundary. Implemented by the catalog once tenant stable-id minting lands;
/// surfaces carry the resolved id on their tenant context so catalog/storage
/// keying can move off raw strings without re-resolving per operation.
/// `None` = no stable id minted for this tenant (yet) — callers must treat
/// the id as an optimization, never a second source of truth.
pub trait TenantStableIdResolver: Send + Sync {
    fn stable_id_of(&self, tenant_id: &str) -> Option<u64>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_read_enforcement_composition_truth_table() {
        for enforcer_wired in [false, true] {
            for subject_present in [false, true] {
                for stable_id_present in [false, true] {
                    for auth_class in [
                        AuthClass::Authenticated,
                        AuthClass::TrustAsserted,
                        AuthClass::Anonymous,
                    ] {
                        let expected = if !enforcer_wired || !subject_present {
                            ReadEnforcementComposition::Passthrough
                        } else if !stable_id_present {
                            ReadEnforcementComposition::DenyMissingStableId
                        } else {
                            ReadEnforcementComposition::ResolvePolicy
                        };
                        assert_eq!(
                            read_enforcement_composition(
                                enforcer_wired,
                                subject_present,
                                stable_id_present,
                                auth_class,
                            ),
                            expected,
                            "enforcer={enforcer_wired} subject={subject_present} \
                             stable_id={stable_id_present} auth_class={auth_class}"
                        );
                    }
                }
            }
        }
    }

    fn binding(tenant: &str, gateway: bool) -> AuthenticatedTenantBinding {
        AuthenticatedTenantBinding {
            tenant_id: tenant.to_string(),
            is_gateway_principal: gateway,
        }
    }

    #[test]
    fn nothing_asserted_nothing_bound_is_no_tenant() {
        for policy in [
            HeaderTrustPolicy::Open,
            HeaderTrustPolicy::AuthenticatedOnly,
            HeaderTrustPolicy::GatewayOnly,
        ] {
            assert_eq!(
                resolve_tenant_assertion(None, None, policy),
                Ok(ResolvedTenantAssertion::NoTenant)
            );
        }
    }

    #[test]
    fn credential_binding_wins_when_nothing_asserted() {
        for policy in [
            HeaderTrustPolicy::Open,
            HeaderTrustPolicy::AuthenticatedOnly,
            HeaderTrustPolicy::GatewayOnly,
        ] {
            assert_eq!(
                resolve_tenant_assertion(None, Some(&binding("acme", false)), policy),
                Ok(ResolvedTenantAssertion::Credential("acme".to_string()))
            );
        }
    }

    #[test]
    fn bare_assertion_accepted_only_under_open() {
        assert_eq!(
            resolve_tenant_assertion(Some("demo1"), None, HeaderTrustPolicy::Open),
            Ok(ResolvedTenantAssertion::Asserted("demo1".to_string()))
        );
        for policy in [
            HeaderTrustPolicy::AuthenticatedOnly,
            HeaderTrustPolicy::GatewayOnly,
        ] {
            assert_eq!(
                resolve_tenant_assertion(Some("demo1"), None, policy),
                Err(TenantAssertionError::UnauthenticatedAssertionRejected {
                    asserted: "demo1".to_string()
                })
            );
        }
    }

    #[test]
    fn matching_assertion_resolves_to_credential() {
        assert_eq!(
            resolve_tenant_assertion(
                Some("acme"),
                Some(&binding("acme", false)),
                HeaderTrustPolicy::AuthenticatedOnly
            ),
            Ok(ResolvedTenantAssertion::Credential("acme".to_string()))
        );
    }

    #[test]
    fn mismatch_rejected_for_non_gateway_in_every_mode() {
        for policy in [
            HeaderTrustPolicy::Open,
            HeaderTrustPolicy::AuthenticatedOnly,
            HeaderTrustPolicy::GatewayOnly,
        ] {
            assert_eq!(
                resolve_tenant_assertion(Some("victim"), Some(&binding("acme", false)), policy),
                Err(TenantAssertionError::Mismatch {
                    asserted: "victim".to_string(),
                    authenticated: "acme".to_string(),
                })
            );
        }
    }

    #[test]
    fn gateway_delegation_only_under_gateway_only() {
        assert_eq!(
            resolve_tenant_assertion(
                Some("demo1"),
                Some(&binding("system", true)),
                HeaderTrustPolicy::GatewayOnly
            ),
            Ok(ResolvedTenantAssertion::Asserted("demo1".to_string()))
        );
        for policy in [
            HeaderTrustPolicy::Open,
            HeaderTrustPolicy::AuthenticatedOnly,
        ] {
            assert!(matches!(
                resolve_tenant_assertion(Some("demo1"), Some(&binding("system", true)), policy),
                Err(TenantAssertionError::Mismatch { .. })
            ));
        }
    }

    #[test]
    fn whitespace_and_empty_assertions_are_treated_as_absent() {
        assert_eq!(
            resolve_tenant_assertion(Some("   "), None, HeaderTrustPolicy::AuthenticatedOnly),
            Ok(ResolvedTenantAssertion::NoTenant)
        );
        assert_eq!(
            resolve_tenant_assertion(Some(""), None, HeaderTrustPolicy::AuthenticatedOnly),
            Ok(ResolvedTenantAssertion::NoTenant)
        );
    }

    #[test]
    fn policy_parses_and_displays() {
        use std::str::FromStr;
        assert_eq!(
            HeaderTrustPolicy::from_str("open").unwrap(),
            HeaderTrustPolicy::Open
        );
        assert_eq!(
            HeaderTrustPolicy::from_str("AUTHENTICATED_MATCH").unwrap(),
            HeaderTrustPolicy::AuthenticatedOnly
        );
        assert_eq!(
            HeaderTrustPolicy::from_str("gateway-only").unwrap(),
            HeaderTrustPolicy::GatewayOnly
        );
        assert!(HeaderTrustPolicy::from_str("everything-goes").is_err());
        assert_eq!(HeaderTrustPolicy::default(), HeaderTrustPolicy::Open);
        assert_eq!(HeaderTrustPolicy::GatewayOnly.to_string(), "gateway-only");
    }

    #[test]
    fn policy_serde_round_trips_kebab_case() {
        assert_eq!(
            serde_json::to_string(&HeaderTrustPolicy::AuthenticatedOnly).unwrap(),
            "\"authenticated-only\""
        );
        assert_eq!(
            serde_json::from_str::<HeaderTrustPolicy>("\"gateway-only\"").unwrap(),
            HeaderTrustPolicy::GatewayOnly
        );
    }

    // --- TD-ABAC-6: resolve_subject_assertion + AuthClass ---

    #[test]
    fn subject_bare_assertion_accepted_only_under_open() {
        assert_eq!(
            resolve_subject_assertion(Some("alice"), HeaderTrustPolicy::Open),
            Ok(Some("alice".to_string()))
        );
        for policy in [
            HeaderTrustPolicy::AuthenticatedOnly,
            HeaderTrustPolicy::GatewayOnly,
        ] {
            assert!(matches!(
                resolve_subject_assertion(Some("alice"), policy),
                Err(TenantAssertionError::UnauthenticatedAssertionRejected { .. })
            ));
        }
    }

    #[test]
    fn subject_none_empty_or_anonymous_is_no_assertion() {
        for policy in [
            HeaderTrustPolicy::Open,
            HeaderTrustPolicy::AuthenticatedOnly,
            HeaderTrustPolicy::GatewayOnly,
        ] {
            assert_eq!(resolve_subject_assertion(None, policy), Ok(None));
            assert_eq!(resolve_subject_assertion(Some(""), policy), Ok(None));
            assert_eq!(resolve_subject_assertion(Some("  "), policy), Ok(None));
            assert_eq!(
                resolve_subject_assertion(Some("anonymous"), policy),
                Ok(None)
            );
        }
    }

    #[test]
    fn auth_class_displays_and_serdes_kebab() {
        assert_eq!(AuthClass::Authenticated.to_string(), "authenticated");
        assert_eq!(AuthClass::TrustAsserted.to_string(), "trust-asserted");
        assert_eq!(AuthClass::Anonymous.to_string(), "anonymous");
        assert_eq!(
            serde_json::to_string(&AuthClass::TrustAsserted).unwrap(),
            "\"trust-asserted\""
        );
        assert_eq!(
            serde_json::from_str::<AuthClass>("\"authenticated\"").unwrap(),
            AuthClass::Authenticated
        );
    }

    #[test]
    fn resolved_identity_carries_the_three_facets() {
        let id = ResolvedRequestIdentity {
            tenant: "acme".to_string(),
            subject: Some("alice".to_string()),
            auth_class: AuthClass::Authenticated,
            tenant_stable_id: Some(7),
        };
        assert_eq!(id.tenant, "acme");
        assert_eq!(id.subject.as_deref(), Some("alice"));
        assert_eq!(id.auth_class, AuthClass::Authenticated);
        assert_eq!(id.tenant_stable_id, Some(7));
    }
}

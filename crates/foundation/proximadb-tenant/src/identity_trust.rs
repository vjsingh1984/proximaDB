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
}

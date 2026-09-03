// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
= TD-SSO-3: remove the dead legacy enterprise-SSO stack (crate + consumers)

[cols="1,3", options="header"]
|===
| Field | Value
| Status | Landing
| Severity | Medium — deletes a quarantined credential-adjacent hazard surface (~2,800 LOC)
| Component | `crates/horizontal/proximadb-auth-sso` (removed); `src/auth/`, `src/api_handlers/enterprise.rs` (removed)
| Relates to | [[TD-SSO-1]] (OIDC seam — the replacement); [[TD-DECOMP-21]] (the extraction being reversed); [[TD-SSO-2]] (multi-provider successor)
|===

== Why

TD-SSO-1 shipped the real OIDC resource server; TD-SSO-2/portability (#1804) removed
the legacy `SSOConfig.aws_iam`/`azure_ad` config surface. What remained was the
extracted crate itself plus its root-crate consumers — all dead:

* `EnterpriseAuthManager` (`src/auth/mod.rs`) — zero constructions outside tests;
  its only consumer was `api_handlers/enterprise.rs`.
* `api_handlers/enterprise.rs` (1,114 LOC) — zero route registrations anywhere.
* `auth/federated_delegation_complete.rs` (1,197 LOC) — a parallel AWS/Azure
  delegation path with zero callers (flagged by the #1804 round-4 review).
* `auth/rbac.rs` — a local RBAC copy; the live manager is
  `storage::tenant::rbac::EnhancedRBACManager`.
* The crate's provider validators (`aws_iam`, `azure_ad`) were `#[cfg(test)]`
  stubs since #1800 quarantined the system_admin-without-verification shape.

Keeping dead credential-adjacent code is a standing hazard: every future refactor
risks re-wiring a stub that authenticates without verification.

== What was removed

* `crates/horizontal/proximadb-auth-sso/` entirely (workspace member, workspace
  dependency, root dependency).
* `src/auth/federated_delegation_complete.rs`, `src/auth/rbac.rs`,
  `src/api_handlers/enterprise.rs` (+ its `pub mod` decl).
* `EnterpriseAuthManager` and the `pub use proximadb_auth_sso::sso` re-export.

== What was kept (relocated verbatim into `src/auth/mod.rs`)

`EnterpriseUserContext` (+ `SecurityClearance`, `ProviderUserContext`,
`enterprise_to_storage_user_context`) — the identity parameter type threaded
through `src/ai/{llm,nlp,insights,natural_language_api}.rs` and `src/audit/mod.rs`.
**Nothing constructs it from a verified token today**; when TD-SSO-2 lands, map
verified OIDC claims into it at those seams. The field set is unchanged — this is
a removal, not a redesign.

== Not removed (separate census, not security-adjacent)

The `src/ai` enterprise-intelligence modules that take
`&EnterpriseUserContext` params (`llm`, `nlp`, `insights`, `natural_language_api`)
have zero external consumers — candidates for their own removal TD. Left in place
to keep this PR single-purpose.

== Verification

* `cargo check -p proximadb --lib` and `--tests` clean; workspace member list
  and layering guards pass (no crate depended on auth-sso besides root).
* Negative census: `grep -rn 'EnterpriseAuthManager|SSOIntegrationManager|
  federated_delegation|api_handlers::enterprise|auth::rbac'` → zero non-comment
  hits outside `storage::tenant` RBAC (live).
* New unit tests pin the relocated type (`system_admin()` shape, converter).

// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
= TD-SSO-2: multi-provider OIDC — issuer-based routing across multiple IdPs
:icons: font

**Status:** Filed 2026-09-02 (deferred from the provider-portability PR).
**Origin:** the portability audit ranked multi-provider #7 of 7 — needed the
moment a deployment fronts two IdPs (e.g. internal Kanidm for services +
Azure Entra for enterprise users), but architecturally larger than the
per-provider config fixes that shipped alongside this TD's filing.

== The gap

`AuthenticationConfig.oidc: Option<OidcProviderConfig>` accepts exactly ONE
provider. The dispatch between local-HS* and OIDC-RS*/ES* is by the
unverified JWT `alg` header; there is no issuer-based routing.

== The design (for the implementing PR)

**Route on unverified `iss` to SELECT a verifier, then verify.** The same
safety argument as the existing `alg` routing: the unauthenticated claim
only chooses which cryptographically-validating verifier runs; it never
authorizes.

```rust
// AuthenticationConfig
pub oidc_providers: Vec<OidcProviderConfig>,  // replaces Option<...>

// authenticate_jwt_token:
// 1. decode_header → if RS*/ES*:
// 2. Peek unverified iss claim (base64-decode payload, read "iss" — no trust)
// 3. Match against each provider's issuer_url + issuer_aliases
// 4. Route to the matching OidcTokenVerifier (which does full validation)
// 5. No match → reject (fail-closed)
```

== Considerations

* **Backward compat:** `oidc: Option<OidcProviderConfig>` deserializes into
  `oidc_providers: Vec<OidcProviderConfig>` via a serde helper (single → vec).
* **Per-provider role sanitization:** each provider carries its own
  `role_allowlist` and `allow_delegation_roles` — an Azure group and a
  Kanidm group of the same name need not have the same engine entitlement.
* **Per-provider tenant claim:** Azure uses `tid`, Kanidm uses a custom
  claim — already independently configurable.
* **JWKS caching:** one cache per provider (the current `OidcTokenVerifier`
  already encapsulates this — just keep one instance per provider).
* **The subject namespace:** `oidc:{sub}` may collide across providers
  (different IdPs can have the same subject UUID). Consider
  `{provider_name}:{sub}` or hashing the issuer into the prefix.

== Non-goals (unchanged from TD-SSO-1)

* No browser redirect/PKCE flows (resource server only).
* No dynamic provider registration (config-file only).

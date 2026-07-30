// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Shared request-identity helpers (TD-ABAC-6) — the root-crate half of the
//! unified identity seam.
//!
//! The foundation half (`AuthClass`, `ResolvedRequestIdentity`,
//! `resolve_subject_assertion`) lives in `proximadb_tenant::identity_trust`
//! (tenant/subject ids are bare strings there — no catalog dep). This module
//! holds the pieces that MUST live in the root crate because they name
//! `SecurityCoordinator` / `AuthenticationData` (security) or `SubjectId`
//! (catalog): the credential parser and the `SubjectId` lift. The full
//! `resolve_request_identity` orchestrator lands with the gRPC/REST wiring
//! (PR-B); this PR-A seeds the module with the credential parser every surface
//! reuses.

use crate::security::AuthenticationData;

/// Parse an `authorization` header/metadata **value** into
/// [`AuthenticationData`] — the ONE credential parser every network surface
/// reuses.
///
/// Collapses three near-identical copies: gRPC `auth_data_from_headers`
/// (`src/network/grpc/auth.rs`), Arrow `auth_data_from_metadata`
/// (`src/network/arrow_ipc/service.rs`), and REST `map_header_to_auth_data`
/// (`src/network/auth/middleware.rs`) — all three implemented the same
/// `Bearer` / `API-Key` / `Api-Key` / raw-as-ApiKey prefix logic.
///
/// Each surface's adapter extracts the raw value from its transport
/// (`http::HeaderMap` / `tonic::MetadataMap` / pgwire startup params) and keeps
/// only its surface-specific concerns: required-vs-optional semantics and any
/// extra fallbacks (e.g. Arrow's mTLS peer-cert and `x-api-key`/`api-key`
/// header fallbacks). The shared scheme-parsing lives here.
pub fn parse_authorization(value: &str) -> AuthenticationData {
    if let Some(token) = value.strip_prefix("Bearer ") {
        AuthenticationData::JWTToken(token.to_string())
    } else if let Some(key) = value
        .strip_prefix("API-Key ")
        .or_else(|| value.strip_prefix("Api-Key "))
    {
        AuthenticationData::ApiKey(key.to_string())
    } else {
        // No recognized scheme → treat the raw value as an API key. This is the
        // legacy behavior all three surfaces had; preserving it keeps the
        // consolidation behavior-neutral.
        AuthenticationData::ApiKey(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_prefix_is_jwt() {
        assert!(matches!(
            parse_authorization("Bearer eyJabc.def.ghi"),
            AuthenticationData::JWTToken(t) if t == "eyJabc.def.ghi"
        ));
    }

    #[test]
    fn api_key_prefixes_are_api_key() {
        assert!(matches!(
            parse_authorization("API-Key secret_123"),
            AuthenticationData::ApiKey(k) if k == "secret_123"
        ));
        assert!(matches!(
            parse_authorization("Api-Key secret_456"),
            AuthenticationData::ApiKey(k) if k == "secret_456"
        ));
    }

    #[test]
    fn raw_value_falls_back_to_api_key() {
        // Legacy behavior: an unrecognized scheme is treated as a bare API key.
        assert!(matches!(
            parse_authorization("bare-key-no-scheme"),
            AuthenticationData::ApiKey(k) if k == "bare-key-no-scheme"
        ));
    }
}

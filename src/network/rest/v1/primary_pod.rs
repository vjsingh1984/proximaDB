/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! REST operator endpoints for the primary-pod registry (Slice 3 of
//! `docs/12-design/TENANT_COLLECTION_POD_AFFINITY_2026_05_27.adoc`).
//!
//! These endpoints expose the durable (tenant_id, collection_id) →
//! primary_pod registry over HTTP for cluster operators. They are
//! **deliberately restricted** to system-operator roles because:
//!
//! * The bindings drive WAL write routing. A malicious or accidental
//!   re-assignment can cause silent data loss — writes land on a pod
//!   whose memtable will never be searched on the reader's pod.
//! * They expose cross-tenant placement. A naive read endpoint would
//!   leak "tenant X's writes go to pod Y" to every authenticated
//!   user.
//! * They are infrastructure-level decisions, not tenant-level. A
//!   tenant administrator doesn't choose their pod placement; the
//!   cluster operator does.
//!
//! ## Auth gate
//!
//! Every handler in this module calls [`authorize_operator`] before
//! touching the registry. The gate:
//!
//! 1. Extracts the `UnifiedUserContext` that
//!    [`crate::network::auth::middleware::auth_middleware_unified`]
//!    injected into the request extensions. Missing context → 401
//!    Unauthorized (means the middleware was bypassed or this
//!    endpoint was mounted without auth — both bugs).
//! 2. Checks the user's `effective_permissions` for either
//!    [`UnifiedPermission::SystemAdmin`] or
//!    [`UnifiedPermission::ConfigureSystem`]. Either is sufficient;
//!    `SystemAdmin` is the strict superset, `ConfigureSystem` is the
//!    narrower "infra operator" role.
//! 3. Returns 403 Forbidden on any other context (regular tenant
//!    user, even a `TenantAdmin`).
//!
//! ## Routes
//!
//! * `GET /api/v1/primary-pod/:tenant_id/:collection_id` — read
//!   current binding. 200 with `Bound { ... }` or `Unbound { ... }`.
//! * `PUT /api/v1/primary-pod/:tenant_id/:collection_id` — assign or
//!   re-assign. Body: `{ "pod": "...", "reason": "operator" }`.
//!   200 with the new state and the optional previous binding.
//! * `DELETE /api/v1/primary-pod/:tenant_id/:collection_id` — unbind.
//!   200 with `{ removed: bool }`.
//! * `GET /api/v1/primary-pod` — list every assignment. 200 with a
//!   `count`-prefixed array sorted by `(tenant_id, collection_id)`.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};

use crate::cluster::primary_pod_registry::{AssignmentReason, PrimaryPod};
use crate::network::rest::v1::handlers::AppState;
use crate::security::rbac_service::{UnifiedPermission, UnifiedUserContext};

/// Standard error payload for the operator endpoints. Mirrors the
/// shape used by the other v1 operator routes — `code` is the HTTP
/// status, `error` is a stable short label for automated handling,
/// `message` is human-readable.
#[derive(Debug, Serialize)]
pub struct OperatorErrorResponse {
    pub error: &'static str,
    pub message: String,
    pub code: u16,
}

/// Authorize an incoming request against the operator-permission
/// gate. Returns `Ok(user_id)` on success; otherwise an
/// `(StatusCode, Json<OperatorErrorResponse>)` tuple suitable for
/// direct `?`-propagation from a handler.
///
/// Outcomes:
///
/// * **401 Unauthorized** — no `UnifiedUserContext` in extensions.
///   Indicates the auth middleware was bypassed or never ran;
///   defaults to deny.
/// * **403 Forbidden** — context present, but permissions don't
///   include `SystemAdmin` nor `ConfigureSystem`.
/// * **Ok(user_id)** — context present and authorized; caller
///   proceeds. The returned user_id is intended for audit logging.
pub fn authorize_operator(
    user_context: Option<&UnifiedUserContext>,
) -> Result<String, (StatusCode, Json<OperatorErrorResponse>)> {
    let Some(ctx) = user_context else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(OperatorErrorResponse {
                error: "missing_auth_context",
                message: "auth context not present — middleware misconfigured".to_string(),
                code: 401,
            }),
        ));
    };

    let allowed = ctx
        .effective_permissions
        .contains(&UnifiedPermission::SystemAdmin)
        || ctx
            .effective_permissions
            .contains(&UnifiedPermission::ConfigureSystem);

    if allowed {
        Ok(ctx.user_id.clone())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(OperatorErrorResponse {
                error: "operator_permission_required",
                message:
                    "primary-pod endpoints require SystemAdmin or ConfigureSystem permission"
                        .to_string(),
                code: 403,
            }),
        ))
    }
}

// ── Request / response payloads ────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AssignRequest {
    /// Pod identifier — typically a k8s pod name. Treated as opaque;
    /// the registry does not validate format or reachability.
    pub pod: String,
    /// Why this assignment is happening. Defaults to `Operator` when
    /// omitted — the natural fit for REST-driven changes.
    #[serde(default = "default_assign_reason")]
    pub reason: AssignmentReason,
}

fn default_assign_reason() -> AssignmentReason {
    AssignmentReason::Operator
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LookupResponse {
    Bound {
        tenant_id: String,
        collection_id: String,
        primary: PrimaryPod,
    },
    Unbound {
        tenant_id: String,
        collection_id: String,
    },
}

#[derive(Debug, Serialize)]
pub struct AssignResponse {
    pub tenant_id: String,
    pub collection_id: String,
    pub primary: PrimaryPod,
    /// The previous binding when this was a re-assignment; `None`
    /// when this was the first assignment.
    pub previous: Option<PrimaryPod>,
}

#[derive(Debug, Serialize)]
pub struct UnassignResponse {
    pub tenant_id: String,
    pub collection_id: String,
    /// True when a binding was actually removed; false when nothing
    /// was bound. Helps operators distinguish "no-op" from "ok".
    pub removed: bool,
}

#[derive(Debug, Serialize)]
pub struct ListItem {
    pub tenant_id: String,
    pub collection_id: String,
    pub primary: PrimaryPod,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub count: usize,
    pub items: Vec<ListItem>,
}

// ── Handlers ───────────────────────────────────────────────────────

/// `GET /api/v1/primary-pod/:tenant_id/:collection_id`
pub async fn get_primary_pod(
    Extension(user_context): Extension<Option<UnifiedUserContext>>,
    State(state): State<AppState>,
    Path((tenant_id, collection_id)): Path<(String, String)>,
) -> Result<Json<LookupResponse>, (StatusCode, Json<OperatorErrorResponse>)> {
    authorize_operator(user_context.as_ref())?;

    match state.primary_pod_registry.lookup(&tenant_id, &collection_id) {
        Some(primary) => Ok(Json(LookupResponse::Bound {
            tenant_id,
            collection_id,
            primary,
        })),
        None => Ok(Json(LookupResponse::Unbound {
            tenant_id,
            collection_id,
        })),
    }
}

/// `PUT /api/v1/primary-pod/:tenant_id/:collection_id`
pub async fn put_primary_pod(
    Extension(user_context): Extension<Option<UnifiedUserContext>>,
    State(state): State<AppState>,
    Path((tenant_id, collection_id)): Path<(String, String)>,
    Json(body): Json<AssignRequest>,
) -> Result<Json<AssignResponse>, (StatusCode, Json<OperatorErrorResponse>)> {
    let user_id = authorize_operator(user_context.as_ref())?;

    let previous = state.primary_pod_registry.assign(
        tenant_id.clone(),
        collection_id.clone(),
        body.pod.clone(),
        body.reason,
    );
    // The `assign` call above unconditionally inserts the entry; the
    // following `lookup` reads the same map and is guaranteed to find it.
    #[allow(clippy::expect_used)]
    let primary = state
        .primary_pod_registry
        .lookup(&tenant_id, &collection_id)
        .expect("lookup must succeed immediately after assign");

    tracing::info!(
        target = "proximadb.primary_pod.audit",
        operator = %user_id,
        tenant_id = %tenant_id,
        collection_id = %collection_id,
        pod = %primary.pod,
        reason = primary.reason.label(),
        had_previous = previous.is_some(),
        "primary-pod assignment applied via REST"
    );

    Ok(Json(AssignResponse {
        tenant_id,
        collection_id,
        primary,
        previous,
    }))
}

/// `DELETE /api/v1/primary-pod/:tenant_id/:collection_id`
pub async fn delete_primary_pod(
    Extension(user_context): Extension<Option<UnifiedUserContext>>,
    State(state): State<AppState>,
    Path((tenant_id, collection_id)): Path<(String, String)>,
) -> Result<Json<UnassignResponse>, (StatusCode, Json<OperatorErrorResponse>)> {
    let user_id = authorize_operator(user_context.as_ref())?;

    let removed = state
        .primary_pod_registry
        .unassign(&tenant_id, &collection_id);

    tracing::info!(
        target = "proximadb.primary_pod.audit",
        operator = %user_id,
        tenant_id = %tenant_id,
        collection_id = %collection_id,
        removed = removed.is_some(),
        "primary-pod unassign via REST"
    );

    Ok(Json(UnassignResponse {
        tenant_id,
        collection_id,
        removed: removed.is_some(),
    }))
}

/// `GET /api/v1/primary-pod`
pub async fn list_primary_pods(
    Extension(user_context): Extension<Option<UnifiedUserContext>>,
    State(state): State<AppState>,
) -> Result<Json<ListResponse>, (StatusCode, Json<OperatorErrorResponse>)> {
    authorize_operator(user_context.as_ref())?;

    let items: Vec<ListItem> = state
        .primary_pod_registry
        .list()
        .into_iter()
        .map(|(tenant_id, collection_id, primary)| ListItem {
            tenant_id,
            collection_id,
            primary,
        })
        .collect();
    let count = items.len();
    Ok(Json(ListResponse { count, items }))
}

#[cfg(test)]
mod tests {
    //! Focused unit tests for the security gate. End-to-end HTTP
    //! tests live in the integration suite; here we lock in the
    //! `authorize_operator` contract so the security guarantee
    //! survives future refactors.

    use super::*;
    use crate::security::rbac_service::UnifiedAuthMethod;
    use chrono::Utc;
    use std::collections::{HashMap, HashSet};

    fn ctx_with_permissions(perms: Vec<UnifiedPermission>) -> UnifiedUserContext {
        UnifiedUserContext {
            user_id: "test-user".to_string(),
            tenant_id: Some("tenant-x".to_string()),
            roles: vec!["test".to_string()],
            effective_permissions: perms.into_iter().collect::<HashSet<_>>(),
            auth_method: UnifiedAuthMethod::ApiKey,
            session_id: "test-session".to_string(),
            expires_at: None,
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn authorize_rejects_missing_context_with_401() {
        let result = authorize_operator(None);
        let (status, body) = result.expect_err("missing context must reject");
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body.0.code, 401);
        assert_eq!(body.0.error, "missing_auth_context");
    }

    #[test]
    fn authorize_rejects_tenant_admin_with_403() {
        // TenantAdmin is the strongest tenant-level permission, but
        // primary-pod endpoints require SYSTEM-level operator
        // privilege. This test locks in that boundary.
        let ctx = ctx_with_permissions(vec![UnifiedPermission::TenantAdmin]);
        let result = authorize_operator(Some(&ctx));
        let (status, body) = result.expect_err("TenantAdmin alone must reject");
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.0.code, 403);
        assert_eq!(body.0.error, "operator_permission_required");
    }

    #[test]
    fn authorize_rejects_collection_read_with_403() {
        // A regular user with collection-read perms must not reach
        // the registry. Operators can read any binding once
        // authorized; tenants must NOT read another tenant's
        // primary_pod even for their own collections.
        let ctx = ctx_with_permissions(vec![
            UnifiedPermission::CollectionRead("any".to_string()),
            UnifiedPermission::VectorSearch("any".to_string()),
        ]);
        let result = authorize_operator(Some(&ctx));
        let (status, _) = result.expect_err("regular user must reject");
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn authorize_accepts_system_admin() {
        let ctx = ctx_with_permissions(vec![UnifiedPermission::SystemAdmin]);
        let user_id = authorize_operator(Some(&ctx))
            .expect("SystemAdmin must be allowed through the gate");
        assert_eq!(user_id, "test-user");
    }

    #[test]
    fn authorize_accepts_configure_system() {
        let ctx = ctx_with_permissions(vec![UnifiedPermission::ConfigureSystem]);
        let user_id = authorize_operator(Some(&ctx))
            .expect("ConfigureSystem must be allowed through the gate");
        assert_eq!(user_id, "test-user");
    }

    #[test]
    fn authorize_accepts_either_permission() {
        // Confirms the gate is OR not AND — either permission is
        // sufficient. Operators with the narrower `ConfigureSystem`
        // don't need to be granted `SystemAdmin` too.
        let admin_ctx = ctx_with_permissions(vec![UnifiedPermission::SystemAdmin]);
        let config_ctx = ctx_with_permissions(vec![UnifiedPermission::ConfigureSystem]);
        assert!(authorize_operator(Some(&admin_ctx)).is_ok());
        assert!(authorize_operator(Some(&config_ctx)).is_ok());
    }

    #[test]
    fn default_assign_reason_is_operator() {
        // REST-driven changes default to `Operator` rather than e.g.
        // `Create`, because `Create` is reserved for the catalog's
        // initial assignment at collection-create time. Lock this in
        // so a refactor doesn't silently change the audit trail.
        assert_eq!(default_assign_reason(), AssignmentReason::Operator);
    }
}

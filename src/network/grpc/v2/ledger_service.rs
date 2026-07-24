// Copyright 2026 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! gRPC V2 ledger service — a thin facade over the shared `LedgerService` port
//! (crate `proximadb-ledger`, ADR-071 / TD-LEDGER-1).
//!
//! Each RPC delegates to the port and owns **no** enforcement logic. Tenant isolation is
//! structural: `resolved_tenant_id` namespaces every scope + CAS key in the port, never a
//! per-request predicate. The server stamps `now` (the port/core stays clock-free and
//! deterministic). Neutral units only — every quantity is a count, never a price.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tonic::{Request, Response, Status};

use proximadb_ledger::{CasError, DurableLedger, LedgerService, Policy, ReserveOutcome};

use crate::network::grpc::auth as grpc_auth;
use crate::proto::proximadb_v2 as pv2;
use crate::proto::proximadb_v2::proxima_ledger_service_server::{
    ProximaLedgerService, ProximaLedgerServiceServer,
};

/// Wall-clock nanoseconds since the Unix epoch — the `now` the port stamps onto leases.
fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Wire enum (`LedgerPolicy`) → the port's [`Policy`]. Unspecified/unknown ⇒ `Block` (the safe cap).
fn policy_from_wire(value: i32) -> Policy {
    match pv2::LedgerPolicy::try_from(value) {
        Ok(pv2::LedgerPolicy::Warn) => Policy::Warn,
        _ => Policy::Block,
    }
}

/// The port's [`Policy`] → the wire enum discriminant.
fn policy_to_wire(policy: Policy) -> i32 {
    match policy {
        Policy::Block => pv2::LedgerPolicy::Block as i32,
        Policy::Warn => pv2::LedgerPolicy::Warn as i32,
    }
}

/// gRPC V2 ledger service — thin delegating facade over `LedgerService`.
pub struct ProximaLedgerServiceImpl {
    ledger: Arc<LedgerService<DurableLedger>>,
}

impl ProximaLedgerServiceImpl {
    /// Build from the shared, durable ledger port (constructed once at startup, shared across
    /// requests — never per-RPC).
    pub fn new(ledger: Arc<LedgerService<DurableLedger>>) -> Self {
        Self { ledger }
    }

    /// Convert to a tonic server.
    pub fn into_server(self) -> ProximaLedgerServiceServer<Self> {
        ProximaLedgerServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl ProximaLedgerService for ProximaLedgerServiceImpl {
    async fn set_limit(
        &self,
        request: Request<pv2::SetLimitRequest>,
    ) -> Result<Response<pv2::SetLimitResponse>, Status> {
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let req = request.into_inner();
        if req.scope.trim().is_empty() {
            return Err(Status::invalid_argument("scope is required"));
        }
        self.ledger
            .set_limit(&tenant, &req.scope, req.limit, policy_from_wire(req.policy));
        Ok(Response::new(pv2::SetLimitResponse {}))
    }

    async fn reserve(
        &self,
        request: Request<pv2::ReserveRequest>,
    ) -> Result<Response<pv2::ReserveResponse>, Status> {
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let req = request.into_inner();
        if req.scope.trim().is_empty() {
            return Err(Status::invalid_argument("scope is required"));
        }
        if req.ttl_ns <= 0 {
            return Err(Status::invalid_argument("ttl_ns must be positive"));
        }
        let response =
            match self
                .ledger
                .reserve(&tenant, &req.scope, req.ceiling, now_ns(), req.ttl_ns)
            {
                ReserveOutcome::Admitted(r) => pv2::ReserveResponse {
                    admitted: true,
                    reservation_id: r.id,
                    expires_at_ns: r.expires_at_ns,
                    limit: 0,
                    spent: 0,
                    reserved: 0,
                },
                ReserveOutcome::Denied(d) => pv2::ReserveResponse {
                    admitted: false,
                    reservation_id: 0,
                    expires_at_ns: 0,
                    limit: d.limit,
                    spent: d.spent,
                    reserved: d.reserved,
                },
            };
        Ok(Response::new(response))
    }

    async fn settle(
        &self,
        request: Request<pv2::SettleRequest>,
    ) -> Result<Response<pv2::SettleResponse>, Status> {
        // Settle is verified against the caller's tenant: a reservation id belonging to another
        // tenant is refused (the reservation's owner is recovered from its namespaced scope).
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let req = request.into_inner();
        if self.ledger.settle(&tenant, req.reservation_id, req.actual) {
            Ok(Response::new(pv2::SettleResponse {}))
        } else {
            Err(Status::permission_denied(
                "reservation does not belong to this tenant",
            ))
        }
    }

    async fn compare_and_swap(
        &self,
        request: Request<pv2::CompareAndSwapRequest>,
    ) -> Result<Response<pv2::CompareAndSwapResponse>, Status> {
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let req = request.into_inner();
        if req.key.trim().is_empty() {
            return Err(Status::invalid_argument("key is required"));
        }
        let response =
            match self
                .ledger
                .compare_and_swap(&tenant, &req.key, req.expected_version, req.value)
            {
                Ok(version) => pv2::CompareAndSwapResponse {
                    swapped: true,
                    version,
                    actual_present: false,
                    actual_version: 0,
                },
                Err(CasError::VersionMismatch { actual, .. }) => pv2::CompareAndSwapResponse {
                    swapped: false,
                    version: 0,
                    actual_present: actual.is_some(),
                    actual_version: actual.unwrap_or(0),
                },
            };
        Ok(Response::new(response))
    }

    async fn get_scope(
        &self,
        request: Request<pv2::GetScopeRequest>,
    ) -> Result<Response<pv2::GetScopeResponse>, Status> {
        let tenant = grpc_auth::resolved_tenant_id(&request)?;
        let req = request.into_inner();
        if req.scope.trim().is_empty() {
            return Err(Status::invalid_argument("scope is required"));
        }
        Ok(Response::new(pv2::GetScopeResponse {
            limit: self.ledger.limit(&tenant, &req.scope),
            spent: self.ledger.spent(&tenant, &req.scope),
            reserved: self.ledger.reserved(&tenant, &req.scope),
            policy: policy_to_wire(self.ledger.policy(&tenant, &req.scope)),
        }))
    }
}

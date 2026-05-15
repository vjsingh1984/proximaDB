//! Shared REST application state.
//!
//! `RestAppState` is the Axum state type injected into every platform REST handler.
//! It holds only port traits from `proximadb-runtime` so no root-crate concrete types
//! cross the crate boundary.

use std::sync::Arc;

use proximadb_runtime::ApiHandlersPort;

/// Tenant context extracted from request headers/JWT and injected as an Axum Extension.
///
/// Handlers receive this via `Extension(tenant): Extension<TenantContext>`.  The
/// `tenant_id` is passed as `Option<&str>` to port methods which resolve it internally.
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub tenant_id: String,
}

impl TenantContext {
    pub fn new(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
        }
    }

    /// Convenience for tests and middleware that need a default tenant.
    pub fn default_tenant() -> Self {
        Self::new("default")
    }
}

/// Axum application state shared by all platform REST v1 handlers.
///
/// All service dependencies are expressed as port traits so the handlers can
/// be compiled independently of root-crate concrete service types.
#[derive(Clone)]
pub struct RestAppState {
    /// Primary API port — collection, vector, hybrid, and SQL operations.
    pub handlers: Arc<dyn ApiHandlersPort>,
}

impl RestAppState {
    pub fn new(handlers: Arc<dyn ApiHandlersPort>) -> Self {
        Self { handlers }
    }
}

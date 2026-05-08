use std::collections::HashMap;

/// Modality-neutral execution metadata shared by query runtimes.
///
/// This intentionally excludes modality scope such as `graph_id`, collection,
/// namespace, or table name. Those belong in modality-specific contexts that
/// embed this common contract.
#[derive(Debug, Clone, Default)]
pub struct ExecutionContext {
    /// Caller-supplied request or trace identifier.
    pub request_id: Option<String>,
    /// Tenant identifier when execution is scoped to a tenant.
    pub tenant_id: Option<String>,
    /// Authenticated principal, if available.
    pub principal: Option<String>,
    /// Query/runtime parameters supplied by the caller.
    pub parameters: HashMap<String, serde_json::Value>,
    /// Resource limits and execution controls.
    pub limits: ExecutionLimits,
    /// Whether detailed execution statistics should be collected.
    pub collect_stats: bool,
}

impl ExecutionContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    pub fn with_principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = Some(principal.into());
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.limits.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn with_memory_limit(mut self, limit: usize) -> Self {
        self.limits.memory_limit_bytes = Some(limit);
        self
    }

    pub fn with_stats(mut self) -> Self {
        self.collect_stats = true;
        self
    }
}

/// Common resource limits used by execution contexts.
#[derive(Debug, Clone, Default)]
pub struct ExecutionLimits {
    /// Execution timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Maximum memory limit in bytes.
    pub memory_limit_bytes: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn execution_context_preserves_common_scope_and_limits() {
        let mut context = ExecutionContext::new()
            .with_request_id("req-1")
            .with_tenant_id("tenant-a")
            .with_principal("alice")
            .with_timeout(500)
            .with_memory_limit(4096)
            .with_stats();
        context.parameters.insert("k".to_string(), json!("v"));

        assert_eq!(context.request_id.as_deref(), Some("req-1"));
        assert_eq!(context.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(context.principal.as_deref(), Some("alice"));
        assert_eq!(context.limits.timeout_ms, Some(500));
        assert_eq!(context.limits.memory_limit_bytes, Some(4096));
        assert!(context.collect_stats);
        assert_eq!(context.parameters.get("k"), Some(&json!("v")));
    }
}

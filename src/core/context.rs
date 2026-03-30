// Shared context for process-wide services and cross-cutting concerns
use std::sync::Arc;
use std::sync::OnceLock;

#[derive(Clone)]
#[derive(Default)]
pub struct SharedContext {
    pub orchestrator: Option<Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>>,
    pub metrics_updater: Option<Arc<dyn crate::metrics::InternalMetricsUpdater + 'static>>,
    pub tracer: Option<Arc<dyn crate::metrics::InternalMetricsUpdater + 'static>>, // placeholder for tracer handle
    pub tenant: Option<String>,
    /// Optional graph traversal runtime settings (e.g., prefetch knobs)
    pub graph_settings: Option<GraphTraversalSettings>,
    /// Multi-tenant manager for tenant validation
    pub tenant_manager: Option<Arc<crate::storage::tenant::TenantManager>>,
    /// RBAC enforcer for permission validation
    pub rbac_enforcer: Option<Arc<crate::storage::tenant::EnhancedRBACManager>>,
}


#[derive(Clone, Debug)]
pub struct GraphTraversalSettings {
    pub enable_prefetch: bool,
    pub prefetch_budget: usize,
}

impl Default for GraphTraversalSettings {
    fn default() -> Self {
        Self {
            enable_prefetch: true,
            prefetch_budget: 8,
        }
    }
}

static GLOBAL_GRAPH_SETTINGS: OnceLock<GraphTraversalSettings> = OnceLock::new();

/// Register global graph settings (called at startup)
pub fn register_global_graph_settings(settings: GraphTraversalSettings) {
    let _ = GLOBAL_GRAPH_SETTINGS.set(settings);
}

/// Get global graph settings if registered
pub fn global_graph_settings() -> Option<GraphTraversalSettings> {
    GLOBAL_GRAPH_SETTINGS.get().cloned()
}

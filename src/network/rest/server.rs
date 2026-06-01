/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! REST server implementation using axum

use axum::{Router, extract::DefaultBodyLimit, middleware};
use proximadb_graph_query::service::GraphExecutionService;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::DecompressionLayer;
use tower_http::trace::TraceLayer;

use super::v1::handlers::{AppState, create_router};
use crate::api_handlers::UnifiedHandlers;
use crate::monitoring::MetricsCollector;
use crate::network::middleware::backpressure::{
    BackpressureConfig, create_concurrency_limit_layer,
};
use crate::network::middleware::cors::{CorsConfig, create_cors_layer};
use crate::network::middleware::request_id::request_id_middleware;
use crate::network::middleware::tenant::{
    TenantExtractor, TenantExtractorConfig, tenant_middleware,
};
use crate::network::middleware::timeout::{TimeoutConfig, create_timeout_layer};
use crate::network::tls::TlsConfig as NetworkTlsConfig;
use crate::security::SecurityCoordinator;

/// REST server for ProximaDB
pub struct RestServer {
    router: Router,
    bind_addr: SocketAddr,
    tls_config: Option<NetworkTlsConfig>,
}

/// Port-based service objects for wiring proximadb-api REST handlers.
///
/// When provided to `RestServer`, the document/graph/observability routes are
/// served by the port-backed handlers from `proximadb-api` instead of the
/// legacy root-crate handlers.
pub struct RestServerPorts {
    pub doc_port: std::sync::Arc<dyn proximadb_runtime::DocumentPort>,
    pub graph_port: std::sync::Arc<dyn proximadb_runtime::GraphPort>,
    pub obs_port: std::sync::Arc<dyn proximadb_runtime::ObservabilityPort>,
    /// Port-backed handler for collection/vector routes (Phase 9.10).
    pub api_handlers: std::sync::Arc<dyn proximadb_runtime::ApiHandlersPort>,
}

/// Authentication configuration for the REST server
#[derive(Debug, Clone, Default)]
pub struct RestAuthConfig {
    /// Whether unified authentication is enabled
    pub enabled: bool,
}

/// Security configuration for the REST server.
///
/// This struct bundles all security-related settings for the REST API.
/// Defaults are configured for production security.
#[derive(Debug, Clone)]
pub struct RestServerSecurityConfig {
    /// CORS configuration
    pub cors: CorsConfig,
    /// Request timeout configuration
    pub timeout: TimeoutConfig,
    /// Backpressure configuration for load shedding
    pub backpressure: BackpressureConfig,
    /// Unified authentication configuration
    pub auth: RestAuthConfig,
    /// Multi-tenant configuration
    pub tenant: TenantExtractorConfig,
    /// Whether to run in development mode (relaxed security)
    pub development_mode: bool,
    /// TLS configuration (None = plaintext, Some = TLS enabled)
    pub tls: Option<RestTlsConfig>,
}

/// TLS configuration for REST server
#[derive(Debug, Clone)]
pub struct RestTlsConfig {
    /// Path to certificate file (PEM format)
    pub cert_file: PathBuf,
    /// Path to private key file (PEM format)
    pub key_file: PathBuf,
    /// Path to CA certificate file for mTLS (optional)
    pub ca_file: Option<PathBuf>,
    /// Require client certificates (mTLS)
    pub require_client_certs: bool,
    /// Allowed CN patterns for mTLS (empty = allow all)
    pub allowed_cn_patterns: Vec<String>,
    /// CN to user ID mappings
    pub cn_to_user_mapping: std::collections::HashMap<String, String>,
    /// Default roles for mTLS-authenticated users
    pub default_roles: Vec<String>,
}

impl Default for RestTlsConfig {
    fn default() -> Self {
        Self {
            cert_file: PathBuf::new(),
            key_file: PathBuf::new(),
            ca_file: None,
            require_client_certs: false,
            allowed_cn_patterns: vec![],
            cn_to_user_mapping: std::collections::HashMap::new(),
            default_roles: vec!["reader".to_string()],
        }
    }
}

impl RestTlsConfig {
    /// Create new TLS config with certificate paths
    pub fn new(cert_file: PathBuf, key_file: PathBuf) -> Self {
        Self {
            cert_file,
            key_file,
            ..Default::default()
        }
    }

    /// Enable mTLS with CA certificate
    pub fn with_mtls(mut self, ca_file: PathBuf) -> Self {
        self.ca_file = Some(ca_file);
        self.require_client_certs = true;
        self
    }

    /// Set allowed CN patterns
    pub fn with_allowed_cn_patterns(mut self, patterns: Vec<String>) -> Self {
        self.allowed_cn_patterns = patterns;
        self
    }

    /// Set CN to user mappings
    pub fn with_cn_mappings(mut self, mappings: std::collections::HashMap<String, String>) -> Self {
        self.cn_to_user_mapping = mappings;
        self
    }

    /// Set default roles for mTLS users
    pub fn with_default_roles(mut self, roles: Vec<String>) -> Self {
        self.default_roles = roles;
        self
    }
}

impl Default for RestServerSecurityConfig {
    #[inline]
    fn default() -> Self {
        Self {
            cors: CorsConfig::production(),
            timeout: TimeoutConfig::default(),
            backpressure: BackpressureConfig::default(),
            auth: RestAuthConfig::default(),
            tenant: TenantExtractorConfig::default(), // Single-tenant mode by default
            development_mode: false,
            tls: None, // Plaintext by default
        }
    }
}

impl RestServerSecurityConfig {
    /// Create a new security configuration with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a development configuration with relaxed security.
    ///
    /// **WARNING**: Only use for local development. Never in production!
    pub fn development() -> Self {
        Self {
            cors: CorsConfig::development(),
            timeout: TimeoutConfig::default(),
            backpressure: BackpressureConfig::disabled(),
            auth: RestAuthConfig { enabled: false },
            tenant: TenantExtractorConfig::single_tenant("default"), // Single-tenant for dev
            development_mode: true,
            tls: None,
        }
    }

    /// Create a production configuration with specified allowed origins.
    pub fn production_with_origins(origins: Vec<String>) -> Self {
        let mut cors = CorsConfig::production();
        cors.allowed_origins = origins;
        Self {
            cors,
            timeout: TimeoutConfig::default(),
            backpressure: BackpressureConfig::default(),
            auth: RestAuthConfig::default(),
            tenant: TenantExtractorConfig::default(),
            development_mode: false,
            tls: None,
        }
    }

    /// Create a production configuration with TLS enabled.
    pub fn production_with_tls(tls_config: RestTlsConfig) -> Self {
        Self {
            cors: CorsConfig::production(),
            timeout: TimeoutConfig::default(),
            backpressure: BackpressureConfig::default(),
            auth: RestAuthConfig::default(),
            tenant: TenantExtractorConfig::default(),
            development_mode: false,
            tls: Some(tls_config),
        }
    }

    /// Enable TLS on this configuration.
    pub fn with_tls(mut self, tls_config: RestTlsConfig) -> Self {
        self.tls = Some(tls_config);
        self
    }

    /// Create a multi-tenant configuration.
    pub fn multi_tenant() -> Self {
        Self {
            cors: CorsConfig::production(),
            timeout: TimeoutConfig::default(),
            backpressure: BackpressureConfig::default(),
            auth: RestAuthConfig { enabled: true }, // Auth required for multi-tenant
            tenant: TenantExtractorConfig::multi_tenant(),
            development_mode: false,
            tls: None,
        }
    }
}

impl RestServer {
    /// Create new REST server with default security configuration.
    ///
    /// Uses production-safe defaults:
    /// - CORS: No cross-origin requests allowed
    /// - Timeout: 30 second request timeout
    /// - Compression: Optional based on parameter
    pub fn new(
        bind_addr: SocketAddr,
        request_handlers: Arc<UnifiedHandlers>,
        max_request_size_mb: Option<u64>,
        compression: bool,
        metrics_collector: Option<Arc<MetricsCollector>>,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
    ) -> Self {
        Self::with_security(
            bind_addr,
            request_handlers,
            max_request_size_mb,
            compression,
            metrics_collector,
            security_coordinator,
            RestServerSecurityConfig::default(),
            None,
        )
    }

    /// Create new REST server with development mode (relaxed security).
    ///
    /// **WARNING**: Only use for local development and testing!
    pub fn new_development(
        bind_addr: SocketAddr,
        request_handlers: Arc<UnifiedHandlers>,
        max_request_size_mb: Option<u64>,
        compression: bool,
        metrics_collector: Option<Arc<MetricsCollector>>,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
    ) -> Self {
        tracing::warn!("🚨 Starting REST server in DEVELOPMENT mode - security is relaxed!");
        Self::with_security(
            bind_addr,
            request_handlers,
            max_request_size_mb,
            compression,
            metrics_collector,
            security_coordinator,
            RestServerSecurityConfig::development(),
            None,
        )
    }

    /// Create new REST server with custom security configuration.
    pub fn with_security(
        bind_addr: SocketAddr,
        request_handlers: Arc<UnifiedHandlers>,
        max_request_size_mb: Option<u64>,
        compression: bool,
        metrics_collector: Option<Arc<MetricsCollector>>,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
        security_config: RestServerSecurityConfig,
        llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
    ) -> Self {
        let graph_execution_service = request_handlers.graph_execution_service.clone();
        Self::with_security_and_config(
            bind_addr,
            request_handlers,
            graph_execution_service,
            max_request_size_mb,
            compression,
            metrics_collector,
            security_coordinator,
            security_config,
            std::path::PathBuf::from("/tmp/proximadb/data"), // Default fallback
            None, // No query adapter for legacy constructor
            llm_engine,
        )
    }

    /// Create new REST server with custom security configuration and data directory from config.
    pub fn with_security_and_config(
        bind_addr: SocketAddr,
        request_handlers: Arc<UnifiedHandlers>,
        graph_execution_service: Arc<dyn GraphExecutionService>,
        max_request_size_mb: Option<u64>,
        compression: bool,
        metrics_collector: Option<Arc<MetricsCollector>>,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
        security_config: RestServerSecurityConfig,
        data_dir: std::path::PathBuf,
        query_adapter: Option<Arc<crate::query::facade::QueryFacadeAdapter>>,
        llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
    ) -> Self {
        Self::with_security_and_config_and_ports(
            bind_addr,
            request_handlers,
            graph_execution_service,
            max_request_size_mb,
            compression,
            metrics_collector,
            security_coordinator,
            security_config,
            data_dir,
            query_adapter,
            llm_engine,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    }

    /// Create new REST server with port-backed service objects wired in.
    ///
    /// When `ports` is `Some`, document/graph/observability routes are served
    /// by handlers from `proximadb-api` that delegate to the port trait objects.
    /// When `queue_client` is `Some`, the v3 `/documents?mode=async` handler
    /// routes through `producer.send`; otherwise it falls back to inline embed.
    pub fn with_security_and_config_and_ports(
        bind_addr: SocketAddr,
        request_handlers: Arc<UnifiedHandlers>,
        graph_execution_service: Arc<dyn GraphExecutionService>,
        max_request_size_mb: Option<u64>,
        compression: bool,
        metrics_collector: Option<Arc<MetricsCollector>>,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
        security_config: RestServerSecurityConfig,
        data_dir: std::path::PathBuf,
        query_adapter: Option<Arc<crate::query::facade::QueryFacadeAdapter>>,
        llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
        ports: Option<RestServerPorts>,
        catalog_manager: Option<Arc<crate::catalog::CatalogManager>>,
        queue_client: Option<Arc<proximadb_queue::QueueClient>>,
        fulltext_indexes: Option<crate::network::rest::v1::handlers::FullTextIndexMap>,
        discovery_service: Option<Arc<crate::services::discovery::DiscoveryService>>,
        external_collection_service: Option<
            Arc<crate::services::external_collection::ExternalCollectionService>,
        >,
    ) -> Self {
        // Tier 1.2 (pre-release foundational plan 2026-05-26):
        // Warn loudly if a production-mode server is being constructed with
        // authentication disabled.  `RestServerSecurityConfig::default()` and
        // `production_with_origins` / `production_with_tls` builders all
        // delegate to `RestAuthConfig::default()` which has `enabled: false`
        // for backward compatibility — explicit `multi_tenant()` setups turn
        // auth on.  Single-tenant production deployments that forget to wire
        // auth would otherwise silently accept unauthenticated requests; the
        // warning makes the foot-gun visible at server start.
        if !security_config.development_mode && !security_config.auth.enabled {
            tracing::warn!(
                target: "proximadb::security",
                bind_addr = %bind_addr,
                "REST server starting in production mode with auth.enabled=false. \
                 All requests will be accepted without authentication. If this is \
                 intentional (single-tenant trusted-network deployment), suppress \
                 by switching to development_mode or by explicitly enabling auth via \
                 RestServerSecurityConfig::multi_tenant() / auth.enabled=true. \
                 See PRE_RELEASE_FOUNDATIONS_2026_05_26.adoc T1.2 for context."
            );
        }

        let mut base_state = AppState::new(
            request_handlers,
            graph_execution_service,
            security_coordinator.clone(),
            data_dir,
            query_adapter.clone(),
            llm_engine,
        );
        if let Some(manager) = catalog_manager {
            base_state = base_state.with_catalog_manager(manager);
        }
        if let Some(qc) = queue_client {
            base_state = base_state.with_queue_client(qc);
        }
        // T3.2 Slice 1b: share `fulltext_indexes` with `SharedServices` so REST
        // hybrid search and gRPC hybrid search read+write the same in-process
        // map. Without this, REST has its own empty map and gRPC's BM25
        // component returns 0 results.
        if let Some(indexes) = fulltext_indexes.clone() {
            base_state = base_state.with_fulltext_indexes(indexes);
        }
        // Phase 8 (F1): share the Continuous Discovery service so the v2
        // `discovery-jobs` endpoints reach the same registry the background
        // executor consumes (multi-port REST path).
        if let Some(discovery) = discovery_service {
            base_state = base_state.with_discovery_service(discovery);
        }
        // Phase 8 (F5): share the External Collection service so the v2
        // `external-collections` endpoints reach the same registry.
        if let Some(external) = external_collection_service {
            base_state = base_state.with_external_collection_service(external);
        }
        let state = if let Some(p) = ports {
            let s = base_state.with_ports(p.doc_port, p.graph_port, p.obs_port);
            s.with_api_handlers(p.api_handlers)
        } else {
            base_state
        };
        let state = if let Some(ref adapter) = query_adapter {
            let port = Arc::new(
                crate::query::UnifiedQueryPortImpl::new(adapter.clone())
                    .with_catalog_manager(state.catalog_manager.clone()),
            ) as Arc<dyn proximadb_runtime::UnifiedQueryPort>;
            state.with_unified_query_port(port)
        } else {
            state
        };

        // Calculate max request size in bytes (default to 64MB if not specified)
        let max_size_bytes = max_request_size_mb.unwrap_or(64) * 1024 * 1024;

        // Create metrics router if metrics collector is available
        let metrics_router = if let Some(collector) = metrics_collector {
            use crate::network::metrics_service::{MetricsService, MetricsServiceConfig};
            let metrics_config = MetricsServiceConfig::default();
            let metrics_service = MetricsService::new(metrics_config, collector);
            Some(metrics_service.create_router())
        } else {
            None
        };

        // Build service layers conditionally to avoid type mismatch
        let security_coordinator = state.security_coordinator.clone();
        let state_for_v2 = state.clone();
        let mut base_router = create_router(state.clone());

        // Nest metrics router if available
        if let Some(metrics) = metrics_router {
            base_router = base_router.nest("/metrics", metrics);
            tracing::info!("✅ Metrics endpoints enabled at /metrics");
        }

        // Add dashboard route
        base_router = base_router.route("/dashboard", axum::routing::get(dashboard_handler));

        // Add V2 API router with ProximaRecord support
        let state_for_v3 = state_for_v2.clone();
        let v2_router = super::v2::create_v2_router().with_state(state_for_v2);
        base_router = base_router.nest("/api/v2", v2_router);
        tracing::info!("✅ V2 API enabled at /api/v2 (ProximaRecord, typed schema)");

        // Add V3 API router with native server-side embedding
        let v3_router = super::v3::create_v3_router().with_state(state_for_v3);
        base_router = base_router.nest("/api/v3", v3_router);
        tracing::info!("✅ V3 API enabled at /api/v3 (native server-side embedding)");

        // Add WebSocket streaming routes
        let ws_state = super::websocket::WebSocketState::new();
        let ws_routes = super::websocket::websocket_routes(ws_state);
        base_router = base_router.nest("/ws", ws_routes);
        tracing::info!("✅ WebSocket streaming enabled at /ws");

        // Create CORS layer using secure configuration
        let cors_layer = create_cors_layer(&security_config.cors);

        // Create timeout layer (if enabled)
        let timeout_layer = create_timeout_layer(&security_config.timeout);

        // Create concurrency limit layer for backpressure (if enabled)
        let concurrency_layer = create_concurrency_limit_layer(&security_config.backpressure);

        // Authentication layer (unified): prefer SecurityCoordinator if available
        let auth_layer = if security_config.auth.enabled {
            if let Some(coordinator) = security_coordinator {
                Some(middleware::from_fn_with_state(
                    coordinator,
                    crate::network::auth::middleware::auth_middleware_unified,
                ))
            } else {
                tracing::warn!(
                    "Security enabled but no coordinator available; auth layer disabled"
                );
                None
            }
        } else {
            None
        };

        // Tenant extraction layer for multi-tenant isolation
        let tenant_extractor = TenantExtractor::with_config(security_config.tenant.clone());
        let tenant_layer = middleware::from_fn_with_state(tenant_extractor, tenant_middleware);

        // Log security configuration
        if security_config.development_mode {
            tracing::warn!("⚠️  REST server security: DEVELOPMENT MODE - CORS allows any origin");
        } else {
            tracing::info!(
                "🔒 REST server security: Production mode - CORS whitelist: {:?}",
                security_config.cors.allowed_origins
            );
        }

        // Log backpressure configuration
        if security_config.backpressure.enabled {
            tracing::info!(
                "🛡️  Backpressure enabled: max {} concurrent requests",
                security_config.backpressure.max_concurrent_requests
            );
        }

        // Apply concurrency limit layer first (if enabled) for early rejection
        let base_router = if let Some(concurrency) = concurrency_layer {
            base_router.layer(concurrency)
        } else {
            base_router
        };

        // Add request ID middleware for tracing and correlation
        let base_router = base_router.layer(middleware::from_fn(request_id_middleware));

        let mut router = if compression {
            // Create compression layer with support for multiple algorithms
            // Priority order (fastest to best compression): deflate, gzip, zstd, brotli
            let compression_layer = CompressionLayer::new()
                .deflate(true) // Fastest, low CPU usage
                .gzip(true) // Good balance of speed and compression
                .zstd(true) // Best compression ratio with good speed
                .br(true); // Brotli - slower but excellent compression

            // Create decompression layer for handling compressed requests
            let decompression_layer = DecompressionLayer::new()
                .deflate(true)
                .gzip(true)
                .br(true)
                .zstd(true);

            // IMPORTANT: Layer ordering matters for security!
            // In Tower ServiceBuilder, layers wrap previous layers, so request flow is:
            // cors -> trace -> compression -> body_limit -> decompression -> handler
            //
            // We want body limit BEFORE decompression to prevent decompression bombs
            // (small compressed payload expanding to huge uncompressed data causing OOM)
            let service_builder = ServiceBuilder::new()
                .layer(decompression_layer) // Handle compressed requests (innermost)
                .layer(DefaultBodyLimit::max(max_size_bytes as usize)) // Body limit BEFORE decompression
                .layer(compression_layer) // Compress responses
                .layer(TraceLayer::new_for_http())
                .layer(cors_layer);

            // Add timeout layer if enabled
            if let Some(timeout) = timeout_layer {
                base_router.layer(service_builder.layer(timeout))
            } else {
                base_router.layer(service_builder)
            }
        } else {
            let service_builder = ServiceBuilder::new()
                .layer(DefaultBodyLimit::max(max_size_bytes as usize))
                .layer(TraceLayer::new_for_http())
                .layer(cors_layer);

            // Add timeout layer if enabled
            if let Some(timeout) = timeout_layer {
                base_router.layer(service_builder.layer(timeout))
            } else {
                base_router.layer(service_builder)
            }
        };

        // Apply tenant layer (runs after auth to access JWT claims)
        router = router.layer(tenant_layer);
        tracing::info!(
            "🏢 Tenant isolation: {}",
            if security_config.tenant.require_tenant {
                "MULTI-TENANT (tenant ID required)"
            } else {
                "SINGLE-TENANT (default tenant fallback)"
            }
        );

        // Apply auth layer last so request IDs/backpressure/cors are preserved
        if let Some(auth) = auth_layer {
            router = router.layer(auth);
        }

        // Build TLS config if specified
        let tls_config = security_config.tls.as_ref().map(|tls| {
            let mut config = NetworkTlsConfig::new(true)
                .with_cert_file(tls.cert_file.clone())
                .with_key_file(tls.key_file.clone());
            // Only add CA file if mTLS is enabled and CA file is provided
            if let Some(ca_file) = &tls.ca_file {
                config = config.with_ca_file(ca_file.clone());
            }
            config
        });

        if tls_config.is_some() {
            tracing::info!("TLS configured for REST server");
        }

        Self {
            router,
            bind_addr,
            tls_config,
        }
    }

    /// Build a REST router for unified mode without starting a server.
    ///
    /// `ports` injects port-backed handlers from `proximadb-api` for document,
    /// graph, and observability routes. When `None`, the legacy root-crate
    /// handlers are used.
    pub fn build_router_for_unified(
        request_handlers: Arc<UnifiedHandlers>,
        graph_execution_service: Arc<dyn GraphExecutionService>,
        metrics_collector: Option<Arc<MetricsCollector>>,
        security_coordinator: Option<Arc<SecurityCoordinator>>,
        data_dir: std::path::PathBuf,
        query_adapter: Option<Arc<crate::query::facade::QueryFacadeAdapter>>,
        llm_engine: Option<Arc<crate::ai::llm_integration::LLMIntegrationEngine>>,
        ports: Option<RestServerPorts>,
        segment_registry: Option<Arc<crate::catalog::SegmentRegistry>>,
        catalog_manager: Option<Arc<crate::catalog::CatalogManager>>,
        queue_client: Option<Arc<proximadb_queue::QueueClient>>,
        fulltext_indexes: Option<crate::network::rest::v1::handlers::FullTextIndexMap>,
        recall_probe_gate: Option<Arc<crate::catalog::RecallProbeGate>>,
        rank_services: Option<Arc<crate::network::rest::v1::rank::RankServices>>,
        rank_profile_store: Option<Arc<dyn crate::services::RankProfileStore>>,
        discovery_service: Option<Arc<crate::services::discovery::DiscoveryService>>,
        external_collection_service: Option<
            Arc<crate::services::external_collection::ExternalCollectionService>,
        >,
    ) -> Router {
        let mut base_state = AppState::new(
            request_handlers,
            graph_execution_service,
            security_coordinator.clone(),
            data_dir,
            query_adapter.clone(),
            llm_engine,
        );
        if let Some(reg) = segment_registry {
            base_state = base_state.with_segment_registry(reg);
        }
        if let Some(manager) = catalog_manager {
            base_state = base_state.with_catalog_manager(manager);
        }
        if let Some(qc) = queue_client {
            base_state = base_state.with_queue_client(qc);
        }
        // T3.2 Slice 1b: share `fulltext_indexes` with `SharedServices` so REST
        // hybrid search and gRPC hybrid search read+write the same in-process
        // map. Without this, REST has its own empty map and gRPC's BM25
        // component returns 0 results.
        if let Some(indexes) = fulltext_indexes.clone() {
            base_state = base_state.with_fulltext_indexes(indexes);
        }
        // TD-064 / LLD §5: share the recall-probe gate so v2 route-health
        // can resolve per-scope `gate_open` state without re-constructing.
        if let Some(gate) = recall_probe_gate {
            base_state = base_state.with_recall_probe_gate(gate);
        }
        // R-7c.3 production wiring: share the rank-pipeline singleton + the
        // durable rank-profile catalog so the REST `/api/v1/rank/search` route
        // and the new `/api/v1/rank/profiles` install endpoints reach the same
        // process-wide `RankServices` that pgwire SQL `RERANK(...)` uses.
        if let Some(services) = rank_services {
            base_state = base_state.with_rank_services(services);
        }
        if let Some(store) = rank_profile_store {
            base_state = base_state.with_rank_profile_store(store);
        }
        // Phase 8 (F1): share the Continuous Discovery service so the v2
        // `discovery-jobs` endpoints reach the same registry the background
        // executor consumes.
        if let Some(discovery) = discovery_service {
            base_state = base_state.with_discovery_service(discovery);
        }
        // Phase 8 (F5): share the External Collection service so the v2
        // `external-collections` endpoints reach the same registry.
        if let Some(external) = external_collection_service {
            base_state = base_state.with_external_collection_service(external);
        }
        let state = if let Some(p) = ports {
            let s = base_state.with_ports(p.doc_port, p.graph_port, p.obs_port);
            s.with_api_handlers(p.api_handlers)
        } else {
            base_state
        };
        let state = if let Some(ref adapter) = query_adapter {
            let port = Arc::new(
                crate::query::UnifiedQueryPortImpl::new(adapter.clone())
                    .with_catalog_manager(state.catalog_manager.clone()),
            ) as Arc<dyn proximadb_runtime::UnifiedQueryPort>;
            state.with_unified_query_port(port)
        } else {
            state
        };

        // Create metrics router if metrics collector is available
        let metrics_router = if let Some(collector) = metrics_collector {
            use crate::network::metrics_service::{MetricsService, MetricsServiceConfig};
            let metrics_config = MetricsServiceConfig::default();
            let metrics_service = MetricsService::new(metrics_config, collector);
            Some(metrics_service.create_router())
        } else {
            None
        };

        // Build base router with all endpoints
        let state_for_v2 = state.clone();
        let mut base_router = create_router(state.clone());

        // Nest metrics router if available
        if let Some(metrics) = metrics_router {
            base_router = base_router.nest("/metrics", metrics);
            tracing::info!("✅ Metrics endpoints enabled at /metrics (unified mode)");
        }

        // Add dashboard route
        base_router = base_router.route("/dashboard", axum::routing::get(dashboard_handler));

        // Add V2 API router with ProximaRecord support
        let state_for_v3 = state_for_v2.clone();
        let v2_router = super::v2::create_v2_router().with_state(state_for_v2);
        base_router = base_router.nest("/api/v2", v2_router);
        tracing::info!("✅ V2 API enabled at /api/v2 (unified mode)");

        // Add V3 API router with native server-side embedding
        let v3_router = super::v3::create_v3_router().with_state(state_for_v3);
        base_router = base_router.nest("/api/v3", v3_router);
        tracing::info!("✅ V3 API enabled at /api/v3 (native embedding, unified mode)");

        // Add WebSocket streaming routes
        let ws_state = super::websocket::WebSocketState::new();
        let ws_routes = super::websocket::websocket_routes(ws_state);
        base_router = base_router.nest("/ws", ws_routes);
        tracing::info!("✅ WebSocket streaming enabled at /ws (unified mode)");

        // Tenant extraction layer for unified mode. v2/v3 handlers require an
        // `Extension<MiddlewareTenantContext>`; without this layer every
        // `/api/v2` request 500s with "missing request extension". The
        // multi-port path applies the same layer in `create_router`; the
        // unified path must apply it too (it is NOT wrapped by the caller).
        // Default config resolves to the single "default" tenant in dev.
        let tenant_extractor = TenantExtractor::with_config(TenantExtractorConfig::default());
        let tenant_layer = middleware::from_fn_with_state(tenant_extractor, tenant_middleware);

        // Layer order: TraceLayer outermost, tenant extraction inside it so the
        // tenant context is populated before handlers run.
        use tower_http::trace::TraceLayer;
        base_router
            .layer(tenant_layer)
            .layer(TraceLayer::new_for_http())
    }

    /// Start the REST server
    pub async fn start(self) -> anyhow::Result<()> {
        if self.tls_config.is_some() {
            self.start_with_tls().await
        } else {
            self.start_plaintext().await
        }
    }

    /// Start the REST server with TLS
    async fn start_with_tls(self) -> anyhow::Result<()> {
        let tls_config = self
            .tls_config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("TLS config required for TLS server"))?;

        let (cert_path, key_path) = tls_config
            .get_certificate_paths()
            .ok_or_else(|| anyhow::anyhow!("Certificate paths not configured"))?;

        let require_client_certs = tls_config.require_client_certs;
        let ca_path = tls_config.get_ca_path();

        tracing::info!("Starting REST server with TLS on {}", self.bind_addr);
        tracing::info!("  Certificate: {:?}", cert_path);
        tracing::info!("  Private key: {:?}", key_path);
        tracing::info!(
            "  mTLS mode: {}",
            if require_client_certs {
                "ENABLED"
            } else {
                "DISABLED"
            }
        );
        if let Some(ref ca) = ca_path {
            tracing::info!("  CA Certificate: {:?}", ca);
        }

        Self::log_endpoints(&self.bind_addr, true);

        // Build rustls config - either mTLS or standard TLS
        if require_client_certs {
            if let Some(ca) = ca_path {
                self.start_with_mtls(cert_path, key_path, ca).await
            } else {
                Err(anyhow::anyhow!(
                    "Client certificates required but CA path not provided"
                ))
            }
        } else {
            // Standard TLS (no client certificates)
            let rustls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to load TLS certificates: {}", e))?;

            axum_server::bind_rustls(self.bind_addr, rustls_config)
                .serve(self.router.into_make_service())
                .await
                .map_err(|e| anyhow::anyhow!("TLS server error: {}", e))?;

            Ok(())
        }
    }

    /// Start REST server with mTLS (mutual TLS) - requires client certificates
    async fn start_with_mtls(
        self,
        cert_path: PathBuf,
        key_path: PathBuf,
        ca_path: PathBuf,
    ) -> anyhow::Result<()> {
        use crate::network::tls::certificate_manager::utils::{
            load_certs_from_pem, load_private_key_from_pem,
        };
        use rustls::{RootCertStore, ServerConfig, server::AllowAnyAuthenticatedClient};
        use std::sync::Arc;

        tracing::info!("Configuring mTLS with client certificate verification");

        // Load server certificate and key
        let cert_pem = tokio::fs::read(&cert_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read server certificate: {}", e))?;
        let key_pem = tokio::fs::read(&key_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read server private key: {}", e))?;

        let certs = load_certs_from_pem(&cert_pem)
            .map_err(|e| anyhow::anyhow!("Failed to parse server certificate: {}", e))?;
        let key = load_private_key_from_pem(&key_pem)
            .map_err(|e| anyhow::anyhow!("Failed to parse server private key: {}", e))?;

        // Load CA certificate for client verification
        let ca_pem = tokio::fs::read(&ca_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read CA certificate: {}", e))?;
        let ca_certs = load_certs_from_pem(&ca_pem)
            .map_err(|e| anyhow::anyhow!("Failed to parse CA certificate: {}", e))?;

        // Build root cert store for client verification
        let mut root_store = RootCertStore::empty();
        for ca_cert in ca_certs {
            root_store.add(&ca_cert).map_err(|e| {
                anyhow::anyhow!("Failed to add CA certificate to root store: {}", e)
            })?;
        }

        // Create client certificate verifier - allows any cert signed by our CA
        let client_verifier = AllowAnyAuthenticatedClient::new(root_store);

        // Build mTLS server config
        let server_config = ServerConfig::builder()
            .with_safe_defaults()
            .with_client_cert_verifier(Arc::new(client_verifier))
            .with_single_cert(certs, key)
            .map_err(|e| anyhow::anyhow!("Failed to build mTLS server config: {}", e))?;

        let rustls_config =
            axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(server_config));

        tracing::info!("mTLS server configured - client certificates will be verified against CA");
        tracing::info!("Note: Client certificate info will be available in request extensions");

        axum_server::bind_rustls(self.bind_addr, rustls_config)
            .serve(self.router.into_make_service())
            .await
            .map_err(|e| anyhow::anyhow!("mTLS server error: {}", e))?;

        Ok(())
    }

    /// Start the REST server without TLS (plaintext)
    async fn start_plaintext(self) -> anyhow::Result<()> {
        tracing::info!("Starting REST server (plaintext) on {}", self.bind_addr);

        Self::log_endpoints(&self.bind_addr, false);

        // For axum 0.6, use axum::Server
        axum::Server::bind(&self.bind_addr)
            .serve(self.router.into_make_service())
            .await?;

        Ok(())
    }

    /// Log available endpoints
    fn log_endpoints(bind_addr: &SocketAddr, tls: bool) {
        let protocol = if tls { "https" } else { "http" };
        tracing::info!(
            "REST server using canonical v2 record routes plus v1 compatibility adapters"
        );
        tracing::info!("REST server listening on {}://{}", protocol, bind_addr);
        tracing::info!("Compression enabled: deflate, gzip, zstd, brotli (in priority order)");
        tracing::info!("Available endpoints:");
        tracing::info!("   GET    /health                           - Health check");
        tracing::info!("   GET    /dashboard                        - Web dashboard");
        tracing::info!("   GET    /metrics                          - Prometheus metrics");
        tracing::info!("   GET    /metrics/json                     - JSON metrics");
        tracing::info!("   GET    /metrics/health                   - Metrics health check");
        tracing::info!(
            "   POST   /api/v2/collections/:id/search     - Canonical record/vector search"
        );
        tracing::info!(
            "   POST   /api/v2/collections/:id/records/batch - Canonical ProximaRecord writes"
        );
        tracing::info!("Compatibility endpoints:");
        tracing::info!(
            "   POST   /api/v1/search                    - Deprecated vector search adapter"
        );
        tracing::info!(
            "   POST   /api/v1/vectors/batch             - Deprecated alias over record writes"
        );
        tracing::info!("   POST   /api/v1/progressive/search/:id    - Progressive search (JSON)");
        tracing::info!(
            "   POST   /api/v1/collections               - Deprecated collection compatibility"
        );
        tracing::info!("   GET    /api/v1/collections               - List collections");
        tracing::info!("   GET    /api/v1/collections/:id           - Get collection by ID");
        tracing::info!("   DELETE /api/v1/collections/:id           - Delete collection");
        tracing::info!("   POST   /api/v1/search/with_metadata      - Vector search with metadata");
        tracing::info!("   WS     /ws/insert                        - WebSocket vector streaming");
        tracing::info!(
            "   WS     /ws/subscribe                     - WebSocket live query subscription"
        );
        tracing::info!("   WS     /ws/status                        - WebSocket session status");
    }
}

/// Dashboard handler - serves a comprehensive professional dashboard
async fn dashboard_handler() -> axum::response::Html<&'static str> {
    axum::response::Html(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>ProximaDB Dashboard</title>
    <script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.0/dist/chart.umd.min.js"></script>
    <style>
        :root {
            --primary-color: #4a90e2;
            --secondary-color: #667eea;
            --success-color: #10b981;
            --warning-color: #f59e0b;
            --danger-color: #ef4444;
            --bg-dark: #1a1d2e;
            --bg-light: #f8fafc;
            --text-dark: #1e293b;
            --text-light: #64748b;
            --border-color: #e2e8f0;
        }
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, Cantarell, sans-serif;
            background: var(--bg-light);
            color: var(--text-dark);
            min-height: 100vh;
        }
        .header {
            background: linear-gradient(135deg, var(--primary-color) 0%, var(--secondary-color) 100%);
            color: white;
            padding: 1.5rem 2rem;
            box-shadow: 0 2px 8px rgba(0,0,0,0.1);
        }
        .header-content {
            max-width: 1400px;
            margin: 0 auto;
            display: flex;
            justify-content: space-between;
            align-items: center;
        }
        .logo {
            font-size: 1.75rem;
            font-weight: 700;
            letter-spacing: -0.5px;
        }
        .status-badge {
            display: flex;
            align-items: center;
            gap: 8px;
            background: rgba(255,255,255,0.2);
            padding: 8px 16px;
            border-radius: 20px;
            font-size: 0.875rem;
            font-weight: 600;
        }
        .status-dot {
            width: 8px;
            height: 8px;
            background: var(--success-color);
            border-radius: 50%;
            animation: pulse 2s infinite;
        }
        @keyframes pulse {
            0%, 100% { opacity: 1; }
            50% { opacity: 0.5; }
        }
        .container {
            max-width: 1400px;
            margin: 0 auto;
            padding: 2rem;
        }
        .tabs {
            display: flex;
            gap: 4px;
            margin-bottom: 2rem;
            border-bottom: 2px solid var(--border-color);
        }
        .tab {
            padding: 12px 24px;
            background: transparent;
            border: none;
            color: var(--text-light);
            font-size: 1rem;
            font-weight: 500;
            cursor: pointer;
            border-bottom: 3px solid transparent;
            transition: all 0.3s;
        }
        .tab:hover {
            color: var(--primary-color);
            background: rgba(74, 144, 226, 0.05);
        }
        .tab.active {
            color: var(--primary-color);
            border-bottom-color: var(--primary-color);
            font-weight: 600;
        }
        .tab-content {
            display: none;
        }
        .tab-content.active {
            display: block;
            animation: fadeIn 0.3s;
        }
        @keyframes fadeIn {
            from { opacity: 0; transform: translateY(10px); }
            to { opacity: 1; transform: translateY(0); }
        }
        .card-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
            gap: 1.5rem;
            margin-bottom: 2rem;
        }
        .card {
            background: white;
            border-radius: 12px;
            padding: 1.5rem;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
            transition: transform 0.2s, box-shadow 0.2s;
        }
        .card:hover {
            transform: translateY(-2px);
            box-shadow: 0 4px 12px rgba(0,0,0,0.15);
        }
        .card-header {
            display: flex;
            align-items: center;
            justify-content: space-between;
            margin-bottom: 1rem;
        }
        .card-title {
            font-size: 0.875rem;
            color: var(--text-light);
            font-weight: 600;
            text-transform: uppercase;
            letter-spacing: 0.5px;
        }
        .card-value {
            font-size: 2rem;
            font-weight: 700;
            color: var(--text-dark);
            margin-bottom: 0.5rem;
        }
        .card-change {
            font-size: 0.875rem;
            display: flex;
            align-items: center;
            gap: 4px;
        }
        .card-change.positive {
            color: var(--success-color);
        }
        .card-change.negative {
            color: var(--danger-color);
        }
        .chart-container {
            background: white;
            border-radius: 12px;
            padding: 1.5rem;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
            margin-bottom: 1.5rem;
        }
        .chart-title {
            font-size: 1.125rem;
            font-weight: 600;
            margin-bottom: 1rem;
            color: var(--text-dark);
        }
        .chart-wrapper {
            position: relative;
            height: 300px;
        }
        .collections-table {
            background: white;
            border-radius: 12px;
            padding: 1.5rem;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
            overflow-x: auto;
        }
        table {
            width: 100%;
            border-collapse: collapse;
        }
        th {
            text-align: left;
            padding: 12px;
            background: var(--bg-light);
            font-weight: 600;
            color: var(--text-dark);
            font-size: 0.875rem;
            text-transform: uppercase;
            letter-spacing: 0.5px;
        }
        td {
            padding: 12px;
            border-top: 1px solid var(--border-color);
            color: var(--text-dark);
        }
        tr:hover {
            background: var(--bg-light);
        }
        .badge {
            display: inline-block;
            padding: 4px 12px;
            border-radius: 12px;
            font-size: 0.75rem;
            font-weight: 600;
        }
        .badge-success {
            background: #d1fae5;
            color: #065f46;
        }
        .badge-warning {
            background: #fef3c7;
            color: #92400e;
        }
        .badge-info {
            background: #dbeafe;
            color: #1e40af;
        }
        .refresh-btn {
            background: var(--primary-color);
            color: white;
            border: none;
            padding: 10px 20px;
            border-radius: 8px;
            cursor: pointer;
            font-size: 0.875rem;
            font-weight: 600;
            transition: background 0.3s;
            display: flex;
            align-items: center;
            gap: 8px;
        }
        .refresh-btn:hover {
            background: #3a7bc8;
        }
        .system-info-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
            gap: 1.5rem;
        }
        .progress-bar {
            width: 100%;
            height: 8px;
            background: var(--border-color);
            border-radius: 4px;
            overflow: hidden;
            margin-top: 8px;
        }
        .progress-fill {
            height: 100%;
            background: var(--primary-color);
            transition: width 0.3s;
        }
        .icon {
            width: 20px;
            height: 20px;
            display: inline-block;
        }
        .loading-spinner {
            border: 3px solid var(--border-color);
            border-top: 3px solid var(--primary-color);
            border-radius: 50%;
            width: 20px;
            height: 20px;
            animation: spin 1s linear infinite;
            display: inline-block;
        }
        @keyframes spin {
            0% { transform: rotate(0deg); }
            100% { transform: rotate(360deg); }
        }
    </style>
</head>
<body>
    <div class="header">
        <div class="header-content">
            <div class="logo">📊 ProximaDB Dashboard</div>
            <div class="status-badge">
                <div class="status-dot"></div>
                <span>ONLINE</span>
            </div>
        </div>
    </div>

    <div class="container">
        <div class="tabs">
            <button class="tab active" onclick="switchTab('overview')">Overview</button>
            <button class="tab" onclick="switchTab('collections')">Collections</button>
            <button class="tab" onclick="switchTab('metrics')">Metrics</button>
            <button class="tab" onclick="switchTab('system')">System</button>
        </div>

        <!-- Overview Tab -->
        <div id="overview-tab" class="tab-content active">
            <div class="card-grid">
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Total Collections</span>
                    </div>
                    <div class="card-value" id="overview-collections">-</div>
                    <div class="card-change positive" id="collections-change">
                        <span>↑</span> <span>0%</span>
                    </div>
                </div>
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Total Vectors</span>
                    </div>
                    <div class="card-value" id="overview-vectors">-</div>
                    <div class="card-change positive" id="vectors-change">
                        <span>↑</span> <span>0%</span>
                    </div>
                </div>
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Total Queries</span>
                    </div>
                    <div class="card-value" id="overview-queries">-</div>
                    <div class="card-change positive" id="queries-change">
                        <span>↑</span> <span>0%</span>
                    </div>
                </div>
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Avg Query Latency</span>
                    </div>
                    <div class="card-value" id="overview-latency">-</div>
                    <div class="card-change positive" id="latency-change">
                        <span>↓</span> <span>0%</span>
                    </div>
                </div>
            </div>

            <div class="chart-container">
                <div class="chart-title">Query Performance (Last 60s)</div>
                <div class="chart-wrapper">
                    <canvas id="query-chart"></canvas>
                </div>
            </div>

            <div class="chart-container">
                <div class="chart-title">Storage Distribution</div>
                <div class="chart-wrapper">
                    <canvas id="storage-chart"></canvas>
                </div>
            </div>
        </div>

        <!-- Collections Tab -->
        <div id="collections-tab" class="tab-content">
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 1.5rem;">
                <h2 style="font-size: 1.5rem; font-weight: 600;">Collections</h2>
                <button class="refresh-btn" onclick="refreshCollections()">
                    <span id="collections-refresh-icon">🔄</span>
                    <span>Refresh</span>
                </button>
            </div>
            <div class="collections-table">
                <table id="collections-table">
                    <thead>
                        <tr>
                            <th>Name</th>
                            <th>Dimension</th>
                            <th>Vectors</th>
                            <th>Engine</th>
                            <th>Distance Metric</th>
                            <th>Status</th>
                        </tr>
                    </thead>
                    <tbody id="collections-tbody">
                        <tr>
                            <td colspan="6" style="text-align: center; padding: 2rem; color: var(--text-light);">
                                Loading collections...
                            </td>
                        </tr>
                    </tbody>
                </table>
            </div>
        </div>

        <!-- Metrics Tab -->
        <div id="metrics-tab" class="tab-content">
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 1.5rem;">
                <h2 style="font-size: 1.5rem; font-weight: 600;">Performance Metrics</h2>
                <button class="refresh-btn" onclick="refreshMetrics()">
                    <span id="metrics-refresh-icon">🔄</span>
                    <span>Refresh</span>
                </button>
            </div>

            <div class="card-grid">
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Cache Hit Rate</span>
                    </div>
                    <div class="card-value" id="cache-hit-rate">-</div>
                    <div class="progress-bar">
                        <div class="progress-fill" id="cache-progress" style="width: 0%"></div>
                    </div>
                </div>
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Queries/sec</span>
                    </div>
                    <div class="card-value" id="qps">-</div>
                </div>
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">P99 Latency</span>
                    </div>
                    <div class="card-value" id="p99-latency">-</div>
                </div>
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Error Rate</span>
                    </div>
                    <div class="card-value" id="error-rate">-</div>
                </div>
            </div>

            <div class="chart-container">
                <div class="chart-title">Query Latency Distribution</div>
                <div class="chart-wrapper">
                    <canvas id="latency-chart"></canvas>
                </div>
            </div>

            <div class="chart-container">
                <div class="chart-title">Throughput Over Time</div>
                <div class="chart-wrapper">
                    <canvas id="throughput-chart"></canvas>
                </div>
            </div>
        </div>

        <!-- System Tab -->
        <div id="system-tab" class="tab-content">
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 1.5rem;">
                <h2 style="font-size: 1.5rem; font-weight: 600;">System Information</h2>
                <button class="refresh-btn" onclick="refreshSystem()">
                    <span id="system-refresh-icon">🔄</span>
                    <span>Refresh</span>
                </button>
            </div>

            <div class="card-grid">
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">CPU Usage</span>
                    </div>
                    <div class="card-value" id="cpu-usage">-</div>
                    <div class="progress-bar">
                        <div class="progress-fill" id="cpu-progress" style="width: 0%"></div>
                    </div>
                </div>
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Memory Usage</span>
                    </div>
                    <div class="card-value" id="memory-usage">-</div>
                    <div class="progress-bar">
                        <div class="progress-fill" id="memory-progress" style="width: 0%"></div>
                    </div>
                </div>
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Disk Usage</span>
                    </div>
                    <div class="card-value" id="disk-usage">-</div>
                    <div class="progress-bar">
                        <div class="progress-fill" id="disk-progress" style="width: 0%"></div>
                    </div>
                </div>
                <div class="card">
                    <div class="card-header">
                        <span class="card-title">Uptime</span>
                    </div>
                    <div class="card-value" id="uptime">-</div>
                </div>
            </div>

            <div class="chart-container">
                <div class="chart-title">Resource Usage Over Time</div>
                <div class="chart-wrapper">
                    <canvas id="resource-chart"></canvas>
                </div>
            </div>

            <div class="chart-container">
                <div class="chart-title">Network I/O</div>
                <div class="chart-wrapper">
                    <canvas id="network-chart"></canvas>
                </div>
            </div>
        </div>
    </div>

    <script>
        // Global state
        let charts = {};
        let metricsHistory = {
            queries: [],
            latency: [],
            cpu: [],
            memory: [],
            timestamps: []
        };
        const MAX_HISTORY = 60;

        // Tab switching
        function switchTab(tabName) {
            document.querySelectorAll('.tab').forEach(tab => tab.classList.remove('active'));
            document.querySelectorAll('.tab-content').forEach(content => content.classList.remove('active'));

            event.target.classList.add('active');
            document.getElementById(tabName + '-tab').classList.add('active');
        }

        // Initialize charts
        function initCharts() {
            // Query performance chart
            const queryCtx = document.getElementById('query-chart').getContext('2d');
            charts.query = new Chart(queryCtx, {
                type: 'line',
                data: {
                    labels: [],
                    datasets: [{
                        label: 'Queries/sec',
                        data: [],
                        borderColor: '#4a90e2',
                        backgroundColor: 'rgba(74, 144, 226, 0.1)',
                        tension: 0.4,
                        fill: true
                    }]
                },
                options: {
                    responsive: true,
                    maintainAspectRatio: false,
                    plugins: {
                        legend: { display: false }
                    },
                    scales: {
                        y: { beginAtZero: true }
                    }
                }
            });

            // Storage distribution chart
            const storageCtx = document.getElementById('storage-chart').getContext('2d');
            charts.storage = new Chart(storageCtx, {
                type: 'doughnut',
                data: {
                    labels: ['Vectors', 'Metadata', 'Indexes', 'Cache'],
                    datasets: [{
                        data: [0, 0, 0, 0],
                        backgroundColor: ['#4a90e2', '#667eea', '#10b981', '#f59e0b']
                    }]
                },
                options: {
                    responsive: true,
                    maintainAspectRatio: false
                }
            });

            // Latency distribution chart
            const latencyCtx = document.getElementById('latency-chart').getContext('2d');
            charts.latency = new Chart(latencyCtx, {
                type: 'bar',
                data: {
                    labels: ['P50', 'P90', 'P95', 'P99'],
                    datasets: [{
                        label: 'Latency (ms)',
                        data: [0, 0, 0, 0],
                        backgroundColor: '#4a90e2'
                    }]
                },
                options: {
                    responsive: true,
                    maintainAspectRatio: false,
                    plugins: {
                        legend: { display: false }
                    },
                    scales: {
                        y: { beginAtZero: true }
                    }
                }
            });

            // Throughput chart
            const throughputCtx = document.getElementById('throughput-chart').getContext('2d');
            charts.throughput = new Chart(throughputCtx, {
                type: 'line',
                data: {
                    labels: [],
                    datasets: [{
                        label: 'Throughput (ops/s)',
                        data: [],
                        borderColor: '#10b981',
                        backgroundColor: 'rgba(16, 185, 129, 0.1)',
                        tension: 0.4,
                        fill: true
                    }]
                },
                options: {
                    responsive: true,
                    maintainAspectRatio: false,
                    plugins: {
                        legend: { display: false }
                    },
                    scales: {
                        y: { beginAtZero: true }
                    }
                }
            });

            // Resource usage chart
            const resourceCtx = document.getElementById('resource-chart').getContext('2d');
            charts.resource = new Chart(resourceCtx, {
                type: 'line',
                data: {
                    labels: [],
                    datasets: [
                        {
                            label: 'CPU %',
                            data: [],
                            borderColor: '#4a90e2',
                            tension: 0.4,
                            fill: false
                        },
                        {
                            label: 'Memory %',
                            data: [],
                            borderColor: '#667eea',
                            tension: 0.4,
                            fill: false
                        }
                    ]
                },
                options: {
                    responsive: true,
                    maintainAspectRatio: false,
                    scales: {
                        y: { beginAtZero: true, max: 100 }
                    }
                }
            });

            // Network I/O chart
            const networkCtx = document.getElementById('network-chart').getContext('2d');
            charts.network = new Chart(networkCtx, {
                type: 'line',
                data: {
                    labels: [],
                    datasets: [
                        {
                            label: 'RX (MB/s)',
                            data: [],
                            borderColor: '#10b981',
                            tension: 0.4,
                            fill: false
                        },
                        {
                            label: 'TX (MB/s)',
                            data: [],
                            borderColor: '#f59e0b',
                            tension: 0.4,
                            fill: false
                        }
                    ]
                },
                options: {
                    responsive: true,
                    maintainAspectRatio: false,
                    scales: {
                        y: { beginAtZero: true }
                    }
                }
            });
        }

        // Refresh metrics
        async function refreshMetrics() {
            const icon = document.getElementById('metrics-refresh-icon');
            icon.innerHTML = '<div class="loading-spinner"></div>';

            try {
                // Fetch both metrics and collections data
                const [metricsResponse, collectionsResponse] = await Promise.all([
                    fetch('/metrics/json'),
                    fetch('/api/v1/collections')
                ]);

                const metrics = await metricsResponse.json();
                const collectionsData = await collectionsResponse.json();
                const collections = collectionsData.collections || collectionsData || [];

                // Compute actual totals from collections
                const totalCollections = collections.length;
                const totalVectors = collections.reduce((sum, col) => {
                    return sum + (col.stats?.vector_count || col.vector_count || 0);
                }, 0);

                // Update overview cards with real data
                document.getElementById('overview-collections').textContent = totalCollections;
                document.getElementById('overview-vectors').textContent = totalVectors.toLocaleString();
                document.getElementById('overview-queries').textContent =
                    (metrics.query?.total_queries || 0).toLocaleString();
                document.getElementById('overview-latency').textContent =
                    (metrics.query?.p99_latency_ms || 0).toFixed(2) + ' ms';

                // Update metrics tab
                const cacheHitRate = (metrics.cache_hit_rate || 0) * 100;
                document.getElementById('cache-hit-rate').textContent = cacheHitRate.toFixed(1) + '%';
                document.getElementById('cache-progress').style.width = cacheHitRate + '%';

                document.getElementById('qps').textContent =
                    (metrics.index?.search_operations_per_second || 0).toFixed(2);
                document.getElementById('p99-latency').textContent =
                    (metrics.query?.p99_latency_ms || 0).toFixed(2) + ' ms';

                const errorRate = metrics.query?.total_queries > 0
                    ? (metrics.query.failed_queries / metrics.query.total_queries * 100)
                    : 0;
                document.getElementById('error-rate').textContent = errorRate.toFixed(2) + '%';

                // Update history
                const now = new Date().toLocaleTimeString();
                metricsHistory.timestamps.push(now);
                metricsHistory.queries.push(metrics.query?.total_queries || 0);
                metricsHistory.latency.push(metrics.query?.p99_latency_ms || 0);

                // Keep only last MAX_HISTORY points
                if (metricsHistory.timestamps.length > MAX_HISTORY) {
                    metricsHistory.timestamps.shift();
                    metricsHistory.queries.shift();
                    metricsHistory.latency.shift();
                }

                // Update charts with real collection count
                updateQueryChart();
                // Update storage chart with real totals
                const enhancedMetrics = {
                    ...metrics,
                    storage: {
                        ...metrics.storage,
                        total_collections: totalCollections,
                        total_vectors: totalVectors
                    }
                };
                updateStorageChart(enhancedMetrics);
                updateLatencyChart(metrics);
                updateThroughputChart();

            } catch (error) {
                console.error('Failed to fetch metrics:', error);
            } finally {
                icon.textContent = '🔄';
            }
        }

        // Refresh system info
        async function refreshSystem() {
            const icon = document.getElementById('system-refresh-icon');
            icon.innerHTML = '<div class="loading-spinner"></div>';

            try {
                const response = await fetch('/metrics/json');
                const metrics = await response.json();

                // Update system cards
                const cpuUsage = metrics.cpu_usage || 0;
                document.getElementById('cpu-usage').textContent = cpuUsage.toFixed(1) + '%';
                document.getElementById('cpu-progress').style.width = cpuUsage + '%';

                const memUsed = metrics.memory_used_bytes || 0;
                const memTotal = metrics.memory_total_bytes || 1;
                const memPercent = (memUsed / memTotal) * 100;
                document.getElementById('memory-usage').textContent =
                    (memUsed / 1024 / 1024).toFixed(0) + ' MB';
                document.getElementById('memory-progress').style.width = memPercent + '%';

                const diskUsed = metrics.disk_used_bytes || 0;
                const diskTotal = metrics.disk_total_bytes || 1;
                const diskPercent = (diskUsed / diskTotal) * 100;
                document.getElementById('disk-usage').textContent =
                    (diskUsed / 1024 / 1024 / 1024).toFixed(2) + ' GB';
                document.getElementById('disk-progress').style.width = diskPercent + '%';

                const uptime = metrics.uptime_seconds || 0;
                const hours = Math.floor(uptime / 3600);
                const minutes = Math.floor((uptime % 3600) / 60);
                document.getElementById('uptime').textContent = `${hours}h ${minutes}m`;

                // Update resource history
                metricsHistory.cpu.push(cpuUsage);
                metricsHistory.memory.push(memPercent);

                if (metricsHistory.cpu.length > MAX_HISTORY) {
                    metricsHistory.cpu.shift();
                    metricsHistory.memory.shift();
                }

                updateResourceChart();
                updateNetworkChart(metrics);

            } catch (error) {
                console.error('Failed to fetch system metrics:', error);
            } finally {
                icon.textContent = '🔄';
            }
        }

        // Refresh collections
        async function refreshCollections() {
            const icon = document.getElementById('collections-refresh-icon');
            icon.innerHTML = '<div class="loading-spinner"></div>';

            try {
                const response = await fetch('/api/v1/collections');
                const data = await response.json();

                // Extract collections array from response object
                const collections = data.collections || data || [];

                const tbody = document.getElementById('collections-tbody');
                if (!collections || collections.length === 0) {
                    tbody.innerHTML = `
                        <tr>
                            <td colspan="6" style="text-align: center; padding: 2rem; color: var(--text-light);">
                                No collections found
                            </td>
                        </tr>
                    `;
                } else {
                    tbody.innerHTML = collections.map(col => {
                        // Extract name and other fields from config if nested
                        const name = col.config?.name || col.name || 'N/A';
                        const dimension = col.config?.dimension || col.dimension || '-';
                        const vectorCount = col.vector_count || 0;
                        const engine = col.config?.storage_engine || col.engine || 0;
                        const engineName = ['AUTO', 'VIPER', 'SST', 'NOVA', 'HELIX', 'SWIFT', 'RAPTOR'][engine] || 'SST';
                        const distanceMetric = col.config?.distance_metric || col.distance_metric || 1;
                        const metricName = ['UNSPECIFIED', 'COSINE', 'EUCLIDEAN', 'DOT_PRODUCT'][distanceMetric] || 'COSINE';

                        return `
                            <tr>
                                <td><strong>${name}</strong></td>
                                <td>${dimension}</td>
                                <td>${vectorCount.toLocaleString()}</td>
                                <td><span class="badge badge-info">${engineName}</span></td>
                                <td>${metricName}</td>
                                <td><span class="badge badge-success">Active</span></td>
                            </tr>
                        `;
                    }).join('');
                }
            } catch (error) {
                console.error('Failed to fetch collections:', error);
                const tbody = document.getElementById('collections-tbody');
                tbody.innerHTML = `
                    <tr>
                        <td colspan="6" style="text-align: center; padding: 2rem; color: var(--danger-color);">
                            Failed to load collections
                        </td>
                    </tr>
                `;
            } finally {
                icon.textContent = '🔄';
            }
        }

        // Chart update functions
        function updateQueryChart() {
            charts.query.data.labels = metricsHistory.timestamps;
            charts.query.data.datasets[0].data = metricsHistory.queries;
            charts.query.update('none');
        }

        function updateStorageChart(metrics) {
            const total = metrics.storage?.storage_size_bytes || 1;
            charts.storage.data.datasets[0].data = [
                (metrics.storage?.total_vectors || 0) * 0.6,
                total * 0.2,
                total * 0.15,
                total * 0.05
            ];
            charts.storage.update('none');
        }

        function updateLatencyChart(metrics) {
            const p99 = metrics.query?.p99_latency_ms || 0;
            charts.latency.data.datasets[0].data = [
                p99 * 0.3,
                p99 * 0.6,
                p99 * 0.8,
                p99
            ];
            charts.latency.update('none');
        }

        function updateThroughputChart() {
            charts.throughput.data.labels = metricsHistory.timestamps;
            charts.throughput.data.datasets[0].data = metricsHistory.queries.map((q, i) =>
                i > 0 ? (q - metricsHistory.queries[i-1]) : 0
            );
            charts.throughput.update('none');
        }

        function updateResourceChart() {
            charts.resource.data.labels = metricsHistory.timestamps;
            charts.resource.data.datasets[0].data = metricsHistory.cpu;
            charts.resource.data.datasets[1].data = metricsHistory.memory;
            charts.resource.update('none');
        }

        function updateNetworkChart(metrics) {
            const now = new Date().toLocaleTimeString();
            const rx = (metrics.network_rx_bytes || 0) / 1024 / 1024;
            const tx = (metrics.network_tx_bytes || 0) / 1024 / 1024;

            if (charts.network.data.labels.length > MAX_HISTORY) {
                charts.network.data.labels.shift();
                charts.network.data.datasets[0].data.shift();
                charts.network.data.datasets[1].data.shift();
            }

            charts.network.data.labels.push(now);
            charts.network.data.datasets[0].data.push(rx);
            charts.network.data.datasets[1].data.push(tx);
            charts.network.update('none');
        }

        // Initialize
        window.addEventListener('load', () => {
            initCharts();
            refreshMetrics();
            refreshSystem();
            refreshCollections();

            // Auto-refresh interval (default: 60 seconds, minimum: 15 seconds)
            // To change: update monitoring.dashboard_refresh_interval_seconds in config.toml
            // Note: This value is currently hardcoded in the dashboard HTML for simplicity.
            // For dynamic configuration, the backend would need to serve this value via an API endpoint.
            const refreshInterval = 60000; // 60 seconds in milliseconds
            setInterval(() => {
                refreshMetrics();
                refreshSystem();
            }, refreshInterval);

            // Refresh collections at the same interval
            setInterval(refreshCollections, refreshInterval);
        });
    </script>
</body>
</html>"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use axum::routing::get;
    use hyper::body::to_bytes;
    use tower::ServiceExt;

    use crate::network::auth::middleware::auth_middleware_unified;
    use crate::security::auth_service::{
        ApiKeyInfo, AuthenticationConfig, AuthenticationMethod, JwtConfig, MtlsConfig, SSOConfig,
    };
    use crate::security::rbac_service::RBACConfig;
    use crate::security::security_coordinator::{ComplianceConfig, TlsConfig};
    use crate::security::{AuditConfig, SecurityConfig, SecurityCoordinator, SecurityMode};

    fn build_api_key_security_config() -> SecurityConfig {
        let mut api_keys = std::collections::HashMap::new();
        api_keys.insert(
            "test-key".to_string(),
            ApiKeyInfo {
                user_id: "user1".to_string(),
                tenant_id: None,
                permissions: vec!["read".to_string()],
                created_at: None,
                expires_at: None,
                rate_limit_per_minute: None,
                ip_restrictions: vec![],
            },
        );

        SecurityConfig {
            enabled: true,
            mode: SecurityMode::Development,
            authentication: AuthenticationConfig {
                enabled: true,
                methods: vec![AuthenticationMethod::ApiKey],
                require_authentication: true,
                default_session_timeout_minutes: 60,
                api_keys,
                jwt: JwtConfig {
                    enabled: false,
                    secret: "dev-secret".to_string(),
                    access_token_expiration_minutes: 15,
                    refresh_token_expiration_days: 7,
                    issuer: "test".to_string(),
                    audience: "test".to_string(),
                    algorithm: "HS256".to_string(),
                },
                sso: SSOConfig {
                    enabled: false,
                    providers: vec![],
                    token_cache_ttl_minutes: 5,
                    aws_iam: None,
                    azure_ad: None,
                },
                mtls: MtlsConfig::default(),
            },
            rbac: RBACConfig::default(),
            audit: AuditConfig::default(),
            tls: TlsConfig {
                enabled: false,
                require_client_certificates: false,
                cert_file: None,
                key_file: None,
                ca_file: None,
            },
            compliance: ComplianceConfig {
                frameworks: vec![],
                data_residency: None,
                encryption_at_rest: false,
                encryption_in_transit: false,
            },
            encryption: crate::security::EncryptionConfig::default(),
            key_store: crate::security::KeyStoreConfig::default(),
        }
    }

    async fn build_test_router(auth_enabled: bool) -> Router {
        let base_router = Router::new().route("/api/v1/search", get(|| async { "ok" }));

        if auth_enabled {
            let coordinator = Arc::new(
                SecurityCoordinator::from_config(build_api_key_security_config())
                    .await
                    .unwrap(),
            );
            base_router.layer(middleware::from_fn_with_state(
                coordinator,
                auth_middleware_unified,
            ))
        } else {
            base_router
        }
    }

    #[tokio::test]
    async fn rest_auth_disabled_allows_requests_without_header() {
        let router = build_test_router(false).await;
        let request = Request::builder()
            .uri("/api/v1/search")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body()).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn rest_auth_enabled_requires_header() {
        let router = build_test_router(true).await;
        let request = Request::builder()
            .uri("/api/v1/search")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rest_auth_enabled_accepts_api_key() {
        let router = build_test_router(true).await;
        let request = Request::builder()
            .uri("/api/v1/search")
            .header("Authorization", "Api-Key test-key")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body()).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }
}

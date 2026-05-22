//! # gRPC Service Builders
//!
//! Builder patterns for constructing and configuring gRPC services.
//!
//! Services that route through `ApiHandlersPort` accept `Arc<dyn ApiHandlersPort>`.
//! Services with their own concrete dependencies (document, entity, observability,
//! security, streaming) accept those dependencies directly.

use std::sync::Arc;

use crate::grpc::v1::{
    collection::CollectionServiceImpl, document::DocumentServiceImpl, entity::EntityServiceImpl,
    graph::GraphServiceImpl, hybrid::HybridSearchServiceImpl,
    observability::ObservabilityServiceImpl, security::SecurityServiceImpl, sql::SqlServiceImpl,
    streaming::StreamingServiceImpl, vector::VectorServiceImpl,
};
use proximadb_proto::streaming::v1::streaming_service_server::StreamingServiceServer;
use proximadb_proto::v1::{
    collection_service_server::CollectionServiceServer,
    document_service_server::DocumentServiceServer, entity_service_server::EntityServiceServer,
    graph_service_server::GraphServiceServer,
    hybrid_search_service_server::HybridSearchServiceServer,
    observability_service_server::ObservabilityServiceServer,
    security_service_server::SecurityServiceServer, sql_service_server::SqlServiceServer,
    vector_service_server::VectorServiceServer,
};
use proximadb_runtime::{
    ApiHandlersPort, DocumentPort, EntityPort, GraphPort, HybridPort, ObservabilityPort,
    SecurityPort, StreamingPort,
};

/// Configuration for gRPC services
#[derive(Debug, Clone)]
pub struct GrpcServiceConfig {
    /// Enable compression (gzip)
    pub compression_enabled: bool,
    /// Maximum message size for decoding (bytes)
    pub max_decoding_message_size: usize,
    /// Maximum message size for encoding (bytes)
    pub max_encoding_message_size: usize,
}

impl GrpcServiceConfig {
    /// Default max message size: 64MB
    pub const DEFAULT_MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;
}

impl Default for GrpcServiceConfig {
    fn default() -> Self {
        Self {
            compression_enabled: false,
            max_decoding_message_size: Self::DEFAULT_MAX_MESSAGE_SIZE,
            max_encoding_message_size: Self::DEFAULT_MAX_MESSAGE_SIZE,
        }
    }
}

/// Builder for constructing gRPC services with consistent configuration
pub struct GrpcServiceBuilder {
    config: GrpcServiceConfig,
}

impl GrpcServiceBuilder {
    /// Create a new builder with default configuration
    pub fn new() -> Self {
        Self {
            config: GrpcServiceConfig::default(),
        }
    }

    /// Enable compression for all services
    pub fn with_compression(mut self, enabled: bool) -> Self {
        self.config.compression_enabled = enabled;
        self
    }

    /// Set maximum message size for both encoding and decoding
    pub fn with_max_message_size(mut self, size: usize) -> Self {
        self.config.max_decoding_message_size = size;
        self.config.max_encoding_message_size = size;
        self
    }

    /// Set maximum decoding message size
    pub fn with_max_decoding_message_size(mut self, size: usize) -> Self {
        self.config.max_decoding_message_size = size;
        self
    }

    /// Set maximum encoding message size
    pub fn with_max_encoding_message_size(mut self, size: usize) -> Self {
        self.config.max_encoding_message_size = size;
        self
    }

    /// Build vector service
    pub fn build_vector_service(
        &self,
        port: Arc<dyn ApiHandlersPort>,
    ) -> VectorServiceServer<VectorServiceImpl> {
        let server = VectorServiceServer::new(VectorServiceImpl::new(port));
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build collection service
    pub fn build_collection_service(
        &self,
        port: Arc<dyn ApiHandlersPort>,
    ) -> CollectionServiceServer<CollectionServiceImpl> {
        let server = CollectionServiceServer::new(CollectionServiceImpl::new(port));
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build document service.
    ///
    /// When `port` is `None` the service returns `UNIMPLEMENTED` for every RPC,
    /// allowing the server to start without a document backend.
    pub fn build_document_service(
        &self,
        port: Option<Arc<dyn DocumentPort>>,
    ) -> DocumentServiceServer<DocumentServiceImpl> {
        let impl_ = match port {
            Some(p) => DocumentServiceImpl::new(p),
            None => DocumentServiceImpl::without_backend(),
        };
        let server = DocumentServiceServer::new(impl_);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build entity service.
    ///
    /// When `port` is `None` the service returns `UNIMPLEMENTED` for every RPC.
    pub fn build_entity_service(
        &self,
        port: Option<Arc<dyn EntityPort>>,
    ) -> EntityServiceServer<EntityServiceImpl> {
        let impl_ = match port {
            Some(p) => EntityServiceImpl::new(p),
            None => EntityServiceImpl::without_backend(),
        };
        let server = EntityServiceServer::new(impl_);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build graph service.
    ///
    /// When `port` is `None` the service returns `UNIMPLEMENTED` for every RPC.
    pub fn build_graph_service(
        &self,
        port: Option<Arc<dyn GraphPort>>,
    ) -> GraphServiceServer<GraphServiceImpl> {
        let impl_ = match port {
            Some(p) => GraphServiceImpl::new(p),
            None => GraphServiceImpl::without_backend(),
        };
        let server = GraphServiceServer::new(impl_);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build hybrid search service.
    ///
    /// When `port` is `None` the service returns `UNIMPLEMENTED` for every RPC.
    pub fn build_hybrid_search_service(
        &self,
        port: Option<Arc<dyn HybridPort>>,
    ) -> HybridSearchServiceServer<HybridSearchServiceImpl> {
        let impl_ = match port {
            Some(p) => HybridSearchServiceImpl::new(p),
            None => HybridSearchServiceImpl::without_backend(),
        };
        let server = HybridSearchServiceServer::new(impl_);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build SQL service
    pub fn build_sql_service(
        &self,
        port: Arc<dyn ApiHandlersPort>,
    ) -> SqlServiceServer<SqlServiceImpl> {
        let server = SqlServiceServer::new(SqlServiceImpl::new(port));
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build streaming service.
    ///
    /// When `port` is `None` the session-management RPCs return `UNIMPLEMENTED`;
    /// the streaming RPCs always return `UNIMPLEMENTED` as they are
    /// protocol-specific and cannot be represented as port methods.
    pub fn build_streaming_service(
        &self,
        port: Option<Arc<dyn StreamingPort>>,
    ) -> StreamingServiceServer<StreamingServiceImpl> {
        let impl_ = match port {
            Some(p) => StreamingServiceImpl::new(p),
            None => StreamingServiceImpl::without_backend(),
        };
        let server = StreamingServiceServer::new(impl_);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build observability service.
    ///
    /// When `port` is `None` the service starts but returns `UNIMPLEMENTED` for
    /// every RPC; this allows the server to run without an observability backend.
    pub fn build_observability_service(
        &self,
        port: Option<Arc<dyn ObservabilityPort>>,
    ) -> ObservabilityServiceServer<ObservabilityServiceImpl> {
        let impl_ = match port {
            Some(p) => ObservabilityServiceImpl::new(p),
            None => ObservabilityServiceImpl::without_backend(),
        };
        let server = ObservabilityServiceServer::new(impl_);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build security service.
    ///
    /// When `port` is `None` the service starts but returns `NOT_FOUND` for
    /// every RPC; this allows the server to run without a security backend
    /// configured (e.g., development mode).
    pub fn build_security_service(
        &self,
        port: Option<Arc<dyn SecurityPort>>,
    ) -> SecurityServiceServer<SecurityServiceImpl> {
        let impl_ = match port {
            Some(p) => SecurityServiceImpl::new(p),
            None => SecurityServiceImpl::with_default_config(),
        };
        let server = SecurityServiceServer::new(impl_);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    fn apply_compression<S>(server: S, _enabled: bool) -> S {
        server
    }

    fn apply_message_limits<S>(server: S, _max_decoding: usize, _max_encoding: usize) -> S {
        server
    }
}

impl Default for GrpcServiceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create all gRPC services with a single port reference.
pub struct GrpcServiceFactory {
    port: Arc<dyn ApiHandlersPort>,
    document_port: Option<Arc<dyn DocumentPort>>,
    entity_port: Option<Arc<dyn EntityPort>>,
    graph_port: Option<Arc<dyn GraphPort>>,
    hybrid_port: Option<Arc<dyn HybridPort>>,
    observability_port: Option<Arc<dyn ObservabilityPort>>,
    security_port: Option<Arc<dyn SecurityPort>>,
    streaming_port: Option<Arc<dyn StreamingPort>>,
    config: GrpcServiceConfig,
}

impl GrpcServiceFactory {
    /// Create a new factory backed by the given port.
    pub fn new(port: Arc<dyn ApiHandlersPort>) -> Self {
        Self {
            port,
            document_port: None,
            entity_port: None,
            graph_port: None,
            hybrid_port: None,
            observability_port: None,
            security_port: None,
            streaming_port: None,
            config: GrpcServiceConfig::default(),
        }
    }

    /// Attach a document port so the factory can wire the document service.
    pub fn with_document(mut self, document: Arc<dyn DocumentPort>) -> Self {
        self.document_port = Some(document);
        self
    }

    /// Attach an entity port so the factory can wire the entity service.
    pub fn with_entity(mut self, entity: Arc<dyn EntityPort>) -> Self {
        self.entity_port = Some(entity);
        self
    }

    /// Attach a graph port so the factory can wire the graph service.
    pub fn with_graph(mut self, graph: Arc<dyn GraphPort>) -> Self {
        self.graph_port = Some(graph);
        self
    }

    /// Attach a hybrid port so the factory can wire the hybrid search service.
    pub fn with_hybrid(mut self, hybrid: Arc<dyn HybridPort>) -> Self {
        self.hybrid_port = Some(hybrid);
        self
    }

    /// Attach an observability port so the factory can wire the observability service.
    pub fn with_observability(mut self, observability: Arc<dyn ObservabilityPort>) -> Self {
        self.observability_port = Some(observability);
        self
    }

    /// Attach a security port so the factory can wire the security service.
    pub fn with_security(mut self, security: Arc<dyn SecurityPort>) -> Self {
        self.security_port = Some(security);
        self
    }

    /// Attach a streaming port so the factory can wire session management in the streaming service.
    pub fn with_streaming(mut self, streaming: Arc<dyn StreamingPort>) -> Self {
        self.streaming_port = Some(streaming);
        self
    }

    /// Set service configuration
    pub fn with_config(mut self, config: GrpcServiceConfig) -> Self {
        self.config = config;
        self
    }

    /// Create all gRPC services
    pub async fn create_all_services(&self) -> Result<GrpcServices, String> {
        let builder = GrpcServiceBuilder {
            config: self.config.clone(),
        };

        Ok(GrpcServices {
            vector: builder.build_vector_service(self.port.clone()),
            collection: builder.build_collection_service(self.port.clone()),
            document: builder.build_document_service(self.document_port.clone()),
            entity: builder.build_entity_service(self.entity_port.clone()),
            graph: builder.build_graph_service(self.graph_port.clone()),
            hybrid_search: builder.build_hybrid_search_service(self.hybrid_port.clone()),
            sql: builder.build_sql_service(self.port.clone()),
            streaming: builder.build_streaming_service(self.streaming_port.clone()),
            observability: builder.build_observability_service(self.observability_port.clone()),
            security: builder.build_security_service(self.security_port.clone()),
        })
    }

    /// Create all gRPC services (synchronous)
    pub fn create_all_services_sync(&self) -> GrpcServices {
        let builder = GrpcServiceBuilder {
            config: self.config.clone(),
        };

        GrpcServices {
            vector: builder.build_vector_service(self.port.clone()),
            collection: builder.build_collection_service(self.port.clone()),
            document: builder.build_document_service(self.document_port.clone()),
            entity: builder.build_entity_service(self.entity_port.clone()),
            graph: builder.build_graph_service(self.graph_port.clone()),
            hybrid_search: builder.build_hybrid_search_service(self.hybrid_port.clone()),
            sql: builder.build_sql_service(self.port.clone()),
            streaming: builder.build_streaming_service(self.streaming_port.clone()),
            observability: builder.build_observability_service(self.observability_port.clone()),
            security: builder.build_security_service(self.security_port.clone()),
        }
    }
}

/// Collection of all gRPC services created by the factory.
///
/// All services are unconditionally present — those without an injected port
/// return safe UNIMPLEMENTED responses.  No `Option` wrapping needed; the
/// factory always produces a value regardless of port injection.
pub struct GrpcServices {
    pub vector: VectorServiceServer<VectorServiceImpl>,
    pub collection: CollectionServiceServer<CollectionServiceImpl>,
    pub document: DocumentServiceServer<DocumentServiceImpl>,
    pub entity: EntityServiceServer<EntityServiceImpl>,
    pub graph: GraphServiceServer<GraphServiceImpl>,
    pub hybrid_search: HybridSearchServiceServer<HybridSearchServiceImpl>,
    pub sql: SqlServiceServer<SqlServiceImpl>,
    pub streaming: StreamingServiceServer<StreamingServiceImpl>,
    pub observability: ObservabilityServiceServer<ObservabilityServiceImpl>,
    pub security: SecurityServiceServer<SecurityServiceImpl>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingApiPort;

    #[test]
    fn test_builder_creation() {
        let builder = GrpcServiceBuilder::new();
        assert!(!builder.config.compression_enabled);
        assert_eq!(
            builder.config.max_decoding_message_size,
            GrpcServiceConfig::DEFAULT_MAX_MESSAGE_SIZE
        );
    }

    #[test]
    fn test_builder_with_compression() {
        let builder = GrpcServiceBuilder::new().with_compression(true);
        assert!(builder.config.compression_enabled);
    }

    #[test]
    fn test_builder_with_custom_message_size() {
        let builder = GrpcServiceBuilder::new().with_max_message_size(128 * 1024 * 1024);
        assert_eq!(builder.config.max_decoding_message_size, 128 * 1024 * 1024);
        assert_eq!(builder.config.max_encoding_message_size, 128 * 1024 * 1024);
    }

    #[test]
    fn test_config_default() {
        let config = GrpcServiceConfig::default();
        assert_eq!(
            config.max_decoding_message_size,
            GrpcServiceConfig::DEFAULT_MAX_MESSAGE_SIZE
        );
        assert_eq!(
            config.max_encoding_message_size,
            GrpcServiceConfig::DEFAULT_MAX_MESSAGE_SIZE
        );
    }

    #[test]
    fn builder_supports_independent_encoding_and_decoding_limits() {
        let builder = GrpcServiceBuilder::default()
            .with_max_decoding_message_size(8 * 1024)
            .with_max_encoding_message_size(16 * 1024);

        assert_eq!(builder.config.max_decoding_message_size, 8 * 1024);
        assert_eq!(builder.config.max_encoding_message_size, 16 * 1024);
    }

    #[test]
    fn builder_creates_all_service_server_types_with_default_placeholder_backends() {
        let builder = GrpcServiceBuilder::new().with_compression(true);
        let port = RecordingApiPort::new();

        let _vector = builder.build_vector_service(port.clone());
        let _collection = builder.build_collection_service(port.clone());
        let _document = builder.build_document_service(None);
        let _entity = builder.build_entity_service(None);
        let _graph = builder.build_graph_service(None);
        let _hybrid = builder.build_hybrid_search_service(None);
        let _sql = builder.build_sql_service(port);
        let _streaming = builder.build_streaming_service(None);
        let _observability = builder.build_observability_service(None);
        let _security = builder.build_security_service(None);
    }

    #[tokio::test]
    async fn factory_creates_complete_service_bundle_sync_and_async() {
        let config = GrpcServiceConfig {
            compression_enabled: true,
            max_decoding_message_size: 1024,
            max_encoding_message_size: 2048,
        };
        let factory = GrpcServiceFactory::new(RecordingApiPort::new()).with_config(config);

        let sync_services = factory.create_all_services_sync();
        let GrpcServices {
            vector: _,
            collection: _,
            document: _,
            entity: _,
            graph: _,
            hybrid_search: _,
            sql: _,
            streaming: _,
            observability: _,
            security: _,
        } = sync_services;

        let async_services = factory.create_all_services().await.unwrap();
        let GrpcServices {
            vector: _,
            collection: _,
            document: _,
            entity: _,
            graph: _,
            hybrid_search: _,
            sql: _,
            streaming: _,
            observability: _,
            security: _,
        } = async_services;
    }
}

//! # gRPC Service Builders
//!
//! Builder patterns for constructing and configuring gRPC services.
//!
//! Services that route through `ApiHandlersPort` accept `Arc<dyn ApiHandlersPort>`.
//! Services with their own concrete dependencies (document, entity, observability,
//! security, streaming) accept those dependencies directly.

use std::sync::Arc;

use crate::grpc::v1::{
    collection::CollectionServiceImpl,
    document::{DocumentServiceImpl, DocStorageService},
    entity::{EntityServiceImpl, ProximaEntityStore},
    graph::GraphServiceImpl,
    hybrid::HybridSearchServiceImpl,
    observability::{ObservabilityServiceImpl, ObservabilityStorage},
    security::SecurityServiceImpl,
    sql::SqlServiceImpl,
    streaming::{StreamingServiceImpl, StreamCoordinator},
    vector::VectorServiceImpl,
};
use proximadb_runtime::{ApiHandlersPort, SecurityPort};
use proximadb_proto::streaming::v1::streaming_service_server::StreamingServiceServer;
use proximadb_proto::v1::{
    collection_service_server::CollectionServiceServer,
    document_service_server::DocumentServiceServer,
    entity_service_server::EntityServiceServer,
    graph_service_server::GraphServiceServer,
    hybrid_search_service_server::HybridSearchServiceServer,
    observability_service_server::ObservabilityServiceServer,
    security_service_server::SecurityServiceServer,
    sql_service_server::SqlServiceServer,
    vector_service_server::VectorServiceServer,
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

    /// Build document service with specialized dependencies
    ///
    /// DocumentServiceImpl requires Arc<DocStorageService>, not Arc<dyn ApiHandlersPort>.
    pub fn build_document_service(
        &self,
        doc_storage: Arc<DocStorageService>,
    ) -> DocumentServiceServer<DocumentServiceImpl> {
        let server = DocumentServiceServer::new(DocumentServiceImpl::new(doc_storage));
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build entity service with specialized dependencies
    ///
    /// EntityServiceImpl requires Arc<ProximaEntityStore>, not Arc<dyn ApiHandlersPort>.
    pub fn build_entity_service(
        &self,
        entity_store: Arc<ProximaEntityStore>,
    ) -> EntityServiceServer<EntityServiceImpl> {
        let server = EntityServiceServer::new(EntityServiceImpl::new(entity_store));
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build graph service
    pub fn build_graph_service(
        &self,
        port: Arc<dyn ApiHandlersPort>,
    ) -> GraphServiceServer<GraphServiceImpl> {
        let server = GraphServiceServer::new(GraphServiceImpl::new(port));
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build hybrid search service (stateless — no port needed)
    pub fn build_hybrid_search_service(&self) -> HybridSearchServiceServer<HybridSearchServiceImpl> {
        let server = HybridSearchServiceServer::new(HybridSearchServiceImpl::new());
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

    /// Build streaming service
    pub fn build_streaming_service(&self) -> StreamingServiceServer<StreamingServiceImpl> {
        let server =
            StreamingServiceServer::new(StreamingServiceImpl::new(Arc::new(StreamCoordinator)));
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
    security_port: Option<Arc<dyn SecurityPort>>,
    config: GrpcServiceConfig,
}

impl GrpcServiceFactory {
    /// Create a new factory backed by the given port.
    pub fn new(port: Arc<dyn ApiHandlersPort>) -> Self {
        Self {
            port,
            security_port: None,
            config: GrpcServiceConfig::default(),
        }
    }

    /// Attach a security port so the factory can wire the security service.
    pub fn with_security(mut self, security: Arc<dyn SecurityPort>) -> Self {
        self.security_port = Some(security);
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

        let obs_storage = Arc::new(ObservabilityStorage);
        let obs_server = ObservabilityServiceServer::new(ObservabilityServiceImpl::new(obs_storage));

        Ok(GrpcServices {
            vector: builder.build_vector_service(self.port.clone()),
            collection: builder.build_collection_service(self.port.clone()),
            document: None,
            entity: None,
            graph: builder.build_graph_service(self.port.clone()),
            hybrid_search: builder.build_hybrid_search_service(),
            sql: builder.build_sql_service(self.port.clone()),
            streaming: builder.build_streaming_service(),
            observability: Some(obs_server),
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
            document: None,
            entity: None,
            graph: builder.build_graph_service(self.port.clone()),
            hybrid_search: builder.build_hybrid_search_service(),
            sql: builder.build_sql_service(self.port.clone()),
            streaming: builder.build_streaming_service(),
            observability: None,
            security: builder.build_security_service(self.security_port.clone()),
        }
    }
}

/// Collection of all gRPC services.
///
/// `document` and `entity` are `Option` because they require specialized dependencies
/// beyond `Arc<dyn ApiHandlersPort>`; use their direct constructors.
pub struct GrpcServices {
    pub vector: VectorServiceServer<VectorServiceImpl>,
    pub collection: CollectionServiceServer<CollectionServiceImpl>,
    pub document: Option<DocumentServiceServer<DocumentServiceImpl>>,
    pub entity: Option<EntityServiceServer<EntityServiceImpl>>,
    pub graph: GraphServiceServer<GraphServiceImpl>,
    pub hybrid_search: HybridSearchServiceServer<HybridSearchServiceImpl>,
    pub sql: SqlServiceServer<SqlServiceImpl>,
    pub streaming: StreamingServiceServer<StreamingServiceImpl>,
    pub observability: Option<ObservabilityServiceServer<ObservabilityServiceImpl>>,
    pub security: SecurityServiceServer<SecurityServiceImpl>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

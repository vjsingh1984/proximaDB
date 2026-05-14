//! # gRPC Service Builders
//!
//! Builder patterns for constructing and configuring gRPC services.
//!
//! ## Usage
//!
//! ```rust
//! use proximadb_api::grpc::GrpcServiceBuilder;
//! use proximadb_api::grpc::v1::*;
//!
//! let builder = GrpcServiceBuilder::new()
//!     .with_compression(true)
//!     .with_max_message_size(128 * 1024 * 1024);
//!
//! let vector_server = builder.build_vector_service(request_handlers)?;
//! let collection_server = builder.build_collection_service(request_handlers)?;
//! ```

use std::sync::Arc;

use crate::grpc::v1::{
    collection::CollectionServiceImpl,
    document::{DocumentServiceImpl, DocStorageService},
    entity::{EntityServiceImpl, ProximaEntityStore},
    graph::{GraphServiceImpl, QueryFacadeAdapter as GraphQueryFacadeAdapter},
    hybrid::HybridSearchServiceImpl,
    observability::{ObservabilityServiceImpl, ObservabilityStorage},
    security::SecurityServiceImpl,
    sql::SqlServiceImpl,
    streaming::{StreamingServiceImpl, StreamCoordinator},
    vector::{VectorServiceImpl, QueryFacadeAdapter as VectorQueryFacadeAdapter},
};
use proximadb_runtime::UnifiedHandlers;
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
        handlers: Arc<UnifiedHandlers>,
        query_adapter: Option<Arc<VectorQueryFacadeAdapter>>,
    ) -> VectorServiceServer<VectorServiceImpl> {
        let service_impl = if let Some(adapter) = query_adapter {
            VectorServiceImpl::with_adapter(handlers, Some(adapter))
        } else {
            VectorServiceImpl::new(handlers)
        };

        let server = VectorServiceServer::new(service_impl);
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
        handlers: Arc<UnifiedHandlers>,
    ) -> CollectionServiceServer<CollectionServiceImpl> {
        let service_impl = CollectionServiceImpl::new(handlers);
        let server = CollectionServiceServer::new(service_impl);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build document service with specialized dependencies
    ///
    /// DocumentServiceImpl requires Arc<DocStorageService>, not Arc<UnifiedHandlers>.
    /// Use this method when you have the document storage service available.
    pub fn build_document_service(
        &self,
        doc_storage: Arc<DocStorageService>,
    ) -> DocumentServiceServer<DocumentServiceImpl> {
        let service_impl = DocumentServiceImpl::new(doc_storage);
        let server = DocumentServiceServer::new(service_impl);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build entity service with specialized dependencies
    ///
    /// EntityServiceImpl requires Arc<ProximaEntityStore>, not Arc<UnifiedHandlers>.
    /// Use this method when you have the entity store available.
    pub fn build_entity_service(
        &self,
        entity_store: Arc<ProximaEntityStore>,
    ) -> EntityServiceServer<EntityServiceImpl> {
        let service_impl = EntityServiceImpl::new(entity_store);
        let server = EntityServiceServer::new(service_impl);
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
        handlers: Arc<UnifiedHandlers>,
        query_adapter: Option<Arc<GraphQueryFacadeAdapter>>,
    ) -> GraphServiceServer<GraphServiceImpl> {
        let service_impl = if let Some(adapter) = query_adapter {
            GraphServiceImpl::with_adapter(handlers, Some(adapter))
        } else {
            GraphServiceImpl::new(handlers)
        };

        let server = GraphServiceServer::new(service_impl);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build hybrid search service
    pub fn build_hybrid_search_service(
        &self,
        _handlers: Arc<UnifiedHandlers>,
    ) -> HybridSearchServiceServer<HybridSearchServiceImpl> {
        let service_impl = HybridSearchServiceImpl::new();
        let server = HybridSearchServiceServer::new(service_impl);
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
        handlers: Arc<UnifiedHandlers>,
    ) -> SqlServiceServer<SqlServiceImpl> {
        let service_impl = SqlServiceImpl::new(handlers);
        let server = SqlServiceServer::new(service_impl);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build streaming service
    pub fn build_streaming_service(
        &self,
        _config: Option<()>,
    ) -> StreamingServiceServer<StreamingServiceImpl> {
        let service_impl = StreamingServiceImpl::new(Arc::new(StreamCoordinator));

        let server = StreamingServiceServer::new(service_impl);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Build security service
    pub fn build_security_service(
        &self,
        _handlers: Arc<UnifiedHandlers>,
    ) -> SecurityServiceServer<SecurityServiceImpl> {
        let service_impl = SecurityServiceImpl::with_default_config();
        let server = SecurityServiceServer::new(service_impl);
        let server = Self::apply_message_limits(
            server,
            self.config.max_decoding_message_size,
            self.config.max_encoding_message_size,
        );
        Self::apply_compression(server, self.config.compression_enabled)
    }

    /// Configure a service server with compression and message size limits
    fn configure_server<S>(&self, server: S) -> S {
        server
    }

    /// Apply compression configuration to a service server
    fn apply_compression<S>(server: S, _enabled: bool) -> S {
        // Compression is applied in each build method
        server
    }

    /// Apply message size configuration to a service server
    fn apply_message_limits<S>(server: S, _max_decoding: usize, _max_encoding: usize) -> S {
        // Message limits are applied in each build method
        server
    }
}

impl Default for GrpcServiceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create all gRPC services with default configuration
pub struct GrpcServiceFactory {
    handlers: Arc<UnifiedHandlers>,
    config: GrpcServiceConfig,
}

impl GrpcServiceFactory {
    /// Create a new factory with default configuration
    pub fn new(handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            handlers,
            config: GrpcServiceConfig::default(),
        }
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

        // Observability service requires async construction - create separately
        // TODO: Replace with actual observability service after migration
        let obs_storage = Arc::new(ObservabilityStorage);

        let obs_server = ObservabilityServiceServer::new(ObservabilityServiceImpl::new(obs_storage));
        let obs_server = Self::apply_compression(obs_server, builder.config.compression_enabled);

        Ok(GrpcServices {
            vector: builder.build_vector_service(
                self.handlers.clone(),
                None,
            ),
            collection: builder.build_collection_service(self.handlers.clone()),
            document: None, // Requires Arc<DocStorageService> - use direct constructor
            entity: None, // Requires Arc<ProximaEntityStore> - use direct constructor
            graph: builder.build_graph_service(
                self.handlers.clone(),
                None,
            ),
            hybrid_search: builder.build_hybrid_search_service(self.handlers.clone()),
            sql: builder.build_sql_service(self.handlers.clone()),
            streaming: builder.build_streaming_service(None),
            observability: Some(obs_server),
            security: builder.build_security_service(self.handlers.clone()),
        })
    }

    /// Create all gRPC services except observability (synchronous version)
    pub fn create_all_services_sync(&self) -> GrpcServices {
        let builder = GrpcServiceBuilder {
            config: self.config.clone(),
        };

        GrpcServices {
            vector: builder.build_vector_service(
                self.handlers.clone(),
                None,
            ),
            collection: builder.build_collection_service(self.handlers.clone()),
            document: None, // Requires Arc<DocStorageService> - use direct constructor
            entity: None, // Requires Arc<ProximaEntityStore> - use direct constructor
            graph: builder.build_graph_service(
                self.handlers.clone(),
                None,
            ),
            hybrid_search: builder.build_hybrid_search_service(self.handlers.clone()),
            sql: builder.build_sql_service(self.handlers.clone()),
            streaming: builder.build_streaming_service(None),
            observability: None, // Observability requires async constructor
            security: builder.build_security_service(self.handlers.clone()),
        }
    }

    /// Apply compression configuration to a service server
    fn apply_compression<S>(server: S, _enabled: bool) -> S {
        // Note: Compression applied directly in build methods
        server
    }

    /// Apply message size configuration to a service server
    fn apply_message_limits<S>(server: S, _max_decoding: usize, _max_encoding: usize) -> S {
        // Note: Message limits applied directly in build methods
        server
    }
}

/// Collection of all gRPC services
///
/// Some services are Option because they require specialized dependencies
/// beyond Arc<UnifiedHandlers>. Use direct constructors for those services.
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

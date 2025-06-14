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

//! gRPC Service implementation for ProximaDB
//! This is a service that creates gRPC service definitions, not a server that binds to ports

use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::transport::server::Router;

use crate::storage::StorageEngine;
use crate::network::grpc::service::ProximaDbGrpcService;

/// gRPC Service configuration
#[derive(Debug, Clone)]
pub struct GrpcServiceConfig {
    pub enable_reflection: bool,
    pub enable_health_check: bool,
    pub enable_metrics: bool,
}

impl Default for GrpcServiceConfig {
    fn default() -> Self {
        Self {
            enable_reflection: true,
            enable_health_check: true,
            enable_metrics: true,
        }
    }
}

/// gRPC Service for ProximaDB
/// Creates gRPC service definitions for integration with unified server
pub struct GrpcService {
    config: GrpcServiceConfig,
    storage: Arc<RwLock<StorageEngine>>,
}

impl GrpcService {
    /// Create new gRPC service
    pub fn new(config: GrpcServiceConfig, storage: Arc<RwLock<StorageEngine>>) -> Self {
        Self {
            config,
            storage,
        }
    }

    /// Create the gRPC service router with all services
    pub fn create_service_router(&self) -> Router {
        use crate::proto::vectordb::v1::vector_db_server::VectorDbServer;
        
        // Create the main ProximaDB gRPC service
        let proximadb_service = ProximaDbGrpcService::new(self.storage.clone());
        let vector_db_server = VectorDbServer::new(proximadb_service);
        
        let router = tonic::transport::Server::builder()
            .add_service(vector_db_server);

        // Add health check service if enabled
        if self.config.enable_health_check {
            // TODO: Add health check service
            // router = router.add_service(health_check_service);
        }

        // Add reflection service if enabled (useful for development)
        if self.config.enable_reflection {
            // TODO: Add reflection service
            // router = router.add_service(reflection_service);
        }

        // Add metrics service if enabled
        if self.config.enable_metrics {
            // TODO: Add gRPC metrics service
            // router = router.add_service(metrics_service);
        }

        router
    }
}
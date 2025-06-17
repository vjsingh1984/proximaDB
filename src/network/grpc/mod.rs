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

pub mod service;

use crate::storage::StorageEngine;
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::transport::Server;
use tracing::info;

/// Start the gRPC server
pub async fn start_grpc_server(
    storage: Arc<RwLock<StorageEngine>>,
    addr: std::net::SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::proto::proximadb::v1::proxima_db_server::ProximaDbServer;
    
    info!("Starting gRPC server on {}", addr);
    
    // Create service instances
    let vector_service = crate::services::VectorService::new(storage.clone());
    let collection_service = crate::services::CollectionService::new(storage.clone());
    
    let service = service::ProximaDbGrpcService::new(
        storage,
        vector_service,
        collection_service,
        "grpc-node".to_string(),
        "0.1.0".to_string(),
    );
    let server = ProximaDbServer::new(service);
    
    Server::builder()
        .add_service(server)
        .serve(addr)
        .await?;
    
    Ok(())
}
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

use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use super::handlers::{create_router, AppState};
use crate::services::unified_avro_service::UnifiedAvroService;
use crate::services::collection_service::CollectionService;

/// REST server for ProximaDB
pub struct RestServer {
    router: Router,
    bind_addr: SocketAddr,
}

impl RestServer {
    /// Create new REST server
    pub fn new(
        bind_addr: SocketAddr,
        unified_service: Arc<UnifiedAvroService>,
        collection_service: Arc<CollectionService>,
    ) -> Self {
        let state = AppState {
            unified_service,
            collection_service,
        };
        
        let router = create_router(state)
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(
                        CorsLayer::new()
                            .allow_origin(Any)
                            .allow_methods(Any)
                            .allow_headers(Any)
                    )
            );
        
        Self {
            router,
            bind_addr,
        }
    }
    
    /// Start the REST server
    pub async fn start(self) -> anyhow::Result<()> {
        tracing::info!("🌐 Starting REST server on {}", self.bind_addr);
        
        tracing::info!("✅ REST server listening on {}", self.bind_addr);
        tracing::info!("📋 Available endpoints:");
        tracing::info!("   GET    /health                           - Health check");
        tracing::info!("   POST   /collections                      - Create collection");
        tracing::info!("   GET    /collections                      - List collections");
        tracing::info!("   GET    /collections/:id                  - Get collection");
        tracing::info!("   DELETE /collections/:id                  - Delete collection");
        tracing::info!("   POST   /collections/:id/vectors          - Insert vector");
        tracing::info!("   GET    /collections/:id/vectors/:vid     - Get vector");
        tracing::info!("   PUT    /collections/:id/vectors/:vid     - Update vector");
        tracing::info!("   DELETE /collections/:id/vectors/:vid     - Delete vector");
        tracing::info!("   POST   /collections/:id/search           - Search vectors");
        tracing::info!("   POST   /collections/:id/vectors/batch    - Batch insert");
        
        // For axum 0.6, use axum::Server
        axum::Server::bind(&self.bind_addr)
            .serve(self.router.into_make_service())
            .await?;
        
        Ok(())
    }
}
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

use axum::{Router, extract::DefaultBodyLimit};
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::DecompressionLayer;

use super::handlers::{create_router, AppState};
use crate::api_handlers::UnifiedHandlers;

/// REST server for ProximaDB
pub struct RestServer {
    router: Router,
    bind_addr: SocketAddr,
}

impl RestServer {
    /// Create new REST server
    pub fn new(
        bind_addr: SocketAddr,
        unified_handlers: Arc<UnifiedHandlers>,
        max_request_size_mb: Option<u64>,
        compression: bool,
    ) -> Self {
        let state = AppState {
            unified_handlers,
        };

        // Calculate max request size in bytes (default to 64MB if not specified)
        let max_size_bytes = max_request_size_mb * 1024 * 1024;

        // Build service layers conditionally to avoid type mismatch
        let router = if compression {
            // Create compression layer with support for multiple algorithms
            // Priority order (fastest to best compression): deflate, gzip, zstd, brotli
            let compression_layer = CompressionLayer::new()
                .deflate(true)  // Fastest, low CPU usage
                .gzip(true)     // Good balance of speed and compression
                .zstd(true)     // Best compression ratio with good speed
                .br(true);      // Brotli - slower but excellent compression
            
            // Create decompression layer for handling compressed requests
            let decompression_layer = DecompressionLayer::new()
                .deflate(true)
                .gzip(true)
                .br(true)
                .zstd(true);

            create_router(state).layer(
                ServiceBuilder::new()
                    .layer(DefaultBodyLimit::max(max_size_bytes as usize))
                    .layer(decompression_layer)  // Handle compressed requests
                    .layer(compression_layer)    // Compress responses
                    .layer(TraceLayer::new_for_http())
                    .layer(
                        CorsLayer::new()
                            .allow_origin(Any)
                            .allow_methods(Any)
                            .allow_headers(Any),
                    ),
            )
        } else {
            create_router(state).layer(
                ServiceBuilder::new()
                    .layer(DefaultBodyLimit::max(max_size_bytes as usize))
                    .layer(TraceLayer::new_for_http())
                    .layer(
                        CorsLayer::new()
                            .allow_origin(Any)
                            .allow_methods(Any)
                            .allow_headers(Any),
                    ),
            )
        };

        Self { router, bind_addr }
    }

    /// Start the REST server
    pub async fn start(self) -> anyhow::Result<()> {
        tracing::info!("🌐 Starting REST server on {}", self.bind_addr);

        tracing::info!("✅ REST server listening on {}", self.bind_addr);
        tracing::info!("🗜️  Compression enabled: deflate, gzip, zstd, brotli (in priority order)");
        tracing::info!("📋 Available endpoints:");
        tracing::info!("   GET    /health                           - Health check");
        tracing::info!("   POST   /api/v1/collection                - Unified collection operations");
        tracing::info!("   POST   /api/v1/vector/batch              - Vector batch operations");
        tracing::info!("   POST   /api/v1/vector/search             - Vector search");
        tracing::info!("   POST   /internal/flush                   - Flush all (testing only)");
        tracing::info!("   POST   /internal/flush/:id               - Flush collection (testing only)");

        // For axum 0.6, use axum::Server
        axum::Server::bind(&self.bind_addr)
            .serve(self.router.into_make_service())
            .await?;

        Ok(())
    }
}

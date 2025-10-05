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
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::decompression::DecompressionLayer;
use tower_http::trace::TraceLayer;

use super::v1::handlers::{AppState, create_router};
use crate::api_handlers::UnifiedHandlers;
use crate::monitoring::MetricsCollector;

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
        metrics_collector: Option<Arc<MetricsCollector>>,
    ) -> Self {
        let state = AppState { unified_handlers };

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
        let mut base_router = create_router(state);

        // Nest metrics router if available
        if let Some(metrics) = metrics_router {
            base_router = base_router.nest("/metrics", metrics);
            tracing::info!("✅ Metrics endpoints enabled at /metrics");
        }

        // Add dashboard route
        base_router = base_router.route("/dashboard", axum::routing::get(dashboard_handler));

        let router = if compression {
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

            base_router.layer(
                ServiceBuilder::new()
                    .layer(DefaultBodyLimit::max(max_size_bytes as usize))
                    .layer(decompression_layer) // Handle compressed requests
                    .layer(compression_layer) // Compress responses
                    .layer(TraceLayer::new_for_http())
                    .layer(
                        CorsLayer::new()
                            .allow_origin(Any)
                            .allow_methods(Any)
                            .allow_headers(Any),
                    ),
            )
        } else {
            base_router.layer(
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
        tracing::info!("🔧 REST server using v1 handlers with collection endpoints enabled");

        tracing::info!("✅ REST server listening on {}", self.bind_addr);
        tracing::info!("🗜️  Compression enabled: deflate, gzip, zstd, brotli (in priority order)");
        tracing::info!("📋 Available endpoints:");
        tracing::info!("   GET    /health                           - Health check");
        tracing::info!("   GET    /dashboard                        - Web dashboard");
        tracing::info!("   GET    /metrics                          - Prometheus metrics");
        tracing::info!("   GET    /metrics/json                     - JSON metrics");
        tracing::info!("   GET    /metrics/health                   - Metrics health check");
        tracing::info!("   POST   /api/v1/search                    - Vector search");
        tracing::info!("   POST   /api/v1/vectors/batch             - Vector batch operations");
        tracing::info!("   POST   /api/v1/progressive/search/:id    - Progressive search (JSON)");
        tracing::info!(
            "   POST   /api/v1/collections               - Unified collection operations"
        );
        tracing::info!("   GET    /api/v1/collections               - List collections");
        tracing::info!("   GET    /api/v1/collections/:id           - Get collection by ID");
        tracing::info!("   DELETE /api/v1/collections/:id           - Delete collection");
        tracing::info!("   POST   /api/v1/search/with_metadata      - Vector search with metadata");

        // For axum 0.6, use axum::Server
        axum::Server::bind(&self.bind_addr)
            .serve(self.router.into_make_service())
            .await?;

        Ok(())
    }
}

/// Dashboard handler - serves a simple HTML dashboard
async fn dashboard_handler() -> axum::response::Html<&'static str> {
    axum::response::Html(r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>ProximaDB Dashboard</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, Cantarell, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 20px;
        }
        .container {
            background: white;
            border-radius: 16px;
            box-shadow: 0 20px 60px rgba(0,0,0,0.3);
            padding: 40px;
            max-width: 900px;
            width: 100%;
        }
        h1 {
            color: #667eea;
            font-size: 2.5rem;
            margin-bottom: 10px;
        }
        .subtitle {
            color: #666;
            font-size: 1.1rem;
            margin-bottom: 30px;
        }
        .metrics-container {
            background: #f8f9fa;
            border-radius: 8px;
            padding: 20px;
            margin: 20px 0;
        }
        .metrics-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 15px;
            margin-top: 15px;
        }
        .metric-card {
            background: white;
            padding: 15px;
            border-radius: 8px;
            border-left: 4px solid #667eea;
        }
        .metric-label {
            color: #666;
            font-size: 0.875rem;
            margin-bottom: 5px;
        }
        .metric-value {
            color: #333;
            font-size: 1.5rem;
            font-weight: bold;
        }
        .links {
            display: flex;
            gap: 15px;
            flex-wrap: wrap;
            margin-top: 30px;
        }
        .link-card {
            flex: 1;
            min-width: 250px;
            background: #f8f9fa;
            padding: 20px;
            border-radius: 8px;
            text-decoration: none;
            color: inherit;
            transition: transform 0.2s, box-shadow 0.2s;
        }
        .link-card:hover {
            transform: translateY(-2px);
            box-shadow: 0 4px 12px rgba(0,0,0,0.1);
        }
        .link-title {
            color: #667eea;
            font-size: 1.1rem;
            font-weight: 600;
            margin-bottom: 8px;
        }
        .link-desc {
            color: #666;
            font-size: 0.9rem;
        }
        .status {
            display: inline-block;
            padding: 4px 12px;
            border-radius: 12px;
            font-size: 0.875rem;
            font-weight: 600;
            background: #d4edda;
            color: #155724;
        }
        #refresh-btn {
            background: #667eea;
            color: white;
            border: none;
            padding: 10px 20px;
            border-radius: 6px;
            cursor: pointer;
            font-size: 1rem;
            margin-top: 15px;
        }
        #refresh-btn:hover {
            background: #5568d3;
        }
        .loading {
            color: #666;
            font-style: italic;
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>ProximaDB Dashboard</h1>
        <p class="subtitle">Real-time Monitoring & Metrics</p>

        <div style="margin: 20px 0;">
            <span class="status">● ONLINE</span>
        </div>

        <div class="metrics-container">
            <h2 style="margin-bottom: 10px; color: #333;">System Metrics</h2>
            <button id="refresh-btn" onclick="refreshMetrics()">Refresh Metrics</button>
            <div class="metrics-grid" id="metrics-grid">
                <div class="metric-card">
                    <div class="metric-label">Total Collections</div>
                    <div class="metric-value loading" id="total-collections">-</div>
                </div>
                <div class="metric-card">
                    <div class="metric-label">Total Vectors</div>
                    <div class="metric-value loading" id="total-vectors">-</div>
                </div>
                <div class="metric-card">
                    <div class="metric-label">Cache Hit Rate</div>
                    <div class="metric-value loading" id="cache-hit-rate">-</div>
                </div>
                <div class="metric-card">
                    <div class="metric-label">Memory Usage</div>
                    <div class="metric-value loading" id="memory-usage">-</div>
                </div>
            </div>
        </div>

        <div class="links">
            <a href="/metrics" class="link-card">
                <div class="link-title">Metrics (Prometheus)</div>
                <div class="link-desc">View raw Prometheus metrics for monitoring integration</div>
            </a>
            <a href="/metrics/json" class="link-card">
                <div class="link-title">Metrics (JSON)</div>
                <div class="link-desc">View metrics in JSON format for custom integrations</div>
            </a>
            <a href="/health" class="link-card">
                <div class="link-title">Health Check</div>
                <div class="link-desc">System health status and component checks</div>
            </a>
            <a href="/api/v1/collections" class="link-card">
                <div class="link-title">Collections API</div>
                <div class="link-desc">REST API for collection management</div>
            </a>
        </div>
    </div>

    <script>
        async function refreshMetrics() {
            try {
                const response = await fetch('/metrics/json');
                const metrics = await response.json();

                // Update metric values
                document.getElementById('total-collections').textContent =
                    metrics.collection_count || '0';
                document.getElementById('total-vectors').textContent =
                    (metrics.vector_count || 0).toLocaleString();
                document.getElementById('cache-hit-rate').textContent =
                    ((metrics.cache_hit_rate || 0) * 100).toFixed(1) + '%';
                document.getElementById('memory-usage').textContent =
                    ((metrics.memory_usage_bytes || 0) / 1024 / 1024).toFixed(0) + ' MB';
            } catch (error) {
                console.error('Failed to fetch metrics:', error);
            }
        }

        // Auto-refresh metrics every 5 seconds
        refreshMetrics();
        setInterval(refreshMetrics, 5000);
    </script>
</body>
</html>"#)
}

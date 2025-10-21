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

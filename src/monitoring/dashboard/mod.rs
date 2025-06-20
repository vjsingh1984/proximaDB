// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Web dashboard for monitoring ProximaDB

use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, Json},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::monitoring::metrics::{MetricsCollector, SystemMetrics};

/// Dashboard server state
#[derive(Clone)]
pub struct DashboardState {
    metrics_collector: Arc<MetricsCollector>,
}

/// Query parameters for metrics endpoint
#[derive(Debug, Deserialize)]
pub struct MetricsQuery {
    since: Option<String>,  // ISO 8601 timestamp
    format: Option<String>, // json, prometheus, html
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub version: String,
    pub uptime_seconds: f64,
}

/// Dashboard home page
async fn dashboard_home(State(state): State<DashboardState>) -> Result<Html<String>, StatusCode> {
    let metrics = state.metrics_collector.get_current_metrics().await;
    let summary = state.metrics_collector.get_metrics_summary().await;

    let html = format!(
        r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>ProximaDB Dashboard</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 0; padding: 20px; background-color: #f5f5f5; }}
        .header {{ background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); color: white; padding: 20px; border-radius: 8px; margin-bottom: 20px; }}
        .metrics-grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 20px; }}
        .metric-card {{ background: white; padding: 20px; border-radius: 8px; box-shadow: 0 2px 4px rgba(0,0,0,0.1); }}
        .metric-title {{ font-size: 18px; font-weight: bold; color: #333; margin-bottom: 10px; }}
        .metric-value {{ font-size: 24px; font-weight: bold; color: #667eea; }}
        .metric-unit {{ font-size: 14px; color: #666; }}
        .health-indicator {{ display: inline-block; width: 12px; height: 12px; border-radius: 50%; margin-right: 8px; }}
        .health-good {{ background-color: #4CAF50; }}
        .health-warning {{ background-color: #FF9800; }}
        .health-critical {{ background-color: #F44336; }}
        .refresh-note {{ color: #666; font-size: 12px; margin-top: 20px; }}
        .nav-links {{ margin: 20px 0; }}
        .nav-links a {{ margin-right: 20px; color: #667eea; text-decoration: none; }}
        .nav-links a:hover {{ text-decoration: underline; }}
    </style>
    <script>
        setTimeout(() => location.reload(), 30000); // Auto-refresh every 30 seconds
    </script>
</head>
<body>
    <div class="header">
        <h1>🚀 ProximaDB Dashboard</h1>
        <p>Real-time monitoring and metrics for your vector database</p>
        <div class="health-indicator {}"></div>
        <span>System Health: {:.1}%</span>
    </div>
    
    <div class="nav-links">
        <a href="/">Dashboard</a>
        <a href="/metrics">Raw Metrics</a>
        <a href="/health">Health Check</a>
        <a href="/alerts">Alerts</a>
        <a href="/api/metrics">API Metrics</a>
    </div>
    
    <div class="metrics-grid">
        <div class="metric-card">
            <div class="metric-title">Server Status</div>
            <div class="metric-value">{:.1} <span class="metric-unit">% CPU</span></div>
            <div>Memory: {:.1} MB used</div>
            <div>Uptime: {:.1} hours</div>
        </div>
        
        <div class="metric-card">
            <div class="metric-title">Storage</div>
            <div class="metric-value">{} <span class="metric-unit">vectors</span></div>
            <div>Collections: {}</div>
            <div>Storage: {:.1} MB</div>
            <div>Cache Hit Rate: {:.1}%</div>
        </div>
        
        <div class="metric-card">
            <div class="metric-title">Query Performance</div>
            <div class="metric-value">{:.1} <span class="metric-unit">QPS</span></div>
            <div>P99 Latency: {:.1} ms</div>
            <div>Total Queries: {}</div>
            <div>Error Rate: {:.2}%</div>
        </div>
        
        <div class="metric-card">
            <div class="metric-title">Indexing</div>
            <div class="metric-value">{} <span class="metric-unit">indexes</span></div>
            <div>Index Memory: {:.1} MB</div>
            <div>Searches/sec: {:.1}</div>
        </div>
        
        <div class="metric-card">
            <div class="metric-title">Alerts</div>
            <div class="metric-value">{} <span class="metric-unit">active</span></div>
            <div>Critical: {}</div>
            <div>Last Updated: {}</div>
        </div>
    </div>
    
    <div class="refresh-note">
        📊 Dashboard auto-refreshes every 30 seconds | Last updated: {}
    </div>
</body>
</html>
    "#,
        if summary.system_health > 0.8 {
            "health-good"
        } else if summary.system_health > 0.5 {
            "health-warning"
        } else {
            "health-critical"
        },
        summary.system_health * 100.0,
        summary.cpu_usage,
        summary.memory_usage_percent / 1024.0 / 1024.0,
        metrics.server.uptime_seconds / 3600.0,
        metrics.storage.total_vectors,
        metrics.storage.total_collections,
        metrics.storage.storage_size_bytes as f64 / 1024.0 / 1024.0,
        summary.cache_hit_rate * 100.0,
        summary.queries_per_second,
        summary.query_latency_p99,
        metrics.query.total_queries,
        (metrics.query.failed_queries as f64 / metrics.query.total_queries.max(1) as f64) * 100.0,
        metrics.index.total_indexes,
        metrics.index.index_memory_usage_bytes as f64 / 1024.0 / 1024.0,
        metrics.index.search_operations_per_second,
        summary.active_alerts_count,
        summary.critical_alerts_count,
        metrics.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );

    Ok(Html(html))
}

/// Health check endpoint
async fn health_check(State(state): State<DashboardState>) -> Json<HealthResponse> {
    let metrics = state.metrics_collector.get_current_metrics().await;

    Json(HealthResponse {
        status: "healthy".to_string(),
        timestamp: chrono::Utc::now(),
        version: "0.1.0".to_string(),
        uptime_seconds: metrics.server.uptime_seconds,
    })
}

/// Metrics endpoint (Prometheus format)
async fn metrics_endpoint(
    Query(params): Query<MetricsQuery>,
    State(state): State<DashboardState>,
) -> Result<String, StatusCode> {
    let format = params.format.unwrap_or_else(|| "prometheus".to_string());

    match format.as_str() {
        "json" => {
            let metrics = state.metrics_collector.get_current_metrics().await;
            serde_json::to_string_pretty(&metrics).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
        "prometheus" | _ => {
            let metrics = state.metrics_collector.get_current_metrics().await;
            use crate::monitoring::metrics::exporters::{MetricsExporter, PrometheusExporter};
            let exporter = PrometheusExporter::new();
            exporter
                .export_system_metrics(&metrics)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// API metrics endpoint (JSON)
async fn api_metrics_endpoint(
    Query(params): Query<MetricsQuery>,
    State(state): State<DashboardState>,
) -> Json<SystemMetrics> {
    let _since = params.since; // TODO: Use this for historical data
    let metrics = state.metrics_collector.get_current_metrics().await;
    Json(metrics)
}

/// Alerts endpoint
async fn alerts_endpoint(
    State(state): State<DashboardState>,
) -> Json<Vec<crate::monitoring::metrics::Alert>> {
    let alerts = state.metrics_collector.get_active_alerts().await;
    Json(alerts)
}

/// Alerts page
async fn alerts_page(State(state): State<DashboardState>) -> Result<Html<String>, StatusCode> {
    let alerts = state.metrics_collector.get_active_alerts().await;

    let alerts_html = if alerts.is_empty() {
        "<p>🎉 No active alerts</p>".to_string()
    } else {
        alerts
            .iter()
            .map(|alert| {
                format!(
                    r#"
                <div class="alert alert-{}">
                    <h3>{}</h3>
                    <p>{}</p>
                    <small>Current: {:.2} | Threshold: {:.2} | Time: {}</small>
                </div>
            "#,
                    match alert.level {
                        crate::monitoring::metrics::AlertLevel::Critical => "critical",
                        crate::monitoring::metrics::AlertLevel::Warning => "warning",
                        crate::monitoring::metrics::AlertLevel::Info => "info",
                    },
                    alert.metric_name,
                    alert.message,
                    alert.current_value,
                    alert.threshold_value,
                    alert.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let html = format!(
        r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>ProximaDB Alerts</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 0; padding: 20px; background-color: #f5f5f5; }}
        .header {{ background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); color: white; padding: 20px; border-radius: 8px; margin-bottom: 20px; }}
        .alert {{ background: white; padding: 15px; margin: 10px 0; border-radius: 8px; border-left: 4px solid; }}
        .alert-critical {{ border-left-color: #F44336; }}
        .alert-warning {{ border-left-color: #FF9800; }}
        .alert-info {{ border-left-color: #2196F3; }}
        .nav-links {{ margin: 20px 0; }}
        .nav-links a {{ margin-right: 20px; color: #667eea; text-decoration: none; }}
        .nav-links a:hover {{ text-decoration: underline; }}
    </style>
</head>
<body>
    <div class="header">
        <h1>🚨 ProximaDB Alerts</h1>
        <p>Active system alerts and notifications</p>
    </div>
    
    <div class="nav-links">
        <a href="/">Dashboard</a>
        <a href="/metrics">Raw Metrics</a>
        <a href="/health">Health Check</a>
        <a href="/alerts">Alerts</a>
        <a href="/api/alerts">API Alerts</a>
    </div>
    
    <div class="alerts-container">
        {}
    </div>
</body>
</html>
    "#,
        alerts_html
    );

    Ok(Html(html))
}

/// Create dashboard router for integration with UnifiedServer
pub fn create_dashboard_router(metrics_collector: Arc<MetricsCollector>) -> Router {
    let state = DashboardState { metrics_collector };

    Router::new()
        .route("/", get(dashboard_home))
        .route("/health", get(health_check))
        .route("/metrics", get(metrics_endpoint))
        .route("/api/metrics", get(api_metrics_endpoint))
        .route("/api/alerts", get(alerts_endpoint))
        .route("/alerts", get(alerts_page))
        .with_state(state)
}

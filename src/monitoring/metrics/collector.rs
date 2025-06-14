// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metrics collection engine for ProximaDB

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc};
use tokio::time::interval;
use anyhow::Result;
use chrono::{DateTime, Utc};
use sysinfo::System;

use super::{
    SystemMetrics, ServerMetrics, StorageMetrics, IndexMetrics, QueryMetrics,
    MetricsConfig, Alert, MetricsRateLimiter
};

/// Central metrics collector that gathers system and application metrics
pub struct MetricsCollector {
    config: MetricsConfig,
    
    /// System information collector
    system: Arc<RwLock<System>>,
    
    /// Current metrics snapshot
    current_metrics: Arc<RwLock<SystemMetrics>>,
    
    /// Historical metrics storage (last N samples)
    metrics_history: Arc<RwLock<Vec<SystemMetrics>>>,
    
    /// Active alerts
    active_alerts: Arc<RwLock<Vec<Alert>>>,
    
    /// Rate limiter for metrics emission
    rate_limiter: MetricsRateLimiter,
    
    /// Start time for uptime calculation
    start_time: Instant,
    
    /// Metrics event sender
    event_sender: mpsc::UnboundedSender<MetricsEvent>,
    
    /// Shutdown signal
    shutdown: Arc<RwLock<bool>>,
}

/// Metrics collection events
#[derive(Debug, Clone)]
pub enum MetricsEvent {
    SystemMetricsUpdated(SystemMetrics),
    AlertGenerated(Alert),
    AlertResolved(String), // Alert ID
    MetricsCleanup,
}

/// Custom metrics interface for application components
pub trait MetricsProvider {
    async fn collect_metrics(&self) -> Result<HashMap<String, f64>>;
}

impl MetricsCollector {
    /// Create new metrics collector
    pub fn new(config: MetricsConfig) -> Result<(Self, mpsc::UnboundedReceiver<MetricsEvent>)> {
        let system = Arc::new(RwLock::new(System::new_all()));
        let (tx, rx) = mpsc::unbounded_channel();
        
        let collector = Self {
            config: config.clone(),
            system,
            current_metrics: Arc::new(RwLock::new(SystemMetrics::new())),
            metrics_history: Arc::new(RwLock::new(Vec::new())),
            active_alerts: Arc::new(RwLock::new(Vec::new())),
            rate_limiter: MetricsRateLimiter::new(Duration::from_secs(
                config.collection_interval_seconds.max(1)
            )),
            start_time: Instant::now(),
            event_sender: tx,
            shutdown: Arc::new(RwLock::new(false)),
        };
        
        Ok((collector, rx))
    }
    
    /// Start the metrics collection loop
    pub async fn start(&self) -> Result<()> {
        println!("Starting ProximaDB metrics collector...");
        
        let mut collection_interval = interval(Duration::from_secs(self.config.collection_interval_seconds));
        let mut cleanup_interval = interval(Duration::from_secs(3600)); // Cleanup every hour
        
        loop {
            tokio::select! {
                _ = collection_interval.tick() => {
                    if *self.shutdown.read().await {
                        break;
                    }
                    
                    if let Err(e) = self.collect_all_metrics().await {
                        eprintln!("Error collecting metrics: {}", e);
                    }
                }
                
                _ = cleanup_interval.tick() => {
                    if *self.shutdown.read().await {
                        break;
                    }
                    
                    self.cleanup_old_metrics().await;
                    let _ = self.event_sender.send(MetricsEvent::MetricsCleanup);
                }
            }
        }
        
        println!("Metrics collector stopped");
        Ok(())
    }
    
    /// Stop the metrics collector
    pub async fn stop(&self) {
        let mut shutdown = self.shutdown.write().await;
        *shutdown = true;
    }
    
    /// Collect all system and application metrics
    async fn collect_all_metrics(&self) -> Result<()> {
        // Only collect if rate limiter allows
        if !self.rate_limiter.should_emit().await {
            return Ok(());
        }
        
        // Refresh system information
        let mut system = self.system.write().await;
        system.refresh_all();
        
        // Collect server metrics
        let server_metrics = self.collect_server_metrics(&system).await?;
        
        // Collect storage metrics (placeholder for now)
        let storage_metrics = self.collect_storage_metrics().await?;
        
        // Collect index metrics (placeholder for now)
        let index_metrics = self.collect_index_metrics().await?;
        
        // Collect query metrics (placeholder for now)
        let query_metrics = self.collect_query_metrics().await?;
        
        drop(system); // Release the lock
        
        // Create comprehensive metrics snapshot
        let metrics = SystemMetrics {
            timestamp: Utc::now(),
            server: server_metrics,
            storage: storage_metrics,
            index: index_metrics,
            query: query_metrics,
            custom: HashMap::new(),
        };
        
        // Check for alerts
        let alerts = metrics.check_alerts(&self.config.alert_thresholds);
        self.process_alerts(alerts).await;
        
        // Update current metrics
        {
            let mut current = self.current_metrics.write().await;
            *current = metrics.clone();
        }
        
        // Add to history
        {
            let mut history = self.metrics_history.write().await;
            history.push(metrics.clone());
            
            // Limit history size based on retention
            let max_samples = (self.config.retention_hours * 3600) / self.config.collection_interval_seconds;
            if history.len() > max_samples as usize {
                history.remove(0);
            }
        }
        
        // Emit event
        let _ = self.event_sender.send(MetricsEvent::SystemMetricsUpdated(metrics));
        
        Ok(())
    }
    
    /// Collect server-level metrics
    async fn collect_server_metrics(&self, system: &System) -> Result<ServerMetrics> {
        let uptime = self.start_time.elapsed().as_secs_f64();
        
        // CPU usage (average across all cores)
        let cpu_usage = system.cpus()
            .iter()
            .map(|cpu| cpu.cpu_usage() as f64)
            .sum::<f64>() / system.cpus().len() as f64;
        
        // Memory usage
        let memory_total = system.total_memory();
        let memory_used = system.used_memory();
        let _memory_available = system.available_memory();
        
        // Disk usage (placeholder - sysinfo API varies by version)
        let disk_total = 1000000000u64; // 1GB placeholder
        let disk_available = 500000000u64; // 500MB placeholder
        let disk_used = disk_total.saturating_sub(disk_available);
        
        // Network usage (placeholder - sysinfo API varies by version)
        let network_in = 0u64; // Placeholder
        let network_out = 0u64; // Placeholder
        
        Ok(ServerMetrics {
            uptime_seconds: uptime,
            cpu_usage_percent: cpu_usage,
            memory_usage_bytes: memory_used * 1024, // Convert from KB to bytes
            memory_available_bytes: memory_total * 1024,
            disk_usage_bytes: disk_used,
            disk_available_bytes: disk_available,
            network_bytes_in: network_in,
            network_bytes_out: network_out,
            open_connections: 0, // TODO: Implement actual connection counting
            active_requests: 0,  // TODO: Implement actual request counting
        })
    }
    
    /// Collect storage-level metrics (placeholder implementation)
    async fn collect_storage_metrics(&self) -> Result<StorageMetrics> {
        // TODO: Integrate with actual storage components
        Ok(StorageMetrics::default())
    }
    
    /// Collect index-level metrics (placeholder implementation)
    async fn collect_index_metrics(&self) -> Result<IndexMetrics> {
        // TODO: Integrate with AXIS and other indexing components
        Ok(IndexMetrics::default())
    }
    
    /// Collect query-level metrics (placeholder implementation)
    async fn collect_query_metrics(&self) -> Result<QueryMetrics> {
        // TODO: Integrate with query engine
        Ok(QueryMetrics::default())
    }
    
    /// Process alerts and manage alert lifecycle
    async fn process_alerts(&self, new_alerts: Vec<Alert>) -> Result<()> {
        let mut active_alerts = self.active_alerts.write().await;
        
        for alert in new_alerts {
            // Check if this alert already exists
            let existing = active_alerts.iter_mut()
                .find(|a| a.metric_name == alert.metric_name && !a.acknowledged);
                
            match existing {
                Some(existing_alert) => {
                    // Update existing alert
                    existing_alert.current_value = alert.current_value;
                    existing_alert.timestamp = alert.timestamp;
                }
                None => {
                    // New alert
                    let _ = self.event_sender.send(MetricsEvent::AlertGenerated(alert.clone()));
                    active_alerts.push(alert);
                }
            }
        }
        
        // Check for resolved alerts (current metrics below threshold)
        let current_metrics = self.current_metrics.read().await;
        let mut resolved_alerts = Vec::new();
        
        active_alerts.retain(|alert| {
            let is_resolved = match alert.metric_name.as_str() {
                "cpu_usage_percent" => {
                    current_metrics.server.cpu_usage_percent <= alert.threshold_value
                }
                "memory_usage_percent" => {
                    let memory_percent = if current_metrics.server.memory_available_bytes > 0 {
                        (current_metrics.server.memory_usage_bytes as f64 / 
                         current_metrics.server.memory_available_bytes as f64) * 100.0
                    } else { 0.0 };
                    memory_percent <= alert.threshold_value
                }
                "p99_query_latency_ms" => {
                    current_metrics.query.p99_query_latency_ms <= alert.threshold_value
                }
                "cache_hit_rate" => {
                    current_metrics.storage.cache_hit_rate >= alert.threshold_value
                }
                _ => false
            };
            
            if is_resolved {
                resolved_alerts.push(alert.id.clone());
                false // Remove from active alerts
            } else {
                true // Keep in active alerts
            }
        });
        
        // Emit resolved alert events
        for alert_id in resolved_alerts {
            let _ = self.event_sender.send(MetricsEvent::AlertResolved(alert_id));
        }
        
        Ok(())
    }
    
    /// Clean up old metrics from history
    async fn cleanup_old_metrics(&self) {
        let mut history = self.metrics_history.write().await;
        let retention_duration = Duration::from_secs(self.config.retention_hours * 3600);
        let cutoff_time = Utc::now() - chrono::Duration::from_std(retention_duration).unwrap_or_default();
        
        history.retain(|metrics| metrics.timestamp > cutoff_time);
        
        println!("Cleaned up metrics history, {} samples remaining", history.len());
    }
    
    /// Get current metrics snapshot
    pub async fn get_current_metrics(&self) -> SystemMetrics {
        self.current_metrics.read().await.clone()
    }
    
    /// Get metrics history for a time range
    pub async fn get_metrics_history(&self, since: Option<DateTime<Utc>>) -> Vec<SystemMetrics> {
        let history = self.metrics_history.read().await;
        
        match since {
            Some(since_time) => {
                history.iter()
                    .filter(|metrics| metrics.timestamp > since_time)
                    .cloned()
                    .collect()
            }
            None => history.clone()
        }
    }
    
    /// Get active alerts
    pub async fn get_active_alerts(&self) -> Vec<Alert> {
        self.active_alerts.read().await.clone()
    }
    
    /// Acknowledge an alert
    pub async fn acknowledge_alert(&self, alert_id: &str) -> Result<bool> {
        let mut alerts = self.active_alerts.write().await;
        
        if let Some(alert) = alerts.iter_mut().find(|a| a.id == alert_id) {
            alert.acknowledged = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    
    /// Add custom metrics from application components
    pub async fn add_custom_metrics(&self, metrics: HashMap<String, f64>) -> Result<()> {
        let mut current = self.current_metrics.write().await;
        current.custom.extend(metrics);
        Ok(())
    }
    
    /// Get metrics summary for dashboard
    pub async fn get_metrics_summary(&self) -> MetricsSummary {
        let metrics = self.current_metrics.read().await;
        let alerts = self.active_alerts.read().await;
        
        MetricsSummary {
            timestamp: metrics.timestamp,
            system_health: self.calculate_system_health(&metrics).await,
            cpu_usage: metrics.server.cpu_usage_percent,
            memory_usage_percent: if metrics.server.memory_available_bytes > 0 {
                (metrics.server.memory_usage_bytes as f64 / metrics.server.memory_available_bytes as f64) * 100.0
            } else { 0.0 },
            disk_usage_percent: if metrics.server.disk_available_bytes > 0 {
                (metrics.server.disk_usage_bytes as f64 / 
                 (metrics.server.disk_usage_bytes + metrics.server.disk_available_bytes) as f64) * 100.0
            } else { 0.0 },
            query_latency_p99: metrics.query.p99_query_latency_ms,
            queries_per_second: metrics.query.queries_per_second,
            cache_hit_rate: metrics.storage.cache_hit_rate,
            active_alerts_count: alerts.len(),
            critical_alerts_count: alerts.iter()
                .filter(|a| matches!(a.level, super::AlertLevel::Critical))
                .count(),
        }
    }
    
    /// Calculate overall system health score (0.0 to 1.0)
    async fn calculate_system_health(&self, metrics: &SystemMetrics) -> f64 {
        let mut health_factors = Vec::new();
        
        // CPU health (inverse of usage)
        health_factors.push((100.0 - metrics.server.cpu_usage_percent) / 100.0);
        
        // Memory health
        let memory_usage_percent = if metrics.server.memory_available_bytes > 0 {
            (metrics.server.memory_usage_bytes as f64 / metrics.server.memory_available_bytes as f64) * 100.0
        } else { 0.0 };
        health_factors.push((100.0 - memory_usage_percent) / 100.0);
        
        // Query performance health
        let latency_health = if metrics.query.p99_query_latency_ms > 0.0 {
            (100.0 / (1.0 + metrics.query.p99_query_latency_ms)).min(1.0)
        } else { 1.0 };
        health_factors.push(latency_health);
        
        // Cache performance health
        health_factors.push(metrics.storage.cache_hit_rate);
        
        // Calculate weighted average
        health_factors.iter().sum::<f64>() / health_factors.len() as f64
    }
}

/// Metrics summary for dashboard display
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSummary {
    pub timestamp: DateTime<Utc>,
    pub system_health: f64, // 0.0 to 1.0
    pub cpu_usage: f64,
    pub memory_usage_percent: f64,
    pub disk_usage_percent: f64,
    pub query_latency_p99: f64,
    pub queries_per_second: f64,
    pub cache_hit_rate: f64,
    pub active_alerts_count: usize,
    pub critical_alerts_count: usize,
}
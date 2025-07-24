// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::monitoring::metrics::{Alert, AlertLevel, AlertThresholds, MetricsConfig, SystemMetrics};
    use chrono::Utc;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use tokio::time::timeout;

    /// Create test metrics config
    fn test_config() -> MetricsConfig {
        MetricsConfig {
            collection_interval_seconds: 1,
            retention_hours: 1,
            enable_prometheus: true,
            prometheus_port: 9090,
            enable_detailed_logging: false,
            alert_thresholds: AlertThresholds {
                max_cpu_usage_percent: 80.0,
                max_memory_usage_percent: 90.0,
                max_disk_usage_percent: 90.0,
                max_query_latency_ms: 100.0,
                min_cache_hit_rate: 0.5,
                max_error_rate: 0.05,
            },
            histogram_buckets: vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0],
        }
    }

    #[tokio::test]
    async fn test_metrics_collector_creation() {
        let config = test_config();
        let (collector, _rx) = MetricsCollector::new(config).unwrap();
        
        // Verify initial state
        let metrics = collector.get_current_metrics().await;
        assert_eq!(metrics.server.cpu_usage_percent, 0.0);
        assert_eq!(metrics.server.memory_usage_bytes, 0);
        
        let alerts = collector.get_active_alerts().await;
        assert!(alerts.is_empty());
    }

    #[tokio::test]
    async fn test_connection_tracking() {
        let config = test_config();
        let (collector, _rx) = MetricsCollector::new(config).unwrap();
        
        // Test connection counting
        collector.increment_connections();
        collector.increment_connections();
        collector.increment_connections();
        
        // Force metrics collection by resetting rate limiter
        *collector.rate_limiter.last_emit.write().await = Instant::now() - Duration::from_secs(60);
        let _ = collector.collect_all_metrics().await;
        
        let metrics = collector.get_current_metrics().await;
        assert_eq!(metrics.server.open_connections, 3);
        
        collector.decrement_connections();
        
        // Force metrics collection again
        *collector.rate_limiter.last_emit.write().await = Instant::now() - Duration::from_secs(60);
        let _ = collector.collect_all_metrics().await;
        
        let metrics = collector.get_current_metrics().await;
        assert_eq!(metrics.server.open_connections, 2);
    }

    #[tokio::test]
    async fn test_request_tracking() {
        let config = test_config();
        let (collector, _rx) = MetricsCollector::new(config).unwrap();
        
        // Test request counting
        for _ in 0..5 {
            collector.increment_requests();
        }
        
        // Force metrics collection
        *collector.rate_limiter.last_emit.write().await = Instant::now() - Duration::from_secs(60);
        let _ = collector.collect_all_metrics().await;
        let metrics = collector.get_current_metrics().await;
        assert_eq!(metrics.server.active_requests, 5);
        
        for _ in 0..3 {
            collector.decrement_requests();
        }
        
        // Force metrics collection again
        *collector.rate_limiter.last_emit.write().await = Instant::now() - Duration::from_secs(60);
        let _ = collector.collect_all_metrics().await;
        let metrics = collector.get_current_metrics().await;
        assert_eq!(metrics.server.active_requests, 2);
    }

    #[tokio::test]
    async fn test_metrics_history() {
        let mut config = test_config();
        config.collection_interval_seconds = 1;
        
        let (collector, _rx) = MetricsCollector::new(config).unwrap();
        
        // Collect metrics multiple times
        for i in 0..3 {
            // Force rate limiter to allow collection
            *collector.rate_limiter.last_emit.write().await = Instant::now() - Duration::from_secs(60);
            let _ = collector.collect_all_metrics().await;
            if i < 2 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        
        let history = collector.get_metrics_history(None).await;
        assert_eq!(history.len(), 3);
        
        // Test filtering by time
        let since = history[1].timestamp;
        let filtered = collector.get_metrics_history(Some(since)).await;
        assert_eq!(filtered.len(), 1); // Only the last metric
    }

    #[tokio::test]
    async fn test_alert_generation() {
        let mut config = test_config();
        config.alert_thresholds.max_cpu_usage_percent = 50.0;
        
        let (collector, mut rx) = MetricsCollector::new(config).unwrap();
        
        // Manually set high CPU usage
        let mut metrics = collector.get_current_metrics().await;
        metrics.server.cpu_usage_percent = 75.0;
        *collector.current_metrics.write().await = metrics;
        
        // Process alerts
        let alerts = vec![Alert {
            id: "alert_1".to_string(),
            metric_name: "cpu_usage_percent".to_string(),
            threshold_value: 50.0,
            current_value: 75.0,
            level: AlertLevel::Warning,
            message: "CPU usage high".to_string(),
            timestamp: Utc::now(),
            acknowledged: false,
        }];
        
        let _ = collector.process_alerts(alerts).await;
        
        // Check alert was generated
        let active_alerts = collector.get_active_alerts().await;
        assert_eq!(active_alerts.len(), 1);
        assert_eq!(active_alerts[0].metric_name, "cpu_usage_percent");
        
        // Check event was emitted
        if let Ok(Some(event)) = timeout(Duration::from_secs(1), rx.recv()).await {
            match event {
                MetricsEvent::AlertGenerated(alert) => {
                    assert_eq!(alert.metric_name, "cpu_usage_percent");
                }
                _ => panic!("Expected AlertGenerated event"),
            }
        } else {
            panic!("No event received");
        }
    }

    #[tokio::test]
    async fn test_alert_resolution() {
        let config = test_config();
        let (collector, mut rx) = MetricsCollector::new(config).unwrap();
        
        // Add an active alert
        let alert = Alert {
            id: "alert_1".to_string(),
            metric_name: "cpu_usage_percent".to_string(),
            threshold_value: 80.0,
            current_value: 85.0,
            level: AlertLevel::Warning,
            message: "CPU usage high".to_string(),
            timestamp: Utc::now(),
            acknowledged: false,
        };
        
        collector.active_alerts.write().await.push(alert);
        
        // Set CPU usage below threshold
        let mut metrics = collector.get_current_metrics().await;
        metrics.server.cpu_usage_percent = 50.0;
        *collector.current_metrics.write().await = metrics;
        
        // Process alerts (empty new alerts)
        let _ = collector.process_alerts(vec![]).await;
        
        // Alert should be resolved
        let active_alerts = collector.get_active_alerts().await;
        assert!(active_alerts.is_empty());
        
        // Check resolution event
        if let Ok(Some(event)) = timeout(Duration::from_secs(1), rx.recv()).await {
            match event {
                MetricsEvent::AlertResolved(id) => {
                    assert_eq!(id, "alert_1");
                }
                _ => panic!("Expected AlertResolved event"),
            }
        }
    }

    #[tokio::test]
    async fn test_alert_acknowledgment() {
        let config = test_config();
        let (collector, _rx) = MetricsCollector::new(config).unwrap();
        
        // Add an alert
        let alert = Alert {
            id: "alert_1".to_string(),
            metric_name: "memory_usage_percent".to_string(),
            threshold_value: 90.0,
            current_value: 95.0,
            level: AlertLevel::Critical,
            message: "Memory usage critical".to_string(),
            timestamp: Utc::now(),
            acknowledged: false,
        };
        
        collector.active_alerts.write().await.push(alert);
        
        // Acknowledge the alert
        let ack_result = collector.acknowledge_alert("alert_1").await.unwrap();
        assert!(ack_result);
        
        // Verify acknowledgment
        let alerts = collector.get_active_alerts().await;
        assert!(alerts[0].acknowledged);
        
        // Test non-existent alert
        let ack_result = collector.acknowledge_alert("invalid_id").await.unwrap();
        assert!(!ack_result);
    }

    #[tokio::test]
    async fn test_custom_metrics() {
        let config = test_config();
        let (collector, _rx) = MetricsCollector::new(config).unwrap();
        
        // Add custom metrics
        let mut custom = HashMap::new();
        custom.insert("custom_metric_1".to_string(), 42.0);
        custom.insert("custom_metric_2".to_string(), 99.5);
        
        collector.add_custom_metrics(custom).await.unwrap();
        
        // Verify custom metrics were added
        let metrics = collector.get_current_metrics().await;
        assert_eq!(metrics.custom.get("custom_metric_1"), Some(&42.0));
        assert_eq!(metrics.custom.get("custom_metric_2"), Some(&99.5));
    }

    #[tokio::test]
    async fn test_metrics_summary() {
        let config = test_config();
        let (collector, _rx) = MetricsCollector::new(config).unwrap();
        
        // Set up metrics
        let mut metrics = SystemMetrics::new();
        metrics.server.cpu_usage_percent = 45.0;
        metrics.server.memory_usage_bytes = 1024 * 1024 * 1024; // 1GB
        metrics.server.memory_available_bytes = 4 * 1024 * 1024 * 1024; // 4GB
        metrics.query.p99_query_latency_ms = 25.0;
        metrics.query.queries_per_second = 100.0;
        metrics.storage.cache_hit_rate = 0.85;
        
        *collector.current_metrics.write().await = metrics;
        
        // Add some alerts
        collector.active_alerts.write().await.push(Alert {
            id: "alert_1".to_string(),
            metric_name: "test".to_string(),
            threshold_value: 0.0,
            current_value: 0.0,
            level: AlertLevel::Warning,
            message: "Test".to_string(),
            timestamp: Utc::now(),
            acknowledged: false,
        });
        
        collector.active_alerts.write().await.push(Alert {
            id: "alert_2".to_string(),
            metric_name: "test2".to_string(),
            threshold_value: 0.0,
            current_value: 0.0,
            level: AlertLevel::Critical,
            message: "Test2".to_string(),
            timestamp: Utc::now(),
            acknowledged: false,
        });
        
        // Get summary
        let summary = collector.get_metrics_summary().await;
        
        assert_eq!(summary.cpu_usage, 45.0);
        assert_eq!(summary.memory_usage_percent, 25.0); // 1GB/4GB
        assert_eq!(summary.query_latency_p99, 25.0);
        assert_eq!(summary.queries_per_second, 100.0);
        assert_eq!(summary.cache_hit_rate, 0.85);
        assert_eq!(summary.active_alerts_count, 2);
        assert_eq!(summary.critical_alerts_count, 1);
        assert!(summary.system_health > 0.0 && summary.system_health <= 1.0);
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let mut config = test_config();
        config.collection_interval_seconds = 2; // 2 second interval
        
        let (collector, _rx) = MetricsCollector::new(config).unwrap();
        
        // Force first collection
        *collector.rate_limiter.last_emit.write().await = Instant::now() - Duration::from_secs(60);
        let result1 = collector.collect_all_metrics().await;
        assert!(result1.is_ok());
        
        // Immediate second collection should be rate limited (no force)
        let result2 = collector.collect_all_metrics().await;
        assert!(result2.is_ok()); // Still returns Ok, but doesn't actually collect
        
        // Verify metrics weren't updated
        let history = collector.get_metrics_history(None).await;
        assert_eq!(history.len(), 1); // Only one collection happened
        
        // Wait for interval and try again
        tokio::time::sleep(Duration::from_secs(3)).await;
        let result3 = collector.collect_all_metrics().await;
        assert!(result3.is_ok());
        
        // Now we should have 2 collections
        let history = collector.get_metrics_history(None).await;
        assert_eq!(history.len(), 2);
    }

    #[tokio::test]
    async fn test_metrics_cleanup() {
        let mut config = test_config();
        config.retention_hours = 1; // 1 hour retention
        
        let (collector, _rx) = MetricsCollector::new(config).unwrap();
        
        // Clear any existing history
        collector.metrics_history.write().await.clear();
        
        // Add very old metrics (2 hours old, beyond retention)
        let very_old_time = Utc::now() - chrono::Duration::hours(2);
        let mut very_old_metrics = SystemMetrics::new();
        very_old_metrics.timestamp = very_old_time;
        
        // Add old metrics (30 minutes old, within retention)
        let recent_time = Utc::now() - chrono::Duration::minutes(30);
        let mut recent_metrics = SystemMetrics::new();
        recent_metrics.timestamp = recent_time;
        
        // Add both metrics
        collector.metrics_history.write().await.push(very_old_metrics);
        collector.metrics_history.write().await.push(recent_metrics);
        
        // Add current metrics
        *collector.rate_limiter.last_emit.write().await = Instant::now() - Duration::from_secs(60);
        let _ = collector.collect_all_metrics().await;
        
        // Verify we have 3 metrics before cleanup
        assert_eq!(collector.get_metrics_history(None).await.len(), 3);
        
        // Run cleanup
        collector.cleanup_old_metrics().await;
        
        // Should have 2 metrics remaining (recent and current)
        let history = collector.get_metrics_history(None).await;
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|m| m.timestamp > very_old_time));
    }

    #[tokio::test]
    async fn test_collector_lifecycle() {
        let config = test_config();
        let (collector, mut rx) = MetricsCollector::new(config).unwrap();
        
        // Start collector in background
        let collector_clone = collector.clone();
        let handle = tokio::spawn(async move {
            let _ = collector_clone.start().await;
        });
        
        // Let it run briefly
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Stop collector
        collector.stop().await;
        
        // Wait for task to complete
        let _ = timeout(Duration::from_secs(2), handle).await;
        
        // Verify shutdown
        assert!(*collector.shutdown.read().await);
        
        // Close receiver
        rx.close();
    }
}
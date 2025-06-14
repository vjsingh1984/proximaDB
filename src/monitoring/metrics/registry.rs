// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metrics registry for organizing and managing metrics

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;

use super::{Metric, MetricType, MetricValue, Histogram, Summary, HistogramBucket};

/// Central registry for managing metrics
pub struct MetricsRegistry {
    /// Registered metrics by name
    metrics: Arc<RwLock<HashMap<String, Metric>>>,
    
    /// Metric collectors (functions that provide metric values)
    collectors: Arc<RwLock<HashMap<String, Box<dyn MetricCollectorFn>>>>,
    
    /// Histogram configurations
    histogram_configs: Arc<RwLock<HashMap<String, HistogramConfig>>>,
}

/// Configuration for histogram metrics
#[derive(Debug, Clone)]
pub struct HistogramConfig {
    pub buckets: Vec<f64>,
    pub labels: Vec<String>,
}

/// Trait for metric collector functions
pub trait MetricCollectorFn: Send + Sync {
    fn collect(&self) -> Result<Vec<MetricValue>>;
}

/// Counter metric implementation
pub struct Counter {
    name: String,
    help: String,
    value: Arc<RwLock<f64>>,
    labels: HashMap<String, String>,
}

/// Gauge metric implementation  
pub struct Gauge {
    name: String,
    help: String,
    value: Arc<RwLock<f64>>,
    labels: HashMap<String, String>,
}

/// Histogram metric implementation
pub struct HistogramMetric {
    name: String,
    help: String,
    config: HistogramConfig,
    buckets: Arc<RwLock<Vec<Arc<RwLock<u64>>>>>,
    sum: Arc<RwLock<f64>>,
    count: Arc<RwLock<u64>>,
    labels: HashMap<String, String>,
}

/// Summary metric implementation
pub struct SummaryMetric {
    name: String,
    help: String,
    sum: Arc<RwLock<f64>>,
    count: Arc<RwLock<u64>>,
    quantiles: Arc<RwLock<Vec<f64>>>, // Sorted observations for quantile calculation
    labels: HashMap<String, String>,
}

impl MetricsRegistry {
    /// Create new metrics registry
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
            collectors: Arc::new(RwLock::new(HashMap::new())),
            histogram_configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Register a new counter metric
    pub async fn register_counter(&self, name: &str, help: &str, labels: HashMap<String, String>) -> Result<Arc<Counter>> {
        let counter = Arc::new(Counter {
            name: name.to_string(),
            help: help.to_string(),
            value: Arc::new(RwLock::new(0.0)),
            labels,
        });
        
        // Register the metric
        let mut metrics = self.metrics.write().await;
        metrics.insert(name.to_string(), Metric {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Counter,
            values: Vec::new(),
        });
        
        Ok(counter)
    }
    
    /// Register a new gauge metric
    pub async fn register_gauge(&self, name: &str, help: &str, labels: HashMap<String, String>) -> Result<Arc<Gauge>> {
        let gauge = Arc::new(Gauge {
            name: name.to_string(),
            help: help.to_string(),
            value: Arc::new(RwLock::new(0.0)),
            labels,
        });
        
        // Register the metric
        let mut metrics = self.metrics.write().await;
        metrics.insert(name.to_string(), Metric {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Gauge,
            values: Vec::new(),
        });
        
        Ok(gauge)
    }
    
    /// Register a new histogram metric
    pub async fn register_histogram(
        &self, 
        name: &str, 
        help: &str, 
        buckets: Vec<f64>,
        labels: HashMap<String, String>
    ) -> Result<Arc<HistogramMetric>> {
        let config = HistogramConfig {
            buckets: buckets.clone(),
            labels: labels.keys().cloned().collect(),
        };
        
        let bucket_counters: Vec<Arc<RwLock<u64>>> = buckets.iter()
            .map(|_| Arc::new(RwLock::new(0)))
            .collect();
        
        let histogram = Arc::new(HistogramMetric {
            name: name.to_string(),
            help: help.to_string(),
            config: config.clone(),
            buckets: Arc::new(RwLock::new(bucket_counters)),
            sum: Arc::new(RwLock::new(0.0)),
            count: Arc::new(RwLock::new(0)),
            labels,
        });
        
        // Register the metric
        let mut metrics = self.metrics.write().await;
        metrics.insert(name.to_string(), Metric {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Histogram,
            values: Vec::new(),
        });
        
        // Store histogram config
        let mut configs = self.histogram_configs.write().await;
        configs.insert(name.to_string(), config);
        
        Ok(histogram)
    }
    
    /// Register a new summary metric
    pub async fn register_summary(&self, name: &str, help: &str, labels: HashMap<String, String>) -> Result<Arc<SummaryMetric>> {
        let summary = Arc::new(SummaryMetric {
            name: name.to_string(),
            help: help.to_string(),
            sum: Arc::new(RwLock::new(0.0)),
            count: Arc::new(RwLock::new(0)),
            quantiles: Arc::new(RwLock::new(Vec::new())),
            labels,
        });
        
        // Register the metric
        let mut metrics = self.metrics.write().await;
        metrics.insert(name.to_string(), Metric {
            name: name.to_string(),
            help: help.to_string(),
            metric_type: MetricType::Summary,
            values: Vec::new(),
        });
        
        Ok(summary)
    }
    
    /// Collect all metric values
    pub async fn collect_all(&self) -> Result<Vec<Metric>> {
        let metrics = self.metrics.read().await;
        let collectors = self.collectors.read().await;
        
        let mut result = Vec::new();
        
        for (name, metric) in metrics.iter() {
            let mut metric_copy = metric.clone();
            
            // Get values from collector if available
            if let Some(collector) = collectors.get(name) {
                match collector.collect() {
                    Ok(values) => metric_copy.values = values,
                    Err(e) => eprintln!("Error collecting metric {}: {}", name, e),
                }
            }
            
            result.push(metric_copy);
        }
        
        Ok(result)
    }
    
    /// Get specific metric by name
    pub async fn get_metric(&self, name: &str) -> Option<Metric> {
        let metrics = self.metrics.read().await;
        metrics.get(name).cloned()
    }
    
    /// Remove a metric from registry
    pub async fn unregister(&self, name: &str) -> bool {
        let mut metrics = self.metrics.write().await;
        let mut collectors = self.collectors.write().await;
        let mut configs = self.histogram_configs.write().await;
        
        let removed_metric = metrics.remove(name).is_some();
        let removed_collector = collectors.remove(name).is_some();
        let removed_config = configs.remove(name).is_some();
        
        removed_metric || removed_collector || removed_config
    }
    
    /// Get all registered metric names
    pub async fn list_metrics(&self) -> Vec<String> {
        let metrics = self.metrics.read().await;
        metrics.keys().cloned().collect()
    }
    
    /// Clear all metrics
    pub async fn clear(&self) {
        let mut metrics = self.metrics.write().await;
        let mut collectors = self.collectors.write().await;
        let mut configs = self.histogram_configs.write().await;
        
        metrics.clear();
        collectors.clear();
        configs.clear();
    }
}

impl Counter {
    /// Increment counter by 1
    pub async fn inc(&self) {
        self.add(1.0).await;
    }
    
    /// Add value to counter
    pub async fn add(&self, value: f64) {
        if value >= 0.0 {
            let mut val = self.value.write().await;
            *val += value;
        }
    }
    
    /// Get current counter value
    pub async fn get(&self) -> f64 {
        *self.value.read().await
    }
    
    /// Reset counter to zero
    pub async fn reset(&self) {
        let mut val = self.value.write().await;
        *val = 0.0;
    }
}

impl Gauge {
    /// Set gauge value
    pub async fn set(&self, value: f64) {
        let mut val = self.value.write().await;
        *val = value;
    }
    
    /// Increment gauge by value
    pub async fn add(&self, value: f64) {
        let mut val = self.value.write().await;
        *val += value;
    }
    
    /// Decrement gauge by value
    pub async fn sub(&self, value: f64) {
        let mut val = self.value.write().await;
        *val -= value;
    }
    
    /// Increment gauge by 1
    pub async fn inc(&self) {
        self.add(1.0).await;
    }
    
    /// Decrement gauge by 1
    pub async fn dec(&self) {
        self.sub(1.0).await;
    }
    
    /// Get current gauge value
    pub async fn get(&self) -> f64 {
        *self.value.read().await
    }
}

impl HistogramMetric {
    /// Observe a value in the histogram
    pub async fn observe(&self, value: f64) {
        // Update sum and count
        {
            let mut sum = self.sum.write().await;
            *sum += value;
        }
        {
            let mut count = self.count.write().await;
            *count += 1;
        }
        
        // Update buckets
        let buckets = self.buckets.read().await;
        for (i, &upper_bound) in self.config.buckets.iter().enumerate() {
            if value <= upper_bound {
                if let Some(bucket) = buckets.get(i) {
                    let mut bucket_count = bucket.write().await;
                    *bucket_count += 1;
                }
            }
        }
    }
    
    /// Get histogram data
    pub async fn get(&self) -> Histogram {
        let sum = *self.sum.read().await;
        let count = *self.count.read().await;
        
        let buckets = self.buckets.read().await;
        let histogram_buckets: Vec<HistogramBucket> = self.config.buckets.iter()
            .enumerate()
            .map(|(i, &upper_bound)| {
                let bucket_count = if let Some(bucket) = buckets.get(i) {
                    futures::executor::block_on(async {
                        *bucket.read().await
                    })
                } else {
                    0
                };
                
                HistogramBucket {
                    upper_bound,
                    count: bucket_count,
                }
            })
            .collect();
        
        Histogram {
            sum,
            count,
            buckets: histogram_buckets,
        }
    }
    
    /// Reset histogram
    pub async fn reset(&self) {
        {
            let mut sum = self.sum.write().await;
            *sum = 0.0;
        }
        {
            let mut count = self.count.write().await;
            *count = 0;
        }
        
        let buckets = self.buckets.read().await;
        for bucket in buckets.iter() {
            let mut bucket_count = bucket.write().await;
            *bucket_count = 0;
        }
    }
}

impl SummaryMetric {
    /// Observe a value in the summary
    pub async fn observe(&self, value: f64) {
        // Update sum and count
        {
            let mut sum = self.sum.write().await;
            *sum += value;
        }
        {
            let mut count = self.count.write().await;
            *count += 1;
        }
        
        // Add to quantiles (keep sorted)
        let mut quantiles = self.quantiles.write().await;
        quantiles.push(value);
        quantiles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        
        // Limit the number of observations to prevent memory growth
        if quantiles.len() > 10000 {
            quantiles.drain(0..1000); // Remove oldest 1000 observations
        }
    }
    
    /// Get summary data with calculated quantiles
    pub async fn get(&self) -> Summary {
        let sum = *self.sum.read().await;
        let count = *self.count.read().await;
        
        let quantiles_vec = self.quantiles.read().await;
        let mut quantiles_vec_result = Vec::new();
        
        if !quantiles_vec.is_empty() {
            // Calculate common quantiles
            let percentiles = vec![0.5, 0.9, 0.95, 0.99];
            for percentile in percentiles {
                let index = ((quantiles_vec.len() as f64 - 1.0) * percentile) as usize;
                let value = quantiles_vec.get(index).copied().unwrap_or(0.0);
                quantiles_vec_result.push((percentile, value));
            }
        }
        
        Summary {
            sum,
            count,
            quantiles: quantiles_vec_result,
        }
    }
    
    /// Reset summary
    pub async fn reset(&self) {
        {
            let mut sum = self.sum.write().await;
            *sum = 0.0;
        }
        {
            let mut count = self.count.write().await;
            *count = 0;
        }
        {
            let mut quantiles = self.quantiles.write().await;
            quantiles.clear();
        }
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}
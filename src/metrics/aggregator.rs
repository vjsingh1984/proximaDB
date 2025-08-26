// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metrics aggregation for time-series analysis

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::schema::CollectionMetrics;

/// Time window for aggregation
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum AggregationWindow {
    Minute,
    FiveMinutes,
    Hour,
    Day,
    Week,
}

impl AggregationWindow {
    /// Get window duration in seconds
    pub fn duration_seconds(&self) -> i64 {
        match self {
            Self::Minute => 60,
            Self::FiveMinutes => 300,
            Self::Hour => 3600,
            Self::Day => 86400,
            Self::Week => 604800,
        }
    }
}

/// Aggregated metrics over a time window
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedMetrics {
    pub window: AggregationWindow,
    pub start_time: i64,
    pub end_time: i64,
    pub collection_id: String,
    
    // Aggregated values
    pub total_operations: i64,
    pub avg_latency_us: f64,
    pub max_latency_us: f64,
    pub min_latency_us: f64,
    pub total_bytes_written: i64,
    pub total_bytes_read: i64,
    pub error_count: i64,
    pub success_rate: f32,
}

/// Metrics aggregation engine for time-series analysis
pub struct MetricsAggregationEngine {
    /// Time-series data points
    time_series: HashMap<String, Vec<CollectionMetrics>>,
}

impl MetricsAggregationEngine {
    /// Create a new aggregator
    pub fn new() -> Self {
        Self {
            time_series: HashMap::new(),
        }
    }
    
    /// Add a data point
    pub fn add_data_point(&mut self, collection_id: String, metrics: CollectionMetrics) {
        self.time_series
            .entry(collection_id)
            .or_insert_with(Vec::new)
            .push(metrics);
    }
    
    /// Aggregate metrics over a time window
    pub fn aggregate(
        &self,
        collection_id: &str,
        window: AggregationWindow,
        start_time: i64,
        end_time: i64,
    ) -> Result<AggregatedMetrics> {
        let data_points = self.time_series
            .get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No data for collection {}", collection_id))?;
        
        let filtered: Vec<&CollectionMetrics> = data_points
            .iter()
            .filter(|m| m.updated_at >= start_time && m.updated_at <= end_time)
            .collect();
        
        if filtered.is_empty() {
            return Err(anyhow::anyhow!("No data points in the specified time range"));
        }
        
        // Calculate aggregates
        let total_operations: i64 = filtered.iter()
            .map(|m| m.total_inserts + m.total_updates + m.total_deletes + m.total_searches)
            .sum();
        
        let latencies: Vec<f64> = filtered.iter()
            .flat_map(|m| vec![m.avg_insert_latency_us, m.avg_search_latency_us])
            .filter(|&l| l > 0.0)
            .collect();
        
        let avg_latency = if !latencies.is_empty() {
            latencies.iter().sum::<f64>() / latencies.len() as f64
        } else {
            0.0
        };
        
        let max_latency = latencies.iter().cloned().fold(0.0, f64::max);
        let min_latency = if !latencies.is_empty() {
            latencies.iter().cloned().fold(f64::MAX, f64::min)
        } else {
            0.0
        };
        
        let max_bytes = filtered.iter()
            .map(|m| m.data_size_bytes)
            .max()
            .unwrap_or(0);
            
        let min_bytes = filtered.iter()
            .map(|m| m.data_size_bytes)
            .min()
            .unwrap_or(0);
            
        let total_bytes_written: i64 = max_bytes - min_bytes;
        
        Ok(AggregatedMetrics {
            window,
            start_time,
            end_time,
            collection_id: collection_id.to_string(),
            total_operations,
            avg_latency_us: avg_latency,
            max_latency_us: max_latency,
            min_latency_us: min_latency,
            total_bytes_written,
            total_bytes_read: 0, // TODO: Track read bytes
            error_count: 0, // TODO: Track errors
            success_rate: 1.0, // TODO: Calculate from success/failure counts
        })
    }
    
    /// Generate trend analysis
    pub fn analyze_trends(
        &self,
        collection_id: &str,
        metric_name: &str,
    ) -> Result<TrendAnalysis> {
        let data_points = self.time_series
            .get(metric_name)
            .ok_or_else(|| anyhow::anyhow!("No data for collection {}", collection_id))?;
        
        if data_points.len() < 2 {
            return Err(anyhow::anyhow!("Insufficient data points for trend analysis"));
        }
        
        // Extract metric values based on name
        let values: Vec<f64> = data_points.iter().map(|m| {
            match metric_name {
                "vector_count" => m.vector_count as f64,
                "avg_search_latency" => m.avg_search_latency_us,
                "cache_hit_ratio" => m.cache_hit_ratio as f64,
                "data_size_bytes" => m.data_size_bytes as f64,
                _ => 0.0,
            }
        }).collect();
        
        // Simple linear regression for trend
        let n = values.len() as f64;
        let x_mean = (n - 1.0) / 2.0;
        let y_mean = values.iter().sum::<f64>() / n;
        
        let mut numerator = 0.0;
        let mut denominator = 0.0;
        
        for (i, &y) in values.iter().enumerate() {
            let x = i as f64;
            numerator += (x - x_mean) * (y - y_mean);
            denominator += (x - x_mean) * (x - x_mean);
        }
        
        let slope = if denominator != 0.0 {
            numerator / denominator
        } else {
            0.0
        };
        
        let trend = if slope > 0.01 {
            Trend::Increasing
        } else if slope < -0.01 {
            Trend::Decreasing
        } else {
            Trend::Stable
        };
        
        Ok(TrendAnalysis {
            metric_name: metric_name.to_string(),
            trend,
            slope,
            current_value: values.last().cloned().unwrap_or(0.0),
            predicted_next: values.last().cloned().unwrap_or(0.0) + slope,
        })
    }
}

/// Trend direction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Trend {
    Increasing,
    Decreasing,
    Stable,
}

/// Trend analysis result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendAnalysis {
    pub metric_name: String,
    pub trend: Trend,
    pub slope: f64,
    pub current_value: f64,
    pub predicted_next: f64,
}
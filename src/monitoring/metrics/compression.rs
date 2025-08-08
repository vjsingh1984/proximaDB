// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Compression metrics tracking for SST and VIPER engines
//! 
//! Tracks compression ratios, performance, and storage savings across different
//! compression strategies and engines.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use dashmap::DashMap;
use std::time::{Duration, Instant};

/// Compression metrics for a single collection
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompressionMetrics {
    pub collection_id: String,
    pub engine_type: String, // "sst" or "viper"
    
    // === Overall Compression Statistics ===
    pub total_uncompressed_bytes: u64,
    pub total_compressed_bytes: u64,
    pub overall_compression_ratio: f32, // uncompressed/compressed
    pub space_saved_bytes: i64,
    pub space_saved_percent: f32,
    
    // === SST Block Compression ===
    pub sst_blocks_written: u64,
    pub sst_avg_block_size_bytes: u64,
    pub sst_avg_compression_ratio: f32,
    pub sst_best_compression_ratio: f32,
    pub sst_worst_compression_ratio: f32,
    pub sst_compression_algorithm: String, // "zstd", "lz4", etc.
    pub sst_compression_level: i32,
    
    // === VIPER Column Compression ===
    pub viper_fp32_column_bytes: u64,
    pub viper_fp32_compressed_bytes: u64,
    pub viper_fp32_compression_ratio: f32,
    pub viper_quantized_column_bytes: u64,
    pub viper_quantized_type: Option<String>, // "int8", "pq8", "pq4"
    pub viper_quantization_reduction: f32, // % reduction from FP32
    pub viper_normalization_method: Option<String>, // "mean", "trimmed_mean", "median"
    
    // === Performance Metrics ===
    pub avg_compression_time_ms: f64,
    pub avg_decompression_time_ms: f64,
    pub total_compression_operations: u64,
    pub total_decompression_operations: u64,
    pub compression_throughput_mb_per_sec: f64,
    pub decompression_throughput_mb_per_sec: f64,
    
    // === Block Size Distribution (SST) ===
    pub block_size_distribution: BlockSizeDistribution,
    
    // === Compression Effectiveness by Data Type ===
    pub vector_compression_ratio: f32,
    pub metadata_compression_ratio: f32,
    pub index_compression_ratio: f32,
    
    // === Adaptive Compression Stats ===
    pub adaptive_adjustments: u64, // Number of times compression was adjusted
    pub current_adaptive_level: Option<i32>,
    pub last_adjustment_reason: Option<String>,
    
    // === Timestamps ===
    pub first_compression_at: Option<i64>,
    pub last_compression_at: Option<i64>,
    pub last_updated: i64,
}

/// Distribution of block sizes for analysis
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BlockSizeDistribution {
    pub min_bytes: u64,
    pub max_bytes: u64,
    pub median_bytes: u64,
    pub p95_bytes: u64,
    pub p99_bytes: u64,
    pub buckets: HashMap<String, u64>, // e.g., "0-4MB": count, "4-8MB": count
}

/// Compression operation result for metrics tracking
#[derive(Debug, Clone)]
pub struct CompressionResult {
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub compression_time: Duration,
    pub algorithm: String,
    pub level: i32,
    pub data_type: CompressionDataType,
}

/// Decompression operation result
#[derive(Debug, Clone)]
pub struct DecompressionResult {
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub decompression_time: Duration,
    pub algorithm: String,
}

/// Type of data being compressed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionDataType {
    Vector,
    Metadata,
    Index,
    Mixed,
}

/// Compression metrics tracker with thread-safe updates
pub struct CompressionMetricsTracker {
    /// Per-collection metrics
    collection_metrics: Arc<DashMap<String, CompressionMetrics>>,
    
    /// Global compression stats
    global_stats: Arc<RwLock<GlobalCompressionStats>>,
}

/// Global compression statistics across all collections
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct GlobalCompressionStats {
    total_space_saved_bytes: i64,
    total_compression_operations: u64,
    total_decompression_operations: u64,
    avg_compression_ratio: f32,
}

use tokio::sync::RwLock;

impl CompressionMetricsTracker {
    /// Create a new compression metrics tracker
    pub fn new() -> Self {
        Self {
            collection_metrics: Arc::new(DashMap::new()),
            global_stats: Arc::new(RwLock::new(GlobalCompressionStats::default())),
        }
    }
    
    /// Record a compression operation
    pub async fn record_compression(
        &self,
        collection_id: &str,
        engine_type: &str,
        result: CompressionResult,
    ) {
        let mut metrics = self.collection_metrics
            .entry(collection_id.to_string())
            .or_insert_with(|| CompressionMetrics {
                collection_id: collection_id.to_string(),
                engine_type: engine_type.to_string(),
                last_updated: chrono::Utc::now().timestamp(),
                ..Default::default()
            });
        
        // Update compression stats
        metrics.total_uncompressed_bytes += result.uncompressed_size;
        metrics.total_compressed_bytes += result.compressed_size;
        metrics.total_compression_operations += 1;
        
        // Update compression ratio
        if metrics.total_uncompressed_bytes > 0 {
            metrics.overall_compression_ratio = 
                metrics.total_uncompressed_bytes as f32 / metrics.total_compressed_bytes as f32;
        }
        
        // Update space saved
        metrics.space_saved_bytes = 
            metrics.total_uncompressed_bytes as i64 - metrics.total_compressed_bytes as i64;
        metrics.space_saved_percent = if metrics.total_uncompressed_bytes > 0 {
            (metrics.space_saved_bytes as f32 / metrics.total_uncompressed_bytes as f32) * 100.0
        } else {
            0.0
        };
        
        // Update timing
        let time_ms = result.compression_time.as_millis() as f64;
        metrics.avg_compression_time_ms = 
            (metrics.avg_compression_time_ms * (metrics.total_compression_operations - 1) as f64 + time_ms) 
            / metrics.total_compression_operations as f64;
        
        // Update throughput
        if time_ms > 0.0 {
            let mb_compressed = result.uncompressed_size as f64 / (1024.0 * 1024.0);
            metrics.compression_throughput_mb_per_sec = mb_compressed / (time_ms / 1000.0);
        }
        
        // Update timestamps
        if metrics.first_compression_at.is_none() {
            metrics.first_compression_at = Some(chrono::Utc::now().timestamp());
        }
        metrics.last_compression_at = Some(chrono::Utc::now().timestamp());
        metrics.last_updated = chrono::Utc::now().timestamp();
        
        // Update global stats
        let mut global = self.global_stats.write().await;
        global.total_compression_operations += 1;
        global.total_space_saved_bytes += 
            result.uncompressed_size as i64 - result.compressed_size as i64;
    }
    
    /// Record a decompression operation
    pub async fn record_decompression(
        &self,
        collection_id: &str,
        result: DecompressionResult,
    ) {
        if let Some(mut metrics) = self.collection_metrics.get_mut(collection_id) {
            metrics.total_decompression_operations += 1;
            
            // Update timing
            let time_ms = result.decompression_time.as_millis() as f64;
            metrics.avg_decompression_time_ms = 
                (metrics.avg_decompression_time_ms * (metrics.total_decompression_operations - 1) as f64 + time_ms) 
                / metrics.total_decompression_operations as f64;
            
            // Update throughput
            if time_ms > 0.0 {
                let mb_decompressed = result.uncompressed_size as f64 / (1024.0 * 1024.0);
                metrics.decompression_throughput_mb_per_sec = mb_decompressed / (time_ms / 1000.0);
            }
            
            metrics.last_updated = chrono::Utc::now().timestamp();
        }
        
        // Update global stats
        let mut global = self.global_stats.write().await;
        global.total_decompression_operations += 1;
    }
    
    /// Get compression metrics for a collection
    pub fn get_collection_metrics(&self, collection_id: &str) -> Option<CompressionMetrics> {
        self.collection_metrics.get(collection_id).map(|m| m.clone())
    }
    
    /// Get all compression metrics
    pub fn get_all_metrics(&self) -> Vec<CompressionMetrics> {
        self.collection_metrics
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }
    
    /// Get compression recommendation based on metrics
    pub fn get_compression_recommendation(&self, collection_id: &str) -> CompressionRecommendation {
        if let Some(metrics) = self.collection_metrics.get(collection_id) {
            let ratio = metrics.overall_compression_ratio;
            let ops_per_sec = if metrics.total_compression_operations > 0 {
                let duration = metrics.last_compression_at.unwrap_or(0) - 
                             metrics.first_compression_at.unwrap_or(0);
                if duration > 0 {
                    metrics.total_compression_operations as f64 / duration as f64
                } else {
                    0.0
                }
            } else {
                0.0
            };
            
            // Recommend based on compression ratio and operation frequency
            if ratio < 1.5 {
                CompressionRecommendation::DisableCompression {
                    reason: "Low compression ratio".to_string(),
                }
            } else if ratio > 5.0 && ops_per_sec < 10.0 {
                CompressionRecommendation::IncreaseLevel {
                    reason: "Good ratio with low write frequency".to_string(),
                }
            } else if ratio < 2.0 && ops_per_sec > 100.0 {
                CompressionRecommendation::DecreaseLevel {
                    reason: "High write frequency with modest ratio".to_string(),
                }
            } else {
                CompressionRecommendation::Optimal
            }
        } else {
            CompressionRecommendation::NoData
        }
    }
}

/// Compression recommendation based on metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionRecommendation {
    Optimal,
    IncreaseLevel { reason: String },
    DecreaseLevel { reason: String },
    DisableCompression { reason: String },
    EnableQuantization { reason: String },
    NoData,
}

impl Default for CompressionMetricsTracker {
    fn default() -> Self {
        Self::new()
    }
}
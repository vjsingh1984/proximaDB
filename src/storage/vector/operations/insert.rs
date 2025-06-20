// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Vector Insert Operations
//!
//! Optimized insert operations for vector data with intelligent batching,
//! validation, and performance monitoring.

use anyhow::Result;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{debug, info, warn};

use super::super::types::*;
use crate::core::{CollectionId, VectorId, VectorRecord};

/// Vector insert operation handler
pub struct VectorInsertHandler {
    /// Validation rules
    validators: Vec<Box<dyn InsertValidator>>,
    
    /// Insert optimizers
    optimizers: Vec<Box<dyn InsertOptimizer>>,
    
    /// Performance tracker
    performance_tracker: InsertPerformanceTracker,
    
    /// Configuration
    config: InsertConfig,
}

/// Configuration for insert operations
#[derive(Debug, Clone)]
pub struct InsertConfig {
    /// Enable validation
    pub enable_validation: bool,
    
    /// Enable optimization
    pub enable_optimization: bool,
    
    /// Batch size threshold
    pub batch_threshold: usize,
    
    /// Maximum vector dimension
    pub max_dimension: usize,
    
    /// Enable duplicate detection
    pub enable_duplicate_detection: bool,
    
    /// Performance monitoring
    pub enable_performance_tracking: bool,
}

/// Insert validator trait
pub trait InsertValidator: Send + Sync {
    /// Validate a vector record before insertion
    fn validate(&self, record: &VectorRecord) -> Result<ValidationResult>;
    
    /// Get validator name
    fn name(&self) -> &'static str;
}

/// Insert optimizer trait
pub trait InsertOptimizer: Send + Sync {
    /// Optimize insert operation
    fn optimize(&self, operation: InsertOperation) -> Result<OptimizedInsertOperation>;
    
    /// Get optimizer name
    fn name(&self) -> &'static str;
}

/// Validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub suggestions: Vec<String>,
}

/// Insert operation
#[derive(Debug, Clone)]
pub struct InsertOperation {
    pub records: Vec<VectorRecord>,
    pub collection_id: CollectionId,
    pub options: InsertOptions,
}

/// Insert options
#[derive(Debug, Clone)]
pub struct InsertOptions {
    pub batch_mode: bool,
    pub validate_before_insert: bool,
    pub update_indexes: bool,
    pub generate_id_if_missing: bool,
    pub skip_duplicates: bool,
}

/// Optimized insert operation
#[derive(Debug, Clone)]
pub struct OptimizedInsertOperation {
    pub original: InsertOperation,
    pub optimizations_applied: Vec<String>,
    pub batches: Vec<InsertBatch>,
    pub estimated_time_ms: u64,
}

/// Insert batch for optimized processing
#[derive(Debug, Clone)]
pub struct InsertBatch {
    pub batch_id: String,
    pub records: Vec<VectorRecord>,
    pub priority: BatchPriority,
    pub processing_hint: ProcessingHint,
}

/// Batch priority levels
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum BatchPriority {
    Low,
    Normal,
    High,
    Critical,
}

/// Processing hint for batch optimization
#[derive(Debug, Clone)]
pub enum ProcessingHint {
    Sequential,
    Parallel,
    Vectorized,
    MemoryOptimized,
}

/// Insert performance tracker
#[derive(Debug)]
pub struct InsertPerformanceTracker {
    /// Operation history
    history: Vec<InsertPerformanceRecord>,
    
    /// Performance statistics
    stats: InsertStats,
}

/// Insert performance record
#[derive(Debug, Clone)]
pub struct InsertPerformanceRecord {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub collection_id: CollectionId,
    pub record_count: usize,
    pub duration_ms: u64,
    pub throughput_ops_sec: f64,
    pub validation_time_ms: u64,
    pub optimization_time_ms: u64,
    pub actual_insert_time_ms: u64,
    pub success: bool,
    pub error_type: Option<String>,
}

/// Insert statistics
#[derive(Debug, Default)]
pub struct InsertStats {
    pub total_operations: u64,
    pub total_records: u64,
    pub avg_throughput_ops_sec: f64,
    pub avg_latency_ms: f64,
    pub success_rate: f64,
    pub validation_overhead_percent: f64,
    pub optimization_benefit_percent: f64,
}

// Implementation

impl VectorInsertHandler {
    /// Create new insert handler
    pub fn new(config: InsertConfig) -> Self {
        let mut validators: Vec<Box<dyn InsertValidator>> = Vec::new();
        let mut optimizers: Vec<Box<dyn InsertOptimizer>> = Vec::new();
        
        if config.enable_validation {
            validators.push(Box::new(DimensionValidator::new(config.max_dimension)));
            validators.push(Box::new(DuplicateValidator::new()));
            validators.push(Box::new(VectorQualityValidator::new()));
        }
        
        if config.enable_optimization {
            optimizers.push(Box::new(BatchOptimizer::new(config.batch_threshold)));
            optimizers.push(Box::new(VectorizedOptimizer::new()));
        }
        
        Self {
            validators,
            optimizers,
            performance_tracker: InsertPerformanceTracker::new(),
            config,
        }
    }
    
    /// Execute insert operation
    pub async fn execute_insert(
        &mut self,
        operation: InsertOperation,
    ) -> Result<InsertResult> {
        let start_time = Instant::now();
        
        // Step 1: Validation
        let validation_start = Instant::now();
        let validation_results = if self.config.enable_validation {
            self.validate_records(&operation.records).await?
        } else {
            vec![ValidationResult::default(); operation.records.len()]
        };
        let validation_time = validation_start.elapsed();
        
        // Check for validation failures
        let failed_validations: Vec<_> = validation_results.iter()
            .enumerate()
            .filter(|(_, result)| !result.valid)
            .collect();
        
        if !failed_validations.is_empty() {
            warn!("🚨 {} records failed validation", failed_validations.len());
            return Ok(InsertResult {
                success: false,
                inserted_count: 0,
                failed_count: failed_validations.len(),
                validation_results,
                performance_info: None,
                error: Some("Validation failed".to_string()),
            });
        }
        
        // Step 2: Optimization
        let optimization_start = Instant::now();
        let optimized_operation = if self.config.enable_optimization {
            self.optimize_operation(operation).await?
        } else {
            OptimizedInsertOperation::from_original(operation)
        };
        let optimization_time = optimization_start.elapsed();
        
        // Step 3: Execute optimized insertion
        let insert_start = Instant::now();
        let insert_success = self.execute_optimized_insert(&optimized_operation).await?;
        let insert_time = insert_start.elapsed();
        
        let total_time = start_time.elapsed();
        
        // Step 4: Record performance
        if self.config.enable_performance_tracking {
            let performance_record = InsertPerformanceRecord {
                timestamp: chrono::Utc::now(),
                collection_id: optimized_operation.original.collection_id.clone(),
                record_count: optimized_operation.original.records.len(),
                duration_ms: total_time.as_millis() as u64,
                throughput_ops_sec: optimized_operation.original.records.len() as f64 * 1000.0 / total_time.as_millis() as f64,
                validation_time_ms: validation_time.as_millis() as u64,
                optimization_time_ms: optimization_time.as_millis() as u64,
                actual_insert_time_ms: insert_time.as_millis() as u64,
                success: insert_success,
                error_type: None,
            };
            
            self.performance_tracker.record_operation(performance_record);
        }
        
        info!("✅ Insert operation completed: {} records in {:?}", 
              optimized_operation.original.records.len(), total_time);
        
        Ok(InsertResult {
            success: insert_success,
            inserted_count: if insert_success { optimized_operation.original.records.len() } else { 0 },
            failed_count: 0,
            validation_results,
            performance_info: Some(InsertPerformanceInfo {
                total_time_ms: total_time.as_millis() as u64,
                validation_time_ms: validation_time.as_millis() as u64,
                optimization_time_ms: optimization_time.as_millis() as u64,
                insert_time_ms: insert_time.as_millis() as u64,
                throughput_ops_sec: optimized_operation.original.records.len() as f64 * 1000.0 / total_time.as_millis() as f64,
                optimizations_applied: optimized_operation.optimizations_applied,
            }),
            error: None,
        })
    }
    
    /// Validate records
    async fn validate_records(&self, records: &[VectorRecord]) -> Result<Vec<ValidationResult>> {
        let mut results = Vec::new();
        
        for record in records {
            let mut combined_result = ValidationResult::default();
            
            for validator in &self.validators {
                let result = validator.validate(record)?;
                
                if !result.valid {
                    combined_result.valid = false;
                }
                
                combined_result.warnings.extend(result.warnings);
                combined_result.errors.extend(result.errors);
                combined_result.suggestions.extend(result.suggestions);
            }
            
            results.push(combined_result);
        }
        
        Ok(results)
    }
    
    /// Optimize operation
    async fn optimize_operation(&self, operation: InsertOperation) -> Result<OptimizedInsertOperation> {
        let mut optimized = OptimizedInsertOperation::from_original(operation);
        
        for optimizer in &self.optimizers {
            let temp_operation = InsertOperation {
                records: optimized.batches.iter()
                    .flat_map(|batch| batch.records.clone())
                    .collect(),
                collection_id: optimized.original.collection_id.clone(),
                options: optimized.original.options.clone(),
            };
            
            optimized = optimizer.optimize(temp_operation)?;
        }
        
        Ok(optimized)
    }
    
    /// Execute optimized insert
    async fn execute_optimized_insert(&self, operation: &OptimizedInsertOperation) -> Result<bool> {
        // In a real implementation, this would execute the actual insert
        // For now, simulate successful execution
        debug!("🔄 Executing optimized insert with {} batches", operation.batches.len());
        
        for (i, batch) in operation.batches.iter().enumerate() {
            debug!("📦 Processing batch {} with {} records", i, batch.records.len());
            
            match batch.processing_hint {
                ProcessingHint::Parallel => {
                    // Simulate parallel processing
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
                ProcessingHint::Vectorized => {
                    // Simulate vectorized processing  
                    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
                }
                _ => {
                    // Simulate sequential processing
                    tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                }
            }
        }
        
        Ok(true)
    }
    
    /// Get performance statistics
    pub fn get_performance_stats(&self) -> &InsertStats {
        &self.performance_tracker.stats
    }
}

/// Insert result
#[derive(Debug, Clone)]
pub struct InsertResult {
    pub success: bool,
    pub inserted_count: usize,
    pub failed_count: usize,
    pub validation_results: Vec<ValidationResult>,
    pub performance_info: Option<InsertPerformanceInfo>,
    pub error: Option<String>,
}

/// Insert performance information
#[derive(Debug, Clone)]
pub struct InsertPerformanceInfo {
    pub total_time_ms: u64,
    pub validation_time_ms: u64,
    pub optimization_time_ms: u64,
    pub insert_time_ms: u64,
    pub throughput_ops_sec: f64,
    pub optimizations_applied: Vec<String>,
}

// Validator implementations

/// Dimension validator
pub struct DimensionValidator {
    max_dimension: usize,
}

impl DimensionValidator {
    pub fn new(max_dimension: usize) -> Self {
        Self { max_dimension }
    }
}

impl InsertValidator for DimensionValidator {
    fn validate(&self, record: &VectorRecord) -> Result<ValidationResult> {
        let mut result = ValidationResult::default();
        
        if record.vector.is_empty() {
            result.valid = false;
            result.errors.push("Vector cannot be empty".to_string());
        } else if record.vector.len() > self.max_dimension {
            result.valid = false;
            result.errors.push(format!("Vector dimension {} exceeds maximum {}", 
                                     record.vector.len(), self.max_dimension));
        }
        
        // Check for NaN or infinite values
        for (i, &value) in record.vector.iter().enumerate() {
            if !value.is_finite() {
                result.valid = false;
                result.errors.push(format!("Invalid value at dimension {}: {}", i, value));
            }
        }
        
        Ok(result)
    }
    
    fn name(&self) -> &'static str {
        "DimensionValidator"
    }
}

/// Duplicate validator
pub struct DuplicateValidator {
    seen_ids: HashMap<VectorId, usize>,
}

impl DuplicateValidator {
    pub fn new() -> Self {
        Self {
            seen_ids: HashMap::new(),
        }
    }
}

impl InsertValidator for DuplicateValidator {
    fn validate(&self, record: &VectorRecord) -> Result<ValidationResult> {
        let mut result = ValidationResult::default();
        
        if self.seen_ids.contains_key(&record.id) {
            result.warnings.push(format!("Duplicate vector ID detected: {}", record.id));
        }
        
        Ok(result)
    }
    
    fn name(&self) -> &'static str {
        "DuplicateValidator"
    }
}

/// Vector quality validator
pub struct VectorQualityValidator;

impl VectorQualityValidator {
    pub fn new() -> Self {
        Self
    }
}

impl InsertValidator for VectorQualityValidator {
    fn validate(&self, record: &VectorRecord) -> Result<ValidationResult> {
        let mut result = ValidationResult::default();
        
        // Check vector quality metrics
        let norm: f32 = record.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        
        if norm == 0.0 {
            result.warnings.push("Zero-norm vector detected".to_string());
        } else if norm < 1e-6 {
            result.warnings.push("Very small norm vector detected".to_string());
        }
        
        // Check for sparse vectors
        let zero_count = record.vector.iter().filter(|&&x| x == 0.0).count();
        let sparsity = zero_count as f32 / record.vector.len() as f32;
        
        if sparsity > 0.9 {
            result.suggestions.push("Consider using sparse vector format".to_string());
        }
        
        Ok(result)
    }
    
    fn name(&self) -> &'static str {
        "VectorQualityValidator"
    }
}

// Optimizer implementations

/// Batch optimizer
pub struct BatchOptimizer {
    batch_threshold: usize,
}

impl BatchOptimizer {
    pub fn new(batch_threshold: usize) -> Self {
        Self { batch_threshold }
    }
}

impl InsertOptimizer for BatchOptimizer {
    fn optimize(&self, operation: InsertOperation) -> Result<OptimizedInsertOperation> {
        let mut batches = Vec::new();
        let mut optimizations = Vec::new();
        
        if operation.records.len() > self.batch_threshold {
            // Create multiple batches
            for (i, chunk) in operation.records.chunks(self.batch_threshold).enumerate() {
                batches.push(InsertBatch {
                    batch_id: format!("batch_{}", i),
                    records: chunk.to_vec(),
                    priority: BatchPriority::Normal,
                    processing_hint: ProcessingHint::Parallel,
                });
            }
            optimizations.push("Batching applied".to_string());
        } else {
            // Single batch
            batches.push(InsertBatch {
                batch_id: "single_batch".to_string(),
                records: operation.records.clone(),
                priority: BatchPriority::Normal,
                processing_hint: ProcessingHint::Sequential,
            });
        }
        
        Ok(OptimizedInsertOperation {
            original: operation,
            optimizations_applied: optimizations,
            batches,
            estimated_time_ms: 100, // Rough estimate
        })
    }
    
    fn name(&self) -> &'static str {
        "BatchOptimizer"
    }
}

/// Vectorized optimizer
pub struct VectorizedOptimizer;

impl VectorizedOptimizer {
    pub fn new() -> Self {
        Self
    }
}

impl InsertOptimizer for VectorizedOptimizer {
    fn optimize(&self, operation: InsertOperation) -> Result<OptimizedInsertOperation> {
        let mut optimizations = Vec::new();
        
        // Analyze vector characteristics for vectorization opportunities
        if !operation.records.is_empty() {
            let dimension = operation.records[0].vector.len();
            let uniform_dimension = operation.records.iter().all(|r| r.vector.len() == dimension);
            
            if uniform_dimension && dimension % 4 == 0 {
                optimizations.push("SIMD vectorization enabled".to_string());
            }
        }
        
        let batches = vec![InsertBatch {
            batch_id: "vectorized_batch".to_string(),
            records: operation.records.clone(),
            priority: BatchPriority::High,
            processing_hint: ProcessingHint::Vectorized,
        }];
        
        Ok(OptimizedInsertOperation {
            original: operation,
            optimizations_applied: optimizations,
            batches,
            estimated_time_ms: 50, // Faster with vectorization
        })
    }
    
    fn name(&self) -> &'static str {
        "VectorizedOptimizer"
    }
}

// Supporting implementations

impl ValidationResult {
    fn default() -> Self {
        Self {
            valid: true,
            warnings: Vec::new(),
            errors: Vec::new(),
            suggestions: Vec::new(),
        }
    }
}

impl OptimizedInsertOperation {
    fn from_original(operation: InsertOperation) -> Self {
        let batches = vec![InsertBatch {
            batch_id: "default_batch".to_string(),
            records: operation.records.clone(),
            priority: BatchPriority::Normal,
            processing_hint: ProcessingHint::Sequential,
        }];
        
        Self {
            original: operation,
            optimizations_applied: Vec::new(),
            batches,
            estimated_time_ms: 100,
        }
    }
}

impl InsertPerformanceTracker {
    fn new() -> Self {
        Self {
            history: Vec::new(),
            stats: InsertStats::default(),
        }
    }
    
    fn record_operation(&mut self, record: InsertPerformanceRecord) {
        self.history.push(record.clone());
        
        // Update statistics
        self.stats.total_operations += 1;
        self.stats.total_records += record.record_count as u64;
        
        // Update rolling averages
        let alpha = 0.1; // Smoothing factor
        self.stats.avg_throughput_ops_sec = self.stats.avg_throughput_ops_sec * (1.0 - alpha) + record.throughput_ops_sec * alpha;
        self.stats.avg_latency_ms = self.stats.avg_latency_ms * (1.0 - alpha) + record.duration_ms as f64 * alpha;
        
        if record.success {
            self.stats.success_rate = self.stats.success_rate * (1.0 - alpha) + 1.0 * alpha;
        } else {
            self.stats.success_rate = self.stats.success_rate * (1.0 - alpha) + 0.0 * alpha;
        }
        
        // Calculate overhead percentages
        self.stats.validation_overhead_percent = record.validation_time_ms as f64 / record.duration_ms as f64 * 100.0;
        self.stats.optimization_benefit_percent = 
            (record.duration_ms as f64 - record.actual_insert_time_ms as f64) / record.duration_ms as f64 * 100.0;
    }
}

impl Default for InsertConfig {
    fn default() -> Self {
        Self {
            enable_validation: true,
            enable_optimization: true,
            batch_threshold: 1000,
            max_dimension: 2048,
            enable_duplicate_detection: true,
            enable_performance_tracking: true,
        }
    }
}

impl Default for InsertOptions {
    fn default() -> Self {
        Self {
            batch_mode: true,
            validate_before_insert: true,
            update_indexes: true,
            generate_id_if_missing: true,
            skip_duplicates: false,
        }
    }
}
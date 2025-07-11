//! Static Column Projection for Pre-computed Quantization
//!
//! This module implements intelligent column selection strategies for Parquet files
//! that contain pre-computed quantized vectors. It optimizes I/O by projecting only
//! the columns needed for a specific query, reducing memory usage and improving
//! search performance.

use anyhow::Result;
use std::collections::HashSet;
use tracing::{debug, info, warn};

use crate::compute::UnifiedQuantizationLevel;
use crate::core::search::SearchParams;

/// Strategy for selecting columns based on query requirements
#[derive(Debug, Clone)]
pub struct ColumnProjectionStrategy {
    /// Always include core columns
    core_columns: HashSet<String>,
    
    /// Quantization column mapping
    quantization_columns: QuantizationColumnMapping,
    
    /// Performance optimization settings
    optimization_config: ProjectionOptimizationConfig,
}

/// Mapping of quantization types to their column names
#[derive(Debug, Clone)]
pub struct QuantizationColumnMapping {
    /// Product quantization columns
    pub pq_columns: Vec<String>,
    
    /// Scalar quantization columns
    pub sq_columns: Vec<String>,
    
    /// Binary quantization columns
    pub binary_columns: Vec<String>,
    
    /// Uniform quantization columns
    pub uniform_columns: Vec<String>,
    
    /// Original FP32 vector column
    pub fp32_column: String,
}

/// Configuration for projection optimization
#[derive(Debug, Clone)]
pub struct ProjectionOptimizationConfig {
    /// Use quantized columns for initial filtering
    pub enable_quantized_filtering: bool,
    
    /// Use FP32 for final refinement
    pub enable_fp32_refinement: bool,
    
    /// Maximum number of columns to project
    pub max_columns: usize,
    
    /// Minimum accuracy threshold for quantized search
    pub min_accuracy_threshold: f32,
    
    /// Enable adaptive column selection
    pub enable_adaptive_selection: bool,
}

impl Default for ProjectionOptimizationConfig {
    fn default() -> Self {
        Self {
            enable_quantized_filtering: true,
            enable_fp32_refinement: true,
            max_columns: 50,
            min_accuracy_threshold: 0.95,
            enable_adaptive_selection: true,
        }
    }
}

impl ColumnProjectionStrategy {
    /// Create a new column projection strategy
    pub fn new() -> Self {
        let mut core_columns = HashSet::new();
        core_columns.insert("id".to_string());
        core_columns.insert("collection_id".to_string());
        core_columns.insert("timestamp".to_string());
        core_columns.insert("version".to_string());
        
        Self {
            core_columns,
            quantization_columns: QuantizationColumnMapping::default(),
            optimization_config: ProjectionOptimizationConfig::default(),
        }
    }
    
    /// Configure quantization column mapping
    pub fn with_quantization_mapping(mut self, mapping: QuantizationColumnMapping) -> Self {
        self.quantization_columns = mapping;
        self
    }
    
    /// Configure optimization settings
    pub fn with_optimization_config(mut self, config: ProjectionOptimizationConfig) -> Self {
        self.optimization_config = config;
        self
    }
    
    /// Select optimal columns for a given search query
    pub fn select_columns(
        &self,
        search_params: &SearchParams,
        available_quantization: &[UnifiedQuantizationLevel],
        estimated_result_size: usize,
    ) -> Result<ColumnProjection> {
        let mut projection = ColumnProjection::new();
        
        // Always include core columns
        projection.add_columns(self.core_columns.clone());
        
        // Add filterable columns if filters are present
        if let Some(filters) = &search_params.filters {
            for (field_name, _) in filters {
                projection.add_column(field_name.clone());
            }
        }
        
        // Select quantization strategy based on query requirements
        let quantization_strategy = self.select_quantization_strategy(
            search_params,
            available_quantization,
            estimated_result_size,
        )?;
        
        // Add quantization columns based on strategy
        self.add_quantization_columns(&mut projection, &quantization_strategy)?;
        
        // Apply optimization rules
        self.apply_optimization_rules(&mut projection, search_params)?;
        
        info!("📊 Column projection selected {} columns for query", projection.columns.len());
        debug!("🔍 Selected columns: {:?}", projection.columns);
        
        Ok(projection)
    }
    
    /// Select the best quantization strategy for the query
    fn select_quantization_strategy(
        &self,
        search_params: &SearchParams,
        available_quantization: &[UnifiedQuantizationLevel],
        estimated_result_size: usize,
    ) -> Result<QuantizationStrategy> {
        // Rule-based selection based on query characteristics
        let accuracy_requirement = search_params.accuracy_threshold.unwrap_or(0.95);
        let top_k = search_params.top_k.unwrap_or(10);
        
        // High accuracy requirement or small result set -> prefer FP32
        if accuracy_requirement >= 0.99 || top_k <= 10 {
            return Ok(QuantizationStrategy::FP32Only);
        }
        
        // Large result set with moderate accuracy -> use two-stage approach
        if estimated_result_size > 10000 && accuracy_requirement >= 0.90 {
            // Find best quantization level for filtering
            let filter_quantization = self.select_filter_quantization(available_quantization)?;
            return Ok(QuantizationStrategy::TwoStage {
                filter_quantization,
                refinement_quantization: UnifiedQuantizationLevel {
                    level_type: Some(crate::compute::QuantizationLevelType::None(crate::compute::NoQuantization {})),
                },
            });
        }
        
        // Fast approximate search -> use best available quantization
        if accuracy_requirement < 0.90 {
            let best_quantization = self.select_best_quantization(available_quantization)?;
            return Ok(QuantizationStrategy::QuantizedOnly(best_quantization));
        }
        
        // Default to FP32 for safety
        Ok(QuantizationStrategy::FP32Only)
    }
    
    /// Select quantization level for filtering stage
    fn select_filter_quantization(&self, available: &[UnifiedQuantizationLevel]) -> Result<UnifiedQuantizationLevel> {
        // Prefer PQ8 for filtering if available
        let pq8 = UnifiedQuantizationLevel::pq8(8);
        if available.contains(&pq8) {
            return Ok(pq8);
        }
        
        // Fall back to uniform quantization for fast filtering
        let uniform8 = UnifiedQuantizationLevel {
            level_type: Some(crate::compute::QuantizationLevelType::Uniform(crate::compute::UniformQuantization {
                bits: 8,
                scale: None,
                offset: None,
            })),
        };
        if available.contains(&uniform8) {
            return Ok(uniform8);
        }
        
        // Use any available quantization as last resort
        if let Some(first_available) = available.first() {
            return Ok(first_available.clone());
        }
        
        anyhow::bail!("No suitable quantization level found for filtering");
    }
    
    /// Select best available quantization level
    fn select_best_quantization(&self, available: &[UnifiedQuantizationLevel]) -> Result<UnifiedQuantizationLevel> {
        // Preference order: PQ8 > PQ4 > Uniform(8) > Others
        let preference_order = [
            UnifiedQuantizationLevel::pq8(8),
            UnifiedQuantizationLevel::pq4(8),
            UnifiedQuantizationLevel {
                level_type: Some(crate::compute::QuantizationLevelType::Uniform(crate::compute::UniformQuantization {
                    bits: 8,
                    scale: None,
                    offset: None,
                })),
            },
            UnifiedQuantizationLevel {
                level_type: Some(crate::compute::QuantizationLevelType::Uniform(crate::compute::UniformQuantization {
                    bits: 4,
                    scale: None,
                    offset: None,
                })),
            },
        ];
        
        for preferred in &preference_order {
            if available.contains(preferred) {
                return Ok(preferred.clone());
            }
        }
        
        // Use any available quantization as last resort
        if let Some(first_available) = available.first() {
            return Ok(first_available.clone());
        }
        
        anyhow::bail!("No quantization levels available");
    }
    
    /// Add quantization columns based on strategy
    fn add_quantization_columns(
        &self,
        projection: &mut ColumnProjection,
        strategy: &QuantizationStrategy,
    ) -> Result<()> {
        match strategy {
            QuantizationStrategy::FP32Only => {
                projection.add_column(self.quantization_columns.fp32_column.clone());
            }
            QuantizationStrategy::QuantizedOnly(level) => {
                self.add_quantization_level_columns(projection, level)?;
            }
            QuantizationStrategy::TwoStage { filter_quantization, refinement_quantization } => {
                self.add_quantization_level_columns(projection, filter_quantization)?;
                self.add_quantization_level_columns(projection, refinement_quantization)?;
            }
        }
        Ok(())
    }
    
    /// Add columns for specific quantization level
    fn add_quantization_level_columns(
        &self,
        projection: &mut ColumnProjection,
        level: &UnifiedQuantizationLevel,
    ) -> Result<()> {
        match &level.level_type {
            Some(crate::compute::QuantizationLevelType::None(_)) => {
                projection.add_column(self.quantization_columns.fp32_column.clone());
            }
            Some(crate::compute::QuantizationLevelType::Pq(_)) => {
                projection.add_columns(self.quantization_columns.pq_columns.iter().cloned().collect());
            }
            Some(crate::compute::QuantizationLevelType::Uniform(_)) => {
                projection.add_columns(self.quantization_columns.uniform_columns.iter().cloned().collect());
            }
            Some(crate::compute::QuantizationLevelType::Custom(_)) => {
                projection.add_columns(self.quantization_columns.uniform_columns.iter().cloned().collect());
            }
            Some(crate::compute::QuantizationLevelType::Scalar(_)) => {
                projection.add_columns(self.quantization_columns.sq_columns.iter().cloned().collect());
            }
            Some(crate::compute::QuantizationLevelType::Binary(_)) => {
                projection.add_columns(self.quantization_columns.binary_columns.iter().cloned().collect());
            }
            None => {
                // Default to FP32 if no quantization specified
                projection.add_column(self.quantization_columns.fp32_column.clone());
            }
        }
        Ok(())
    }
    
    /// Apply optimization rules to reduce column count
    fn apply_optimization_rules(
        &self,
        projection: &mut ColumnProjection,
        search_params: &SearchParams,
    ) -> Result<()> {
        // Limit total columns
        if projection.columns.len() > self.optimization_config.max_columns {
            warn!("🚨 Column projection exceeds maximum columns ({}), applying limits", 
                  self.optimization_config.max_columns);
            projection.limit_columns(self.optimization_config.max_columns);
        }
        
        // Remove unnecessary metadata columns if no filters
        if search_params.filters.is_none() {
            projection.remove_metadata_columns();
        }
        
        // Remove expires_at if not needed
        if !search_params.include_expired.unwrap_or(false) {
            projection.remove_column("expires_at");
        }
        
        Ok(())
    }
}

/// Quantization strategy for column selection
#[derive(Debug, Clone)]
pub enum QuantizationStrategy {
    /// Use only FP32 vectors
    FP32Only,
    
    /// Use only quantized vectors
    QuantizedOnly(UnifiedQuantizationLevel),
    
    /// Two-stage: quantized filtering + FP32 refinement
    TwoStage {
        filter_quantization: UnifiedQuantizationLevel,
        refinement_quantization: UnifiedQuantizationLevel,
    },
}

/// Result of column projection
#[derive(Debug, Clone)]
pub struct ColumnProjection {
    /// Selected columns
    pub columns: HashSet<String>,
    
    /// Quantization strategy used
    pub strategy: Option<QuantizationStrategy>,
    
    /// Estimated I/O reduction percentage
    pub io_reduction_estimate: f32,
}

impl ColumnProjection {
    fn new() -> Self {
        Self {
            columns: HashSet::new(),
            strategy: None,
            io_reduction_estimate: 0.0,
        }
    }
    
    fn add_column(&mut self, column: String) {
        self.columns.insert(column);
    }
    
    fn add_columns(&mut self, columns: HashSet<String>) {
        self.columns.extend(columns);
    }
    
    fn remove_column(&mut self, column: &str) {
        self.columns.remove(column);
    }
    
    fn limit_columns(&mut self, max_columns: usize) {
        if self.columns.len() > max_columns {
            let columns: Vec<String> = self.columns.iter().cloned().collect();
            self.columns.clear();
            self.columns.extend(columns.into_iter().take(max_columns));
        }
    }
    
    fn remove_metadata_columns(&mut self) {
        self.columns.remove("extra_meta");
        self.columns.remove("created_at");
        self.columns.remove("updated_at");
    }
    
    /// Convert to Parquet column names for projection
    pub fn to_parquet_columns(&self) -> Vec<String> {
        self.columns.iter().cloned().collect()
    }
    
    /// Estimate I/O reduction from projection
    pub fn estimate_io_reduction(&self, total_columns: usize) -> f32 {
        if total_columns == 0 {
            return 0.0;
        }
        
        let selected_columns = self.columns.len();
        let reduction = 1.0 - (selected_columns as f32 / total_columns as f32);
        reduction.max(0.0).min(1.0)
    }
}

impl Default for QuantizationColumnMapping {
    fn default() -> Self {
        Self {
            pq_columns: vec!["vector_pq".to_string()],
            sq_columns: vec!["vector_sq".to_string(), "sq_scale".to_string(), "sq_offset".to_string()],
            binary_columns: vec!["vector_binary".to_string()],
            uniform_columns: vec!["vector_quantized".to_string()],
            fp32_column: "vector".to_string(),
        }
    }
}

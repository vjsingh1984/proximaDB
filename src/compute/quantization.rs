// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Advanced quantization techniques for ProximaDB

use std::collections::HashMap;
use serde::{Serialize, Deserialize};

use crate::core::VectorDBError;

type Result<T> = std::result::Result<T, VectorDBError>;
type ProximaDBError = VectorDBError;

/// Quantization techniques available in ProximaDB
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuantizationType {
    /// No quantization (full precision)
    None,
    
    /// Product Quantization (PQ)
    ProductQuantization,
    
    /// Scalar Quantization (SQ)
    ScalarQuantization,
    
    /// Binary Quantization
    BinaryQuantization,
    
    /// Additive Quantization (AQ)
    AdditiveQuantization,
    
    /// Residual Vector Quantization (RVQ)
    ResidualVectorQuantization,
    
    /// Optimized Product Quantization (OPQ)
    OptimizedProductQuantization,
}

/// Quantization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationConfig {
    /// Type of quantization to use
    pub quantization_type: QuantizationType,
    
    /// Number of subquantizers (for PQ/OPQ)
    pub num_subquantizers: usize,
    
    /// Number of centroids per subquantizer
    pub num_centroids: usize,
    
    /// Number of bits per code
    pub bits_per_code: usize,
    
    /// Training iterations for codebook learning
    pub training_iterations: usize,
    
    /// Enable fast distance computation
    pub enable_fast_distance: bool,
    
    /// Compression ratio achieved
    pub compression_ratio: f32,
}

impl Default for QuantizationConfig {
    fn default() -> Self {
        Self {
            quantization_type: QuantizationType::ProductQuantization,
            num_subquantizers: 8,
            num_centroids: 256,
            bits_per_code: 8,
            training_iterations: 25,
            enable_fast_distance: true,
            compression_ratio: 4.0,
        }
    }
}

/// Main quantization engine
pub struct QuantizationEngine {
    config: QuantizationConfig,
    quantizer: Box<dyn VectorQuantizer + Send + Sync>,
}

/// Trait for vector quantization implementations
pub trait VectorQuantizer {
    /// Train the quantizer on a set of vectors
    fn train(&mut self, vectors: &[Vec<f32>]) -> Result<()>;
    
    /// Quantize a batch of vectors
    fn quantize(&self, vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>>;
    
    /// Dequantize codes back to approximate vectors
    fn dequantize(&self, codes: &[QuantizedVector]) -> Result<Vec<Vec<f32>>>;
    
    /// Compute distance between query and quantized vectors
    fn compute_distances(&self, query: &[f32], codes: &[QuantizedVector]) -> Result<Vec<f32>>;
    
    /// Get memory usage per vector in bytes
    fn memory_per_vector(&self) -> usize;
    
    /// Check if the quantizer is trained
    fn is_trained(&self) -> bool;
}

/// Quantized vector representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizedVector {
    /// Quantization codes
    pub codes: Vec<u8>,
    
    /// Original vector norm (for normalization)
    pub norm: f32,
    
    /// Additional metadata
    pub metadata: HashMap<String, f32>,
}

impl QuantizationEngine {
    /// Create a new quantization engine
    pub fn new(config: QuantizationConfig) -> Result<Self> {
        let quantizer = Self::create_quantizer(&config)?;
        
        Ok(Self {
            config,
            quantizer,
        })
    }
    
    /// Create the appropriate quantizer based on configuration
    fn create_quantizer(config: &QuantizationConfig) -> Result<Box<dyn VectorQuantizer + Send + Sync>> {
        match config.quantization_type {
            QuantizationType::ProductQuantization => {
                Ok(Box::new(ProductQuantizer::new(config.clone())))
            }
            QuantizationType::ScalarQuantization => {
                Ok(Box::new(ScalarQuantizer::new(config.clone())))
            }
            QuantizationType::BinaryQuantization => {
                Ok(Box::new(BinaryQuantizer::new(config.clone())))
            }
            QuantizationType::AdditiveQuantization => {
                Ok(Box::new(AdditiveQuantizer::new(config.clone())))
            }
            QuantizationType::ResidualVectorQuantization => {
                Ok(Box::new(ResidualVectorQuantizer::new(config.clone())))
            }
            QuantizationType::OptimizedProductQuantization => {
                Ok(Box::new(OptimizedProductQuantizer::new(config.clone())))
            }
            QuantizationType::None => {
                Err(ProximaDBError::Config("No quantization specified".to_string()))
            }
        }
    }
    
    /// Train the quantization engine
    pub fn train(&mut self, training_vectors: &[Vec<f32>]) -> Result<()> {
        self.quantizer.train(training_vectors)
    }
    
    /// Quantize vectors
    pub fn quantize(&self, vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>> {
        self.quantizer.quantize(vectors)
    }
    
    /// Dequantize vectors
    pub fn dequantize(&self, codes: &[QuantizedVector]) -> Result<Vec<Vec<f32>>> {
        self.quantizer.dequantize(codes)
    }
    
    /// Compute distances efficiently
    pub fn compute_distances(&self, query: &[f32], codes: &[QuantizedVector]) -> Result<Vec<f32>> {
        self.quantizer.compute_distances(query, codes)
    }
    
    /// Get compression ratio
    pub fn compression_ratio(&self, original_dimension: usize) -> f32 {
        let original_size = original_dimension * 4; // f32 = 4 bytes
        let quantized_size = self.quantizer.memory_per_vector();
        original_size as f32 / quantized_size as f32
    }
    
    /// Check if trained
    pub fn is_trained(&self) -> bool {
        self.quantizer.is_trained()
    }
}

/// Product Quantization implementation
pub struct ProductQuantizer {
    config: QuantizationConfig,
    codebooks: Vec<Vec<Vec<f32>>>,
    subvector_dim: usize,
    trained: bool,
}

impl ProductQuantizer {
    pub fn new(config: QuantizationConfig) -> Self {
        Self {
            config,
            codebooks: Vec::new(),
            subvector_dim: 0,
            trained: false,
        }
    }
    
    /// K-means clustering for codebook learning
    fn train_codebook(&self, vectors: &[Vec<f32>], num_centroids: usize) -> Result<Vec<Vec<f32>>> {
        if vectors.is_empty() {
            return Err(ProximaDBError::Quantization("No training vectors provided".to_string()));
        }
        
        let _dim = vectors[0].len();
        let num_vectors = vectors.len();
        
        if num_vectors < num_centroids {
            return Err(ProximaDBError::Quantization(
                "Not enough training vectors for k-means".to_string()
            ));
        }
        
        // Initialize centroids randomly
        let mut centroids = Vec::new();
        for i in 0..num_centroids {
            let random_idx = i % num_vectors;
            centroids.push(vectors[random_idx].clone());
        }
        
        // K-means iterations
        for _ in 0..self.config.training_iterations {
            let mut assignments = vec![0; num_vectors];
            let mut cluster_sizes = vec![0; num_centroids];
            
            // Assignment step
            for (i, vector) in vectors.iter().enumerate() {
                let mut min_distance = f32::INFINITY;
                let mut best_centroid = 0;
                
                for (j, centroid) in centroids.iter().enumerate() {
                    let distance = self.euclidean_distance(vector, centroid);
                    if distance < min_distance {
                        min_distance = distance;
                        best_centroid = j;
                    }
                }
                
                assignments[i] = best_centroid;
                cluster_sizes[best_centroid] += 1;
            }
            
            // Update step
            for centroid in centroids.iter_mut() {
                centroid.fill(0.0);
            }
            
            for (i, vector) in vectors.iter().enumerate() {
                let centroid_idx = assignments[i];
                for (j, value) in vector.iter().enumerate() {
                    centroids[centroid_idx][j] += value;
                }
            }
            
            // Normalize by cluster sizes
            for (i, size) in cluster_sizes.iter().enumerate() {
                if *size > 0 {
                    for value in centroids[i].iter_mut() {
                        *value /= *size as f32;
                    }
                }
            }
        }
        
        Ok(centroids)
    }
    
    fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>()
            .sqrt()
    }
}

impl VectorQuantizer for ProductQuantizer {
    fn train(&mut self, vectors: &[Vec<f32>]) -> Result<()> {
        if vectors.is_empty() {
            return Err(ProximaDBError::Quantization("No training vectors provided".to_string()));
        }
        
        let dimension = vectors[0].len();
        
        if dimension % self.config.num_subquantizers != 0 {
            return Err(ProximaDBError::Quantization(
                "Dimension must be divisible by number of subquantizers".to_string()
            ));
        }
        
        self.subvector_dim = dimension / self.config.num_subquantizers;
        self.codebooks.clear();
        
        // Train codebook for each subquantizer
        for i in 0..self.config.num_subquantizers {
            let start_idx = i * self.subvector_dim;
            let end_idx = start_idx + self.subvector_dim;
            
            let subvectors: Vec<Vec<f32>> = vectors.iter()
                .map(|v| v[start_idx..end_idx].to_vec())
                .collect();
            
            let codebook = self.train_codebook(&subvectors, self.config.num_centroids)?;
            self.codebooks.push(codebook);
        }
        
        self.trained = true;
        Ok(())
    }
    
    fn quantize(&self, vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>> {
        if !self.trained {
            return Err(ProximaDBError::Quantization("Quantizer not trained".to_string()));
        }
        
        let mut quantized = Vec::with_capacity(vectors.len());
        
        for vector in vectors {
            let mut codes = Vec::with_capacity(self.config.num_subquantizers);
            let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            
            // Quantize each subvector
            for (i, codebook) in self.codebooks.iter().enumerate() {
                let start_idx = i * self.subvector_dim;
                let end_idx = start_idx + self.subvector_dim;
                let subvector = &vector[start_idx..end_idx];
                
                // Find nearest centroid
                let mut min_distance = f32::INFINITY;
                let mut best_code = 0u8;
                
                for (j, centroid) in codebook.iter().enumerate() {
                    let distance = self.euclidean_distance(subvector, centroid);
                    if distance < min_distance {
                        min_distance = distance;
                        best_code = j as u8;
                    }
                }
                
                codes.push(best_code);
            }
            
            quantized.push(QuantizedVector {
                codes,
                norm,
                metadata: HashMap::new(),
            });
        }
        
        Ok(quantized)
    }
    
    fn dequantize(&self, codes: &[QuantizedVector]) -> Result<Vec<Vec<f32>>> {
        if !self.trained {
            return Err(ProximaDBError::Quantization("Quantizer not trained".to_string()));
        }
        
        let dimension = self.config.num_subquantizers * self.subvector_dim;
        let mut result = Vec::with_capacity(codes.len());
        
        for quantized in codes {
            let mut vector = vec![0.0; dimension];
            
            for (j, &code) in quantized.codes.iter().enumerate() {
                let start_idx = j * self.subvector_dim;
                let end_idx = start_idx + self.subvector_dim;
                let centroid = &self.codebooks[j][code as usize];
                vector[start_idx..end_idx].copy_from_slice(centroid);
            }
            
            // Restore original norm
            let current_norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            if current_norm > 0.0 {
                let scale = quantized.norm / current_norm;
                for v in vector.iter_mut() {
                    *v *= scale;
                }
            }
            
            result.push(vector);
        }
        
        Ok(result)
    }
    
    fn compute_distances(&self, query: &[f32], codes: &[QuantizedVector]) -> Result<Vec<f32>> {
        if !self.trained {
            return Err(ProximaDBError::Quantization("Quantizer not trained".to_string()));
        }
        
        // Precompute distance tables for fast distance computation
        let mut distance_tables = Vec::with_capacity(self.config.num_subquantizers);
        
        for (i, codebook) in self.codebooks.iter().enumerate() {
            let start_idx = i * self.subvector_dim;
            let end_idx = start_idx + self.subvector_dim;
            let query_subvector = &query[start_idx..end_idx];
            
            let mut table = Vec::with_capacity(self.config.num_centroids);
            for centroid in codebook {
                let distance = self.euclidean_distance(query_subvector, centroid);
                table.push(distance);
            }
            distance_tables.push(table);
        }
        
        // Compute distances using lookup tables
        let mut distances = Vec::with_capacity(codes.len());
        for quantized in codes {
            let mut total_distance = 0.0;
            
            for (i, &code) in quantized.codes.iter().enumerate() {
                total_distance += distance_tables[i][code as usize].powi(2);
            }
            
            distances.push(total_distance.sqrt());
        }
        
        Ok(distances)
    }
    
    fn memory_per_vector(&self) -> usize {
        self.config.num_subquantizers + 4 // codes + norm (f32)
    }
    
    fn is_trained(&self) -> bool {
        self.trained
    }
}

/// Scalar Quantization implementation
pub struct ScalarQuantizer {
    config: QuantizationConfig,
    min_values: Vec<f32>,
    max_values: Vec<f32>,
    scale_factors: Vec<f32>,
    trained: bool,
}

impl ScalarQuantizer {
    pub fn new(config: QuantizationConfig) -> Self {
        Self {
            config,
            min_values: Vec::new(),
            max_values: Vec::new(),
            scale_factors: Vec::new(),
            trained: false,
        }
    }
    
    fn compute_quantization_parameters(&mut self, vectors: &[Vec<f32>]) -> Result<()> {
        if vectors.is_empty() {
            return Err(ProximaDBError::Quantization("No training vectors provided".to_string()));
        }
        
        let dimension = vectors[0].len();
        self.min_values = vec![f32::INFINITY; dimension];
        self.max_values = vec![f32::NEG_INFINITY; dimension];
        
        // Find min and max values for each dimension
        for vector in vectors {
            for (i, &value) in vector.iter().enumerate() {
                self.min_values[i] = self.min_values[i].min(value);
                self.max_values[i] = self.max_values[i].max(value);
            }
        }
        
        // Compute scale factors
        let max_quantized_value = (1 << self.config.bits_per_code) - 1;
        self.scale_factors = self.min_values.iter().zip(&self.max_values)
            .map(|(&min_val, &max_val)| {
                let range = max_val - min_val;
                if range > 0.0 {
                    max_quantized_value as f32 / range
                } else {
                    1.0
                }
            })
            .collect();
        
        Ok(())
    }
}

impl VectorQuantizer for ScalarQuantizer {
    fn train(&mut self, vectors: &[Vec<f32>]) -> Result<()> {
        self.compute_quantization_parameters(vectors)?;
        self.trained = true;
        Ok(())
    }
    
    fn quantize(&self, vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>> {
        if !self.trained {
            return Err(ProximaDBError::Quantization("Quantizer not trained".to_string()));
        }
        
        let mut quantized = Vec::with_capacity(vectors.len());
        
        for vector in vectors {
            let mut codes = Vec::with_capacity(vector.len());
            let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            
            for (i, &value) in vector.iter().enumerate() {
                let normalized = (value - self.min_values[i]) * self.scale_factors[i];
                let quantized_value = normalized.round().clamp(0.0, 255.0) as u8;
                codes.push(quantized_value);
            }
            
            quantized.push(QuantizedVector {
                codes,
                norm,
                metadata: HashMap::new(),
            });
        }
        
        Ok(quantized)
    }
    
    fn dequantize(&self, codes: &[QuantizedVector]) -> Result<Vec<Vec<f32>>> {
        if !self.trained {
            return Err(ProximaDBError::Quantization("Quantizer not trained".to_string()));
        }
        
        let mut result = Vec::with_capacity(codes.len());
        
        for quantized in codes {
            let mut vector = Vec::with_capacity(quantized.codes.len());
            
            for (i, &code) in quantized.codes.iter().enumerate() {
                let normalized = code as f32 / self.scale_factors[i];
                let value = normalized + self.min_values[i];
                vector.push(value);
            }
            
            // Restore original norm
            let current_norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            if current_norm > 0.0 {
                let scale = quantized.norm / current_norm;
                for v in vector.iter_mut() {
                    *v *= scale;
                }
            }
            
            result.push(vector);
        }
        
        Ok(result)
    }
    
    fn compute_distances(&self, query: &[f32], codes: &[QuantizedVector]) -> Result<Vec<f32>> {
        if !self.trained {
            return Err(ProximaDBError::Quantization("Quantizer not trained".to_string()));
        }
        
        let mut distances = Vec::with_capacity(codes.len());
        
        for quantized in codes {
            let mut distance_squared = 0.0;
            
            for (i, &code) in quantized.codes.iter().enumerate() {
                if i < query.len() {
                    let dequantized = (code as f32 / self.scale_factors[i]) + self.min_values[i];
                    let diff = query[i] - dequantized;
                    distance_squared += diff * diff;
                }
            }
            
            distances.push(distance_squared.sqrt());
        }
        
        Ok(distances)
    }
    
    fn memory_per_vector(&self) -> usize {
        // Each dimension uses 1 byte (8 bits) + norm (4 bytes)
        self.min_values.len() + 4
    }
    
    fn is_trained(&self) -> bool {
        self.trained
    }
}

pub struct BinaryQuantizer {
    config: QuantizationConfig,
    trained: bool,
}

impl BinaryQuantizer {
    pub fn new(config: QuantizationConfig) -> Self {
        Self { config, trained: false }
    }
}

impl VectorQuantizer for BinaryQuantizer {
    fn train(&mut self, _vectors: &[Vec<f32>]) -> Result<()> {
        self.trained = true;
        Ok(())
    }
    
    fn quantize(&self, _vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>> {
        Ok(Vec::new())
    }
    
    fn dequantize(&self, _codes: &[QuantizedVector]) -> Result<Vec<Vec<f32>>> {
        Ok(Vec::new())
    }
    
    fn compute_distances(&self, _query: &[f32], _codes: &[QuantizedVector]) -> Result<Vec<f32>> {
        Ok(Vec::new())
    }
    
    fn memory_per_vector(&self) -> usize {
        8 // 1 bit per dimension / 8
    }
    
    fn is_trained(&self) -> bool {
        self.trained
    }
}

pub struct AdditiveQuantizer {
    config: QuantizationConfig,
    trained: bool,
}

impl AdditiveQuantizer {
    pub fn new(config: QuantizationConfig) -> Self {
        Self { config, trained: false }
    }
}

impl VectorQuantizer for AdditiveQuantizer {
    fn train(&mut self, _vectors: &[Vec<f32>]) -> Result<()> {
        self.trained = true;
        Ok(())
    }
    
    fn quantize(&self, _vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>> {
        Ok(Vec::new())
    }
    
    fn dequantize(&self, _codes: &[QuantizedVector]) -> Result<Vec<Vec<f32>>> {
        Ok(Vec::new())
    }
    
    fn compute_distances(&self, _query: &[f32], _codes: &[QuantizedVector]) -> Result<Vec<f32>> {
        Ok(Vec::new())
    }
    
    fn memory_per_vector(&self) -> usize {
        self.config.num_subquantizers + 4
    }
    
    fn is_trained(&self) -> bool {
        self.trained
    }
}

pub struct ResidualVectorQuantizer {
    config: QuantizationConfig,
    trained: bool,
}

impl ResidualVectorQuantizer {
    pub fn new(config: QuantizationConfig) -> Self {
        Self { config, trained: false }
    }
}

impl VectorQuantizer for ResidualVectorQuantizer {
    fn train(&mut self, _vectors: &[Vec<f32>]) -> Result<()> {
        self.trained = true;
        Ok(())
    }
    
    fn quantize(&self, _vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>> {
        Ok(Vec::new())
    }
    
    fn dequantize(&self, _codes: &[QuantizedVector]) -> Result<Vec<Vec<f32>>> {
        Ok(Vec::new())
    }
    
    fn compute_distances(&self, _query: &[f32], _codes: &[QuantizedVector]) -> Result<Vec<f32>> {
        Ok(Vec::new())
    }
    
    fn memory_per_vector(&self) -> usize {
        self.config.num_subquantizers * 2 + 4
    }
    
    fn is_trained(&self) -> bool {
        self.trained
    }
}

pub struct OptimizedProductQuantizer {
    config: QuantizationConfig,
    base_quantizer: ProductQuantizer,
    trained: bool,
}

impl OptimizedProductQuantizer {
    pub fn new(config: QuantizationConfig) -> Self {
        Self {
            base_quantizer: ProductQuantizer::new(config.clone()),
            config,
            trained: false,
        }
    }
}

impl VectorQuantizer for OptimizedProductQuantizer {
    fn train(&mut self, vectors: &[Vec<f32>]) -> Result<()> {
        self.base_quantizer.train(vectors)?;
        self.trained = true;
        Ok(())
    }
    
    fn quantize(&self, vectors: &[Vec<f32>]) -> Result<Vec<QuantizedVector>> {
        self.base_quantizer.quantize(vectors)
    }
    
    fn dequantize(&self, codes: &[QuantizedVector]) -> Result<Vec<Vec<f32>>> {
        self.base_quantizer.dequantize(codes)
    }
    
    fn compute_distances(&self, query: &[f32], codes: &[QuantizedVector]) -> Result<Vec<f32>> {
        self.base_quantizer.compute_distances(query, codes)
    }
    
    fn memory_per_vector(&self) -> usize {
        self.base_quantizer.memory_per_vector()
    }
    
    fn is_trained(&self) -> bool {
        self.trained
    }
}
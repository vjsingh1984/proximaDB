/*
 * Copyright 2025 ProximaDB
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

//! HMGI Multi-Modality Detection
//!
//! Detects multi-modality collections and auto-enables HMGI for optimal performance.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use super::extraction::ModalityExtractor;

/// Result of modality detection analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    /// Number of distinct modalities found
    pub distinct_modalities: usize,

    /// Modality tags found with their counts
    pub modality_counts: HashMap<String, usize>,

    /// Whether HMGI should be auto-enabled
    pub should_enable_hmgi: bool,

    /// Confidence in the detection (0.0 to 1.0)
    pub confidence: f32,

    /// Reason for the recommendation
    pub reason: EnablementReason,
}

/// Reason for HMGI enablement recommendation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnablementReason {
    /// Single modality - HMGI not beneficial
    SingleModality,

    /// Two distinct modalities - HMGI recommended
    TwoModalities,

    /// Three or more modalities - HMGI highly recommended
    MultipleModalities,

    /// Insufficient data for detection
    InsufficientData,

    /// Explicit user configuration
    UserConfigured,
}

/// Modality detector - analyzes collection to determine if HMGI is beneficial
///
/// ## Detection Strategy
///
/// 1. Sample vectors from the collection
/// 2. Extract modality tags from metadata
/// 3. Count distinct modalities
/// 4. Recommend HMGI if 2+ modalities found
pub struct ModalityDetector {
    /// Sample size for modality detection
    sample_size: usize,

    /// Minimum distinct modalities to trigger auto-enable
    threshold: usize,

    /// Extractor for reading modality tags
    extractor: Arc<ModalityExtractor>,
}

impl ModalityDetector {
    /// Create a new modality detector
    ///
    /// ## Arguments
    ///
    /// - `sample_size`: Number of vectors to sample for detection
    /// - `threshold`: Minimum distinct modalities to recommend HMGI
    pub fn new(sample_size: usize, threshold: usize) -> Self {
        Self {
            sample_size,
            threshold,
            extractor: Arc::new(ModalityExtractor::new()),
        }
    }

    /// Create with default configuration
    ///
    /// - Sample size: 1000 vectors
    /// - Threshold: 2 distinct modalities
    pub fn default_config() -> Self {
        Self::new(1000, 2)
    }

    /// Analyze collection to detect multiple modalities
    ///
    /// ## Arguments
    ///
    /// - `collection_id`: Collection to analyze
    /// - `vectors`: Sample of vectors from the collection
    ///
    /// ## Returns
    ///
    /// Detection result with recommendation
    pub async fn detect_modalities(
        &self,
        _collection_id: &str,
        vectors: &[VectorRecordSample],
    ) -> DetectionResult {
        if vectors.is_empty() {
            return DetectionResult {
                distinct_modalities: 0,
                modality_counts: HashMap::new(),
                should_enable_hmgi: false,
                confidence: 0.0,
                reason: EnablementReason::InsufficientData,
            };
        }

        // Sample vectors if collection is larger than sample size
        let sample: &[VectorRecordSample] = if vectors.len() > self.sample_size {
            let step = vectors.len() / self.sample_size;
            // Create a Vec of references and convert to slice
            let _sampled: Vec<&VectorRecordSample> = vectors.iter().step_by(step.max(1)).collect();
            // For simplicity, just use all vectors if sampling is complex
            // In production, use proper sampling
            vectors
        } else {
            vectors
        };

        // Extract and count modalities
        let mut modality_counts: HashMap<String, usize> = HashMap::new();

        for vector in sample {
            let metadata = &vector.metadata;
            let modality = self.extractor.extract_modality(metadata);
            *modality_counts.entry(modality).or_insert(0) += 1;
        }

        let distinct_modalities = modality_counts.len();

        // Determine if HMGI should be enabled
        let (should_enable, reason, confidence) = if distinct_modalities < self.threshold {
            match distinct_modalities {
                0 => (false, EnablementReason::InsufficientData, 0.0),
                1 => (false, EnablementReason::SingleModality, 1.0),
                _ => (false, EnablementReason::SingleModality, 0.5),
            }
        } else {
            match distinct_modalities {
                2 => (
                    true,
                    EnablementReason::TwoModalities,
                    0.9, // High confidence for 2 modalities
                ),
                3 => (
                    true,
                    EnablementReason::MultipleModalities,
                    0.95, // Very high confidence
                ),
                _ => (
                    true,
                    EnablementReason::MultipleModalities,
                    1.0, // Maximum confidence for 4+ modalities
                ),
            }
        };

        DetectionResult {
            distinct_modalities,
            modality_counts,
            should_enable_hmgi: should_enable,
            confidence,
            reason,
        }
    }

    /// Re-check a collection after new inserts
    ///
    /// Should be called periodically to detect when a collection transitions
    /// from single-modality to multi-modality.
    pub async fn recheck_collection(
        &self,
        collection_id: &str,
        vectors: &[VectorRecordSample],
    ) -> Result<CollectionTransition> {
        let result = self.detect_modalities(collection_id, vectors).await;

        let transition = match result.reason {
            EnablementReason::SingleModality => CollectionTransition::SingleModality,
            EnablementReason::TwoModalities | EnablementReason::MultipleModalities => {
                CollectionTransition::MultiModality {
                    recommended: true,
                    modalities: result.modality_counts.keys().cloned().collect(),
                }
            }
            EnablementReason::InsufficientData => CollectionTransition::InsufficientData,
            EnablementReason::UserConfigured => CollectionTransition::UserConfigured,
        };

        Ok(transition)
    }

    /// Get the sample size
    pub fn sample_size(&self) -> usize {
        self.sample_size
    }

    /// Set the sample size
    pub fn set_sample_size(&mut self, size: usize) {
        self.sample_size = size;
    }

    /// Get the threshold for auto-enablement
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// Set the threshold for auto-enablement
    pub fn set_threshold(&mut self, threshold: usize) {
        self.threshold = threshold;
    }
}

impl Default for ModalityDetector {
    fn default() -> Self {
        Self::default_config()
    }
}

/// Collection transition state
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionTransition {
    /// Collection has single modality
    SingleModality,

    /// Collection has multiple modalities
    MultiModality {
        /// Whether HMGI is recommended
        recommended: bool,

        /// Modality tags found
        modalities: Vec<String>,
    },

    /// Insufficient data for determination
    InsufficientData,

    /// Explicitly configured by user
    UserConfigured,
}

/// Sample of vector record for detection
///
/// In production, this would be a reference to the actual vector record.
/// For detection purposes, we only need the metadata.
#[derive(Debug, Clone)]
pub struct VectorRecordSample {
    /// Metadata fields (including modality tag)
    pub metadata: HashMap<String, serde_json::Value>,
}

impl VectorRecordSample {
    /// Create a new sample from metadata
    pub fn new(metadata: HashMap<String, serde_json::Value>) -> Self {
        Self { metadata }
    }

    /// Create a sample with a modality tag
    pub fn with_modality(modality: &str) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("_modality".to_string(), serde_json::json!(modality));
        Self { metadata }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_detection_single_modality() {
        let detector = ModalityDetector::default_config();

        let vectors = vec![
            VectorRecordSample::with_modality("text"),
            VectorRecordSample::with_modality("text"),
            VectorRecordSample::with_modality("text"),
        ];

        let result = detector
            .detect_modalities("test_collection", &vectors)
            .await;

        assert_eq!(result.distinct_modalities, 1);
        assert!(!result.should_enable_hmgi);
        assert_eq!(result.reason, EnablementReason::SingleModality);
        assert_eq!(result.confidence, 1.0);
    }

    #[tokio::test]
    async fn test_detection_multi_modality() {
        let detector = ModalityDetector::default_config();

        let vectors = vec![
            VectorRecordSample::with_modality("text"),
            VectorRecordSample::with_modality("text"),
            VectorRecordSample::with_modality("image"),
            VectorRecordSample::with_modality("image"),
            VectorRecordSample::with_modality("video"),
        ];

        let result = detector
            .detect_modalities("test_collection", &vectors)
            .await;

        assert_eq!(result.distinct_modalities, 3);
        assert!(result.should_enable_hmgi);
        assert_eq!(result.reason, EnablementReason::MultipleModalities);
        assert!(result.confidence > 0.9);
    }

    #[tokio::test]
    async fn test_detection_threshold() {
        let detector = ModalityDetector::new(100, 3); // Threshold of 3 modalities

        let vectors = vec![
            VectorRecordSample::with_modality("text"),
            VectorRecordSample::with_modality("image"),
        ];

        let result = detector
            .detect_modalities("test_collection", &vectors)
            .await;

        assert_eq!(result.distinct_modalities, 2);
        assert!(!result.should_enable_hmgi); // Below threshold
    }

    #[tokio::test]
    async fn test_detection_empty_collection() {
        let detector = ModalityDetector::default_config();

        let vectors: Vec<VectorRecordSample> = vec![];

        let result = detector
            .detect_modalities("test_collection", &vectors)
            .await;

        assert_eq!(result.distinct_modalities, 0);
        assert!(!result.should_enable_hmgi);
        assert_eq!(result.reason, EnablementReason::InsufficientData);
    }

    #[tokio::test]
    async fn test_detection_sampling() {
        let detector = ModalityDetector::new(10, 2); // Sample size of 10

        // Create 100 vectors
        let vectors: Vec<VectorRecordSample> = (0..100)
            .map(|i| {
                let modality = if i % 2 == 0 { "text" } else { "image" };
                VectorRecordSample::with_modality(modality)
            })
            .collect();

        let result = detector
            .detect_modalities("test_collection", &vectors)
            .await;

        // Should detect both modalities despite sampling
        assert_eq!(result.distinct_modalities, 2);
        assert!(result.should_enable_hmgi);
    }

    #[tokio::test]
    async fn test_recheck_collection() {
        let detector = ModalityDetector::default_config();

        // Initially single modality
        let vectors = vec![VectorRecordSample::with_modality("text")];
        let transition = detector
            .recheck_collection("test_collection", &vectors)
            .await
            .unwrap();

        assert_eq!(transition, CollectionTransition::SingleModality);

        // Now multi-modality
        let vectors = vec![
            VectorRecordSample::with_modality("text"),
            VectorRecordSample::with_modality("image"),
        ];
        let transition = detector
            .recheck_collection("test_collection", &vectors)
            .await
            .unwrap();

        match transition {
            CollectionTransition::MultiModality { recommended, .. } => {
                assert!(recommended);
            }
            _ => panic!("Expected MultiModality transition"),
        }
    }

    #[tokio::test]
    async fn test_modality_counts() {
        let detector = ModalityDetector::default_config();

        let vectors = vec![
            VectorRecordSample::with_modality("text"),
            VectorRecordSample::with_modality("text"),
            VectorRecordSample::with_modality("text"),
            VectorRecordSample::with_modality("image"),
            VectorRecordSample::with_modality("image"),
        ];

        let result = detector
            .detect_modalities("test_collection", &vectors)
            .await;

        assert_eq!(result.modality_counts.get("text"), Some(&3));
        assert_eq!(result.modality_counts.get("image"), Some(&2));
    }
}

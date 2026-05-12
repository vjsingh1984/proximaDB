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

//! Partition pruning using metadata bounds
//!
//! This module extends MetadataBounds with modality-specific tracking
//! for query-time partition pruning.

#![allow(dead_code)] // TODO: Remove as implementation progresses

use crate::cluster::shard::MetadataBounds;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// HMGI partition metadata - extends MetadataBounds with modality info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionMetadata {
    /// Base metadata bounds
    pub bounds: MetadataBounds,

    /// Modality tags present in this partition
    pub modalities: HashSet<String>,

    /// Vector count per modality
    pub modality_counts: HashMap<String, u64>,

    /// Last updated timestamp
    pub last_updated: i64,
}

impl PartitionMetadata {
    /// Create new empty partition metadata
    pub fn new() -> Self {
        Self {
            bounds: MetadataBounds::new(),
            modalities: HashSet::new(),
            modality_counts: HashMap::new(),
            last_updated: chrono::Utc::now().timestamp(),
        }
    }

    /// Add a modality to this partition
    pub fn add_modality(&mut self, modality: String) {
        self.modalities.insert(modality.clone());
        *self.modality_counts.entry(modality).or_insert(0) += 1;
        self.last_updated = chrono::Utc::now().timestamp();
    }

    /// Check if partition contains the given modality
    pub fn contains_modality(&self, modality: &str) -> bool {
        self.modalities.contains(modality)
    }

    /// Get the count for a specific modality
    pub fn modality_count(&self, modality: &str) -> u64 {
        self.modality_counts.get(modality).copied().unwrap_or(0)
    }

    /// Get total vector count across all modalities
    pub fn total_count(&self) -> u64 {
        self.modality_counts.values().sum()
    }

    /// Update the last updated timestamp
    pub fn touch(&mut self) {
        self.last_updated = chrono::Utc::now().timestamp();
    }
}

impl Default for PartitionMetadata {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pruning_by_modality() {
        let mut metadata = PartitionMetadata::new();
        metadata.add_modality("text".to_string());
        metadata.add_modality("image".to_string());

        assert!(metadata.contains_modality("text"));
        assert!(metadata.contains_modality("image"));
        assert!(!metadata.contains_modality("video"));
    }

    #[test]
    fn test_bounds_tracking() {
        let mut metadata = PartitionMetadata::new();

        metadata.add_modality("text".to_string());
        metadata.add_modality("text".to_string());
        metadata.add_modality("image".to_string());

        assert_eq!(metadata.modality_count("text"), 2);
        assert_eq!(metadata.modality_count("image"), 1);
        assert_eq!(metadata.modality_count("video"), 0);
        assert_eq!(metadata.total_count(), 3);
    }

    #[test]
    fn test_metadata_serialization() {
        let mut metadata = PartitionMetadata::new();
        metadata.add_modality("text".to_string());

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: PartitionMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(metadata.modalities, deserialized.modalities);
        assert_eq!(metadata.modality_counts, deserialized.modality_counts);
    }
}

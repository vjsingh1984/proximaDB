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

//! Modality tag extraction for HMGI partitioning
//!
//! This module provides the `ModalityExtractor` which determines which partition
//! a record belongs to based on its modality tag.

#![allow(dead_code)] // TODO: Remove as implementation progresses

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Modality tag extractor - determines which partition a record belongs to
///
/// Uses explicit + fallback strategy:
/// 1. Prefer `_modality` field when present
/// 2. Use fallback when field is missing
/// 3. Per-collection fallback takes precedence over global
#[derive(Debug, Clone)]
pub struct ModalityExtractor {
    /// Field name containing modality tag (default: "_modality")
    modality_field: String,

    /// Fallback modality if field is missing
    fallback_modality: String,

    /// Collection-specific fallback modality (per-collection override)
    collection_fallbacks: Arc<RwLock<HashMap<String, String>>>,
}

impl ModalityExtractor {
    /// Create a new modality extractor with default configuration
    pub fn new() -> Self {
        Self {
            modality_field: "_modality".to_string(),
            fallback_modality: "default".to_string(),
            collection_fallbacks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new modality extractor with custom configuration
    pub fn with_config(modality_field: String, fallback_modality: String) -> Self {
        Self {
            modality_field,
            fallback_modality,
            collection_fallbacks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set collection-specific fallback modality
    pub async fn set_collection_fallback(&self, collection_id: &str, fallback: String) {
        let mut fallbacks = self.collection_fallbacks.write().await;
        fallbacks.insert(collection_id.to_string(), fallback);
    }

    /// Get collection-specific fallback modality
    pub async fn get_collection_fallback(&self, collection_id: &str) -> Option<String> {
        let fallbacks = self.collection_fallbacks.read().await;
        fallbacks.get(collection_id).cloned()
    }

    /// Field name used to read explicit modality tags.
    pub fn modality_field(&self) -> &str {
        &self.modality_field
    }

    /// Extract modality tag from metadata
    ///
    /// Returns the modality tag from the metadata, using fallback if necessary.
    pub fn extract_modality(&self, metadata: &HashMap<String, serde_json::Value>) -> String {
        // Try explicit field first
        if let Some(value) = metadata.get(&self.modality_field)
            && let Some(tag) = value.as_str()
        {
            return tag.to_string();
        }

        // Fall back to default
        self.fallback_modality.clone()
    }

    /// Extract modality tag with collection-specific fallback
    pub async fn extract_modality_for_collection(
        &self,
        collection_id: &str,
        metadata: &HashMap<String, serde_json::Value>,
    ) -> String {
        // Try explicit field first
        if let Some(value) = metadata.get(&self.modality_field)
            && let Some(tag) = value.as_str()
        {
            return tag.to_string();
        }

        // Try collection-specific fallback
        if let Some(fallback) = self.get_collection_fallback(collection_id).await {
            return fallback;
        }

        // Fall back to global default
        self.fallback_modality.clone()
    }
}

impl Default for ModalityExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extraction_explicit_tag() {
        let extractor = ModalityExtractor::new();
        let mut metadata = HashMap::new();
        metadata.insert("_modality".to_string(), json!("text"));

        let tag = extractor.extract_modality(&metadata);
        assert_eq!(tag, "text");
    }

    #[test]
    fn test_extraction_fallback() {
        let extractor = ModalityExtractor::new();
        let metadata = HashMap::new();

        let tag = extractor.extract_modality(&metadata);
        assert_eq!(tag, "default");
    }

    #[tokio::test]
    async fn test_collection_fallback() {
        let extractor = ModalityExtractor::new();
        extractor
            .set_collection_fallback("test_collection", "image".to_string())
            .await;

        let metadata = HashMap::new();
        let tag = extractor
            .extract_modality_for_collection("test_collection", &metadata)
            .await;
        assert_eq!(tag, "image");

        // Global fallback for other collections
        let tag = extractor
            .extract_modality_for_collection("other_collection", &metadata)
            .await;
        assert_eq!(tag, "default");
    }

    #[test]
    fn test_extraction_complex_record() {
        let extractor = ModalityExtractor::new();
        let mut metadata = HashMap::new();
        metadata.insert("_modality".to_string(), json!("video"));
        metadata.insert("title".to_string(), json!("Test Video"));

        let tag = extractor.extract_modality(&metadata);
        assert_eq!(tag, "video");
    }
}

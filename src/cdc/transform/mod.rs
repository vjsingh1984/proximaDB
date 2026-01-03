/*
 * Copyright 2025 Vijaykumar Singh
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

//! CDC Transform Pipeline
//!
//! This module provides transformation capabilities for CDC events:
//! - Schema mapping and field transformations
//! - Embedding generation pipeline
//! - Filter rules for event routing
//!
//! ## Example
//!
//! ```rust,ignore
//! use proximadb::cdc::transform::{TransformPipeline, SchemaMapper, FilterRule};
//!
//! let pipeline = TransformPipeline::new()
//!     .with_schema_mapper(SchemaMapper::new()
//!         .rename_field("old_name", "new_name")
//!         .drop_field("unwanted"))
//!     .with_filter(FilterRule::include_collections(vec!["users", "products"]));
//!
//! let transformed = pipeline.transform(event)?;
//! ```

mod embedding;
mod filter;
mod schema;

pub use embedding::{EmbeddingConfig, EmbeddingPipeline, EmbeddingProvider};
pub use filter::{FilterAction, FilterRule, FilterRuleSet};
pub use schema::{FieldMapping, FieldTransform, SchemaMapper};

use crate::cdc::error::CdcResult;
use crate::cdc::event::ChangeEvent;

/// Transform pipeline for CDC events
pub struct TransformPipeline {
    /// Schema mappers
    schema_mappers: Vec<SchemaMapper>,
    /// Filter rules
    filters: Vec<FilterRuleSet>,
    /// Embedding pipeline (optional)
    embedding: Option<EmbeddingPipeline>,
    /// Transform order
    transform_order: TransformOrder,
}

/// Order of transformations
#[derive(Debug, Clone, Copy, Default)]
pub enum TransformOrder {
    /// Filter -> Schema -> Embed (default)
    #[default]
    FilterFirst,
    /// Schema -> Filter -> Embed
    SchemaFirst,
    /// Custom order
    Custom,
}

impl Default for TransformPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl TransformPipeline {
    /// Create a new empty transform pipeline
    pub fn new() -> Self {
        Self {
            schema_mappers: Vec::new(),
            filters: Vec::new(),
            embedding: None,
            transform_order: TransformOrder::FilterFirst,
        }
    }

    /// Add a schema mapper to the pipeline
    pub fn with_schema_mapper(mut self, mapper: SchemaMapper) -> Self {
        self.schema_mappers.push(mapper);
        self
    }

    /// Add a filter rule set to the pipeline
    pub fn with_filter(mut self, filter: FilterRuleSet) -> Self {
        self.filters.push(filter);
        self
    }

    /// Add an embedding pipeline
    pub fn with_embedding(mut self, embedding: EmbeddingPipeline) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set the transform order
    pub fn with_order(mut self, order: TransformOrder) -> Self {
        self.transform_order = order;
        self
    }

    /// Transform a change event
    ///
    /// Returns None if the event should be filtered out
    pub fn transform(&self, event: ChangeEvent) -> CdcResult<Option<ChangeEvent>> {
        match self.transform_order {
            TransformOrder::FilterFirst => self.transform_filter_first(event),
            TransformOrder::SchemaFirst => self.transform_schema_first(event),
            TransformOrder::Custom => self.transform_filter_first(event),
        }
    }

    /// Transform with filter-first order
    fn transform_filter_first(&self, mut event: ChangeEvent) -> CdcResult<Option<ChangeEvent>> {
        // 1. Apply filters
        if !self.should_include(&event) {
            return Ok(None);
        }

        // 2. Apply schema mappings
        for mapper in &self.schema_mappers {
            event = mapper.transform(event)?;
        }

        // 3. Apply embedding if configured
        if let Some(ref embedding) = self.embedding {
            event = embedding.process(event)?;
        }

        Ok(Some(event))
    }

    /// Transform with schema-first order
    fn transform_schema_first(&self, mut event: ChangeEvent) -> CdcResult<Option<ChangeEvent>> {
        // 1. Apply schema mappings
        for mapper in &self.schema_mappers {
            event = mapper.transform(event)?;
        }

        // 2. Apply filters
        if !self.should_include(&event) {
            return Ok(None);
        }

        // 3. Apply embedding if configured
        if let Some(ref embedding) = self.embedding {
            event = embedding.process(event)?;
        }

        Ok(Some(event))
    }

    /// Check if an event should be included based on filters
    fn should_include(&self, event: &ChangeEvent) -> bool {
        if self.filters.is_empty() {
            return true;
        }

        for filter in &self.filters {
            match filter.evaluate(event) {
                FilterAction::Include => return true,
                FilterAction::Exclude => return false,
                FilterAction::Continue => continue,
            }
        }

        true // Default to include if no filter matches
    }

    /// Transform a batch of events
    pub fn transform_batch(&self, events: Vec<ChangeEvent>) -> CdcResult<Vec<ChangeEvent>> {
        let mut results = Vec::with_capacity(events.len());

        for event in events {
            if let Some(transformed) = self.transform(event)? {
                results.push(transformed);
            }
        }

        Ok(results)
    }

    /// Check if pipeline has any transformations configured
    pub fn is_empty(&self) -> bool {
        self.schema_mappers.is_empty() && self.filters.is_empty() && self.embedding.is_none()
    }

    /// Get the number of schema mappers
    pub fn schema_mapper_count(&self) -> usize {
        self.schema_mappers.len()
    }

    /// Get the number of filter rules
    pub fn filter_count(&self) -> usize {
        self.filters.len()
    }

    /// Check if embedding is configured
    pub fn has_embedding(&self) -> bool {
        self.embedding.is_some()
    }
}

/// A transform that can be applied to change events
pub trait Transform: Send + Sync {
    /// Transform a change event
    fn transform(&self, event: ChangeEvent) -> CdcResult<ChangeEvent>;

    /// Get the transform name
    fn name(&self) -> &str;
}

/// Result of a transformation
#[derive(Debug, Clone)]
pub enum TransformResult {
    /// Event was transformed successfully
    Transformed(ChangeEvent),
    /// Event was filtered out
    Filtered,
    /// Transform error occurred
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::event::{Operation, SourceInfo};

    fn create_test_event() -> ChangeEvent {
        ChangeEvent::new(
            SourceInfo::postgres("testdb", "public", "test_server"),
            Operation::Insert,
            "public.users",
            "user_1",
        )
    }

    #[test]
    fn test_empty_pipeline() {
        let pipeline = TransformPipeline::new();
        assert!(pipeline.is_empty());

        let event = create_test_event();
        let result = pipeline.transform(event.clone()).unwrap();

        assert!(result.is_some());
        let transformed = result.unwrap();
        assert_eq!(transformed.collection, event.collection);
    }

    #[test]
    fn test_pipeline_with_filter() {
        let filter =
            FilterRuleSet::new().with_rule(FilterRule::include_collections(vec!["public.users"]));

        let pipeline = TransformPipeline::new().with_filter(filter);
        assert!(!pipeline.is_empty());

        let event = create_test_event();
        let result = pipeline.transform(event).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_pipeline_filter_exclude() {
        let filter =
            FilterRuleSet::new().with_rule(FilterRule::exclude_collections(vec!["public.users"]));

        let pipeline = TransformPipeline::new().with_filter(filter);

        let event = create_test_event();
        let result = pipeline.transform(event).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_transform_batch() {
        let pipeline = TransformPipeline::new();

        let events = vec![
            create_test_event(),
            create_test_event(),
            create_test_event(),
        ];

        let results = pipeline.transform_batch(events).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_transform_batch_with_filter() {
        let filter =
            FilterRuleSet::new().with_rule(FilterRule::include_operations(vec![Operation::Update]));

        let pipeline = TransformPipeline::new().with_filter(filter);

        let events = vec![
            create_test_event(), // Insert - will be filtered
            create_test_event(), // Insert - will be filtered
        ];

        let results = pipeline.transform_batch(events).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_pipeline_counts() {
        let pipeline = TransformPipeline::new()
            .with_schema_mapper(SchemaMapper::new())
            .with_schema_mapper(SchemaMapper::new())
            .with_filter(FilterRuleSet::new());

        assert_eq!(pipeline.schema_mapper_count(), 2);
        assert_eq!(pipeline.filter_count(), 1);
        assert!(!pipeline.has_embedding());
    }
}

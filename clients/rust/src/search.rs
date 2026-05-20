//! Search operations and fluent query builder for ProximaDB
//!
//! This module provides the `SearchBuilder` for building and executing
//! vector similarity searches with filtering and configuration options.
//!
//! # Examples
//!
//! ```rust,ignore
//! use proximadb_sdk::{ProximaClient, FilterBuilder};
//!
//! let client = ProximaClient::connect("http://localhost:5678")?;
//!
//! // Basic search
//! let results = client.collection("products")
//!     .search()
//!     .vector(&query)
//!     .top_k(10)
//!     .execute()
//!     .await?;
//!
//! // Search with string filter
//! let results = client.collection("products")
//!     .search()
//!     .vector(&query)
//!     .top_k(10)
//!     .filter("category = 'electronics'")
//!     .execute()
//!     .await?;
//!
//! // Search with fluent filter builder
//! let filter = FilterBuilder::new()
//!     .eq("category", "electronics")
//!     .gte("price", 100)
//!     .lte("price", 500)
//!     .build();
//!
//! let results = client.collection("products")
//!     .search()
//!     .vector(&query)
//!     .top_k(10)
//!     .with_filter(filter)
//!     .execute()
//!     .await?;
//!
//! // Search with inline filter methods
//! let results = client.collection("products")
//!     .search()
//!     .vector(&query)
//!     .top_k(10)
//!     .filter_eq("category", "electronics")
//!     .filter_range("price", 100, 500)
//!     .execute()
//!     .await?;
//! ```

use crate::error::{ProximaError, Result, SearchError};
use crate::filter::{Filter, FilterBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Search mode for controlling recall vs performance tradeoff
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// Exact search - 100% recall, searches all partitions
    #[default]
    Exact,
    /// Approximate search - faster with ~95% recall
    Approximate {
        /// Number of partitions to probe (None = auto)
        nprobe: Option<usize>,
    },
    /// Adaptive search - auto-selects based on dataset size
    Adaptive {
        /// Threshold for switching to approximate
        threshold: usize,
    },
}

impl SearchMode {
    /// Convert to string representation for API
    pub fn as_str(&self) -> String {
        match self {
            SearchMode::Exact => "exact".to_string(),
            SearchMode::Approximate { nprobe: None } => "approximate".to_string(),
            SearchMode::Approximate { nprobe: Some(n) } => format!("approximate:{n}"),
            SearchMode::Adaptive { threshold } => format!("adaptive:{threshold}"),
        }
    }
}

/// Builder for search queries
///
/// # Example
///
/// ```rust,ignore
/// let results = client.collection("embeddings")
///     .search()
///     .vector(&query_embedding)
///     .top_k(10)
///     .filter("category = 'tech'")
///     .mode(SearchMode::Approximate { nprobe: Some(5) })
///     .execute()
///     .await?;
///
/// for result in results {
///     println!("ID: {}, Score: {}", result.id, result.score);
/// }
/// ```
pub struct SearchBuilder<'a> {
    #[cfg(feature = "client")]
    client: Option<&'a crate::client::ProximaClient>,
    #[cfg(feature = "embedded")]
    db: Option<&'a crate::embedded::ProximaDB>,
    collection: String,
    vector: Option<Vec<f32>>,
    top_k: usize,
    filter: Option<String>,
    filter_builder: Option<FilterBuilder>,
    mode: SearchMode,
    include_vectors: bool,
    include_metadata: bool,
    min_score: Option<f32>,
    timeout_ms: Option<u64>,
}

impl<'a> SearchBuilder<'a> {
    /// Create a new search builder (client mode)
    #[cfg(feature = "client")]
    pub fn new_client(client: &'a crate::client::ProximaClient, collection: &str) -> Self {
        Self {
            client: Some(client),
            #[cfg(feature = "embedded")]
            db: None,
            collection: collection.to_string(),
            vector: None,
            top_k: 10,
            filter: None,
            filter_builder: None,
            mode: SearchMode::default(),
            include_vectors: false,
            include_metadata: true,
            min_score: None,
            timeout_ms: None,
        }
    }

    /// Create a new search builder (embedded mode)
    #[cfg(feature = "embedded")]
    pub fn new_embedded(db: &'a crate::embedded::ProximaDB, collection: &str) -> Self {
        Self {
            #[cfg(feature = "client")]
            client: None,
            db: Some(db),
            collection: collection.to_string(),
            vector: None,
            top_k: 10,
            filter: None,
            filter_builder: None,
            mode: SearchMode::default(),
            include_vectors: false,
            include_metadata: true,
            min_score: None,
            timeout_ms: None,
        }
    }

    /// Set the query vector
    pub fn vector(mut self, vector: &[f32]) -> Self {
        self.vector = Some(vector.to_vec());
        self
    }

    /// Set the query vector from owned Vec
    pub fn vector_owned(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }

    /// Set the number of results to return
    pub fn top_k(mut self, k: usize) -> Self {
        self.top_k = k;
        self
    }

    /// Alias for top_k
    pub fn limit(self, k: usize) -> Self {
        self.top_k(k)
    }

    /// Set a filter expression
    ///
    /// Supports simple expressions like:
    /// - `"category = 'tech'"`
    /// - `"timestamp > 1704067200"`
    /// - `"price >= 100 AND price <= 500"`
    pub fn filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = Some(filter.into());
        self
    }

    /// Set the search mode
    pub fn mode(mut self, mode: SearchMode) -> Self {
        self.mode = mode;
        self
    }

    /// Use exact search (100% recall)
    pub fn exact(mut self) -> Self {
        self.mode = SearchMode::Exact;
        self
    }

    /// Use approximate search for faster results
    pub fn approximate(mut self) -> Self {
        self.mode = SearchMode::Approximate { nprobe: None };
        self
    }

    /// Use approximate search with specific nprobe value
    pub fn approximate_with_nprobe(mut self, nprobe: usize) -> Self {
        self.mode = SearchMode::Approximate {
            nprobe: Some(nprobe),
        };
        self
    }

    /// Use adaptive search mode
    pub fn adaptive(mut self, threshold: usize) -> Self {
        self.mode = SearchMode::Adaptive { threshold };
        self
    }

    /// Include vectors in results
    pub fn include_vectors(mut self, include: bool) -> Self {
        self.include_vectors = include;
        self
    }

    /// Include metadata in results
    pub fn include_metadata(mut self, include: bool) -> Self {
        self.include_metadata = include;
        self
    }

    /// Set minimum score threshold
    pub fn min_score(mut self, score: f32) -> Self {
        self.min_score = Some(score);
        self
    }

    /// Set request timeout in milliseconds
    pub fn timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout_ms = Some(timeout);
        self
    }

    /// Set timeout in seconds (convenience method)
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_ms = Some(secs * 1000);
        self
    }

    /// Apply a pre-built filter
    pub fn with_filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter.to_expression());
        self
    }

    /// Add an equality filter condition
    pub fn filter_eq(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).eq(field, value);
        }
        self
    }

    /// Add a not-equal filter condition
    pub fn filter_ne(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).ne(field, value);
        }
        self
    }

    /// Add a greater-than filter condition
    pub fn filter_gt(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).gt(field, value);
        }
        self
    }

    /// Add a greater-than-or-equal filter condition
    pub fn filter_gte(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).gte(field, value);
        }
        self
    }

    /// Add a less-than filter condition
    pub fn filter_lt(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).lt(field, value);
        }
        self
    }

    /// Add a less-than-or-equal filter condition
    pub fn filter_lte(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).lte(field, value);
        }
        self
    }

    /// Add a range filter condition (inclusive)
    pub fn filter_range<V: Into<Value> + Clone>(
        mut self,
        field: impl Into<String>,
        min: V,
        max: V,
    ) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).range(field, min, max);
        }
        self
    }

    /// Add an IN filter condition
    pub fn filter_in<V: Into<Value>>(mut self, field: impl Into<String>, values: Vec<V>) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).in_list(field, values);
        }
        self
    }

    /// Add a contains filter condition
    pub fn filter_contains(mut self, field: impl Into<String>, value: impl Into<Value>) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).contains(field, value);
        }
        self
    }

    /// Add an exists filter condition
    pub fn filter_exists(mut self, field: impl Into<String>) -> Self {
        self.ensure_filter_builder();
        if let Some(ref mut fb) = self.filter_builder {
            *fb = std::mem::take(fb).exists(field);
        }
        self
    }

    /// Helper to ensure filter builder exists
    fn ensure_filter_builder(&mut self) {
        if self.filter_builder.is_none() {
            self.filter_builder = Some(FilterBuilder::new());
        }
    }

    /// Build the final filter expression (combines string filter and builder)
    fn build_filter(&self) -> Option<String> {
        // If there's a filter builder, use its expression
        if let Some(ref fb) = self.filter_builder {
            let filter = fb.clone().build();
            return Some(filter.to_expression());
        }
        // Otherwise use the string filter
        self.filter.clone()
    }

    /// Execute the search (async, client mode)
    #[cfg(feature = "client")]
    pub async fn execute(self) -> Result<Vec<SearchResult>> {
        let client = self
            .client
            .ok_or_else(|| ProximaError::Internal("No client reference for search".to_string()))?;

        // Build the filter expression before moving self.vector
        let filter_expr = self.build_filter();

        let vector = self.vector.ok_or_else(|| {
            ProximaError::Search(SearchError::InvalidFilter {
                reason: "query vector is required".to_string(),
            })
        })?;

        if self.top_k == 0 || self.top_k > 10000 {
            return Err(ProximaError::Search(SearchError::InvalidTopK {
                value: self.top_k,
                max: 10000,
            }));
        }

        let request = SearchRequest {
            collection: self.collection,
            vector,
            top_k: self.top_k,
            filter: filter_expr,
            search_mode: Some(self.mode.as_str()),
            include_vectors: self.include_vectors,
            include_metadata: self.include_metadata,
            timeout_ms: self.timeout_ms,
        };

        let url = format!(
            "{}/api/v2/collections/{}/search",
            client.url(),
            request.collection
        );
        let response: SearchResponse = client.post(&url, &request).await?;

        let mut results = response.results;

        // Apply min_score filter if set
        if let Some(min) = self.min_score {
            results.retain(|r| r.score >= min);
        }

        Ok(results)
    }

    /// Execute the search (sync, embedded mode)
    #[cfg(feature = "embedded")]
    pub fn execute_sync(self) -> Result<Vec<SearchResult>> {
        let db = self.db.ok_or_else(|| {
            ProximaError::Internal("No embedded DB reference for search".to_string())
        })?;

        // Build the filter expression before moving self.vector
        let filter_expr = self.build_filter();

        let vector = self.vector.ok_or_else(|| {
            ProximaError::Search(SearchError::InvalidFilter {
                reason: "query vector is required".to_string(),
            })
        })?;

        if self.top_k == 0 || self.top_k > 10000 {
            return Err(ProximaError::Search(SearchError::InvalidTopK {
                value: self.top_k,
                max: 10000,
            }));
        }

        let mut results =
            db.search_internal(&self.collection, vector, self.top_k, filter_expr, self.mode)?;

        // Apply min_score filter if set
        if let Some(min) = self.min_score {
            results.retain(|r| r.score >= min);
        }

        Ok(results)
    }
}

/// A single search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Vector ID
    pub id: String,
    /// Similarity score (interpretation depends on metric)
    pub score: f32,
    /// Associated metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Optional vector data (if include_vectors was set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
}

impl SearchResult {
    /// Create a new search result
    pub fn new(id: impl Into<String>, score: f32) -> Self {
        Self {
            id: id.into(),
            score,
            metadata: HashMap::new(),
            vector: None,
        }
    }

    /// Add metadata to the result
    pub fn with_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata = metadata;
        self
    }

    /// Add a single metadata field
    pub fn with_meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Get a metadata value by key
    pub fn get_meta(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(|s| s.as_str())
    }
}

// Request/Response types for HTTP API

#[derive(Debug, Serialize)]
struct SearchRequest {
    #[serde(skip_serializing)]
    collection: String,
    vector: Vec<f32>,
    top_k: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_mode: Option<String>,
    #[serde(default)]
    include_vectors: bool,
    #[serde(default)]
    include_metadata: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_mode_as_str() {
        assert_eq!(SearchMode::Exact.as_str(), "exact");
        assert_eq!(
            SearchMode::Approximate { nprobe: None }.as_str(),
            "approximate"
        );
        assert_eq!(
            SearchMode::Approximate { nprobe: Some(5) }.as_str(),
            "approximate:5"
        );
        assert_eq!(
            SearchMode::Adaptive { threshold: 10000 }.as_str(),
            "adaptive:10000"
        );
    }

    #[test]
    fn test_search_result() {
        let result = SearchResult::new("vec_1", 0.95)
            .with_meta("category", "tech")
            .with_meta("source", "api");

        assert_eq!(result.id, "vec_1");
        assert_eq!(result.score, 0.95);
        assert_eq!(result.get_meta("category"), Some("tech"));
        assert_eq!(result.get_meta("source"), Some("api"));
        assert_eq!(result.get_meta("missing"), None);
    }
}

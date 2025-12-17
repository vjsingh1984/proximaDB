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

//! Request Tracing Utilities
//!
//! This module provides consistent request ID generation and tracing span creation
//! for observability across REST and gRPC protocols.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use crate::utils::tracing::{RequestContext, create_request_context};
//!
//! // In REST handler - extract from header or generate
//! let ctx = create_request_context(headers.get("x-request-id"));
//!
//! // In gRPC handler - extract from metadata or generate
//! let ctx = create_request_context(metadata.get("x-request-id"));
//!
//! // Create a tracing span
//! let _span = ctx.create_span("vector_search");
//! ```

use crate::utils::uuid::Uuid;
use tracing::{info_span, Span};

/// Request context containing correlation IDs for distributed tracing
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Unique identifier for this request (UUID v4)
    pub request_id: String,
    /// Optional parent trace ID for distributed tracing
    pub trace_id: Option<String>,
    /// Optional span ID for distributed tracing
    pub span_id: Option<String>,
}

impl RequestContext {
    /// Create a new request context with a generated request ID
    pub fn new() -> Self {
        Self {
            request_id: Uuid::new_v4().to_string(),
            trace_id: None,
            span_id: None,
        }
    }

    /// Create a request context with a specific request ID
    pub fn with_request_id(request_id: String) -> Self {
        Self {
            request_id,
            trace_id: None,
            span_id: None,
        }
    }

    /// Create a request context from an optional header value
    ///
    /// If the header is present and non-empty, use it as the request ID.
    /// Otherwise, generate a new UUID v4.
    pub fn from_header(header_value: Option<&str>) -> Self {
        let request_id = header_value
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        Self {
            request_id,
            trace_id: None,
            span_id: None,
        }
    }

    /// Set trace ID for distributed tracing
    pub fn with_trace_id(mut self, trace_id: String) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    /// Set span ID for distributed tracing
    pub fn with_span_id(mut self, span_id: String) -> Self {
        self.span_id = Some(span_id);
        self
    }

    /// Create an info-level tracing span for an operation
    ///
    /// The span includes:
    /// - `request_id`: The correlation ID for this request
    /// - `operation`: The name of the operation being performed
    /// - `trace_id` (optional): Parent trace ID for distributed tracing
    pub fn create_span(&self, operation: &str) -> Span {
        if let Some(ref trace_id) = self.trace_id {
            info_span!(
                "api_request",
                request_id = %self.request_id,
                operation = %operation,
                trace_id = %trace_id
            )
        } else {
            info_span!(
                "api_request",
                request_id = %self.request_id,
                operation = %operation
            )
        }
    }

    /// Create a span for a collection operation
    pub fn create_collection_span(&self, operation: &str, collection_id: &str) -> Span {
        info_span!(
            "collection_operation",
            request_id = %self.request_id,
            operation = %operation,
            collection_id = %collection_id
        )
    }

    /// Create a span for a vector operation
    pub fn create_vector_span(
        &self,
        operation: &str,
        collection_id: &str,
        vector_count: Option<usize>,
    ) -> Span {
        if let Some(count) = vector_count {
            info_span!(
                "vector_operation",
                request_id = %self.request_id,
                operation = %operation,
                collection_id = %collection_id,
                vector_count = %count
            )
        } else {
            info_span!(
                "vector_operation",
                request_id = %self.request_id,
                operation = %operation,
                collection_id = %collection_id
            )
        }
    }

    /// Create a span for a search operation
    pub fn create_search_span(
        &self,
        collection_id: &str,
        top_k: usize,
        has_filter: bool,
    ) -> Span {
        info_span!(
            "search_operation",
            request_id = %self.request_id,
            operation = "search",
            collection_id = %collection_id,
            top_k = %top_k,
            has_filter = %has_filter
        )
    }

    /// Create a span for a SQL operation
    pub fn create_sql_span(&self, query_preview: &str) -> Span {
        info_span!(
            "sql_operation",
            request_id = %self.request_id,
            operation = "sql_execute",
            query_preview = %query_preview
        )
    }

    /// Create a span for a graph operation
    pub fn create_graph_span(&self, operation: &str, graph_id: &str) -> Span {
        info_span!(
            "graph_operation",
            request_id = %self.request_id,
            operation = %operation,
            graph_id = %graph_id
        )
    }
}

impl Default for RequestContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a request context from an optional header value
///
/// Convenience function for extracting request ID from HTTP headers or gRPC metadata.
pub fn create_request_context(header_value: Option<&str>) -> RequestContext {
    RequestContext::from_header(header_value)
}

/// Extract request ID from a string, or generate a new one if empty/None
pub fn extract_or_generate_request_id(value: Option<&str>) -> String {
    value
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

/// HTTP header name for request ID
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// gRPC metadata key for request ID
pub const REQUEST_ID_METADATA_KEY: &str = "x-request-id";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_context_new() {
        let ctx = RequestContext::new();
        assert!(!ctx.request_id.is_empty());
        assert!(ctx.trace_id.is_none());
        assert!(ctx.span_id.is_none());
    }

    #[test]
    fn test_request_context_from_header() {
        // With header value
        let ctx = RequestContext::from_header(Some("custom-request-id-123"));
        assert_eq!(ctx.request_id, "custom-request-id-123");

        // Without header value
        let ctx = RequestContext::from_header(None);
        assert!(!ctx.request_id.is_empty());
        assert!(ctx.request_id.len() == 36); // UUID format

        // With empty header value
        let ctx = RequestContext::from_header(Some(""));
        assert!(!ctx.request_id.is_empty());
        assert!(ctx.request_id.len() == 36); // UUID format
    }

    #[test]
    fn test_extract_or_generate_request_id() {
        // With value
        let id = extract_or_generate_request_id(Some("my-request-id"));
        assert_eq!(id, "my-request-id");

        // Without value
        let id = extract_or_generate_request_id(None);
        assert!(!id.is_empty());
        assert!(id.len() == 36); // UUID format

        // With empty value
        let id = extract_or_generate_request_id(Some(""));
        assert!(!id.is_empty());
        assert!(id.len() == 36); // UUID format
    }

    #[test]
    fn test_request_context_with_trace_id() {
        let ctx = RequestContext::new()
            .with_trace_id("trace-123".to_string())
            .with_span_id("span-456".to_string());

        assert_eq!(ctx.trace_id, Some("trace-123".to_string()));
        assert_eq!(ctx.span_id, Some("span-456".to_string()));
    }
}

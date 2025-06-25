//! Search result types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::core::foundation::BaseResult;

/// Unified search result structure - replaces 13+ duplicates across schema_types and other files
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Vector/document identifier
    pub id: String,
    /// Alternative identifier field for compatibility with schema_types
    pub vector_id: Option<String>,
    /// Similarity score (higher = more similar)
    pub score: f32,
    /// Distance value (lower = more similar, if different from score)  
    pub distance: Option<f32>,
    /// Result rank (1-based)
    pub rank: Option<i32>,
    /// Original vector data (optional for bandwidth optimization)
    pub vector: Option<Vec<f32>>,
    /// Associated metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Debug information for result
    pub debug_info: Option<SearchDebugInfo>,
    
    // Additional fields for compatibility with existing code
    /// Index path for result tracking
    pub index_path: Option<String>,
    /// Collection identifier
    pub collection_id: Option<String>,
    /// Creation timestamp
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Debug information for search results
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchDebugInfo {
    /// Algorithm used for this result
    pub algorithm: String,
    /// Number of candidates evaluated
    pub candidates_evaluated: u32,
    /// Time spent processing this result (microseconds)
    pub processing_time_us: u64,
}

impl SearchResult {
    /// Create a basic search result
    pub fn new(id: String, score: f32) -> Self {
        Self {
            id,
            vector_id: None,
            score,
            distance: None,
            rank: None,
            vector: None,
            metadata: HashMap::new(),
            debug_info: None,
            index_path: None,
            collection_id: None,
            created_at: None,
        }
    }
    
    /// Create search result with metadata
    pub fn with_metadata(
        id: String,
        score: f32,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            id,
            vector_id: None,
            score,
            distance: None,
            rank: None,
            vector: None,
            metadata,
            debug_info: None,
            index_path: None,
            collection_id: None,
            created_at: None,
        }
    }
    
    /// Add vector data to result
    pub fn with_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }
    
    /// Add debug information
    pub fn with_debug_info(mut self, debug_info: SearchDebugInfo) -> Self {
        self.debug_info = Some(debug_info);
        self
    }
}

/// Collection of search results with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultSet {
    /// Individual search results
    pub results: Vec<SearchResult>,
    /// Total number of matching documents (before pagination)
    pub total_count: u64,
    /// Query that generated these results
    pub query_id: Option<String>,
    /// Processing time for entire query (microseconds)
    pub processing_time_us: u64,
    /// Algorithm used for search
    pub algorithm: String,
    /// Additional query metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

impl BaseResult<Vec<SearchResult>> for SearchResultSet {
    fn is_success(&self) -> bool {
        true // SearchResultSet represents successful search
    }
    
    fn data(&self) -> Option<&Vec<SearchResult>> {
        Some(&self.results)
    }
    
    fn error(&self) -> Option<&str> {
        None // SearchResultSet doesn't contain errors
    }
    
    fn processing_time_us(&self) -> Option<u64> {
        Some(self.processing_time_us)
    }
}
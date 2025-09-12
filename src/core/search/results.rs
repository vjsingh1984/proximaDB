//! Search result types

use crate::compute::distance_computation::engine::SimilarityResult;
use crate::compute::quantization::unified::UnifiedQuantizationLevel;
use crate::proto::proximadb_v1::SourceContent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Convert SqlValue map to serde_json::Value map for compatibility
fn convert_sql_value_map_to_json_map(
    sql_map: &HashMap<String, crate::proto::proximadb_v1::SqlValue>,
) -> HashMap<String, serde_json::Value> {
    sql_map
        .iter()
        .filter_map(|(key, sql_value)| {
            use crate::proto::proximadb_v1::sql_value::Value;
            let json_value = match &sql_value.value {
                Some(Value::StringValue(s)) => serde_json::Value::String(s.clone()),
                Some(Value::NumberValue(n)) => serde_json::Value::Number(
                    serde_json::Number::from_f64(*n)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                ),
                Some(Value::BoolValue(b)) => serde_json::Value::Bool(*b),
                Some(Value::Int64Value(i)) => serde_json::Value::Number(
                    serde_json::Number::from(*i)
                ),
                Some(Value::BytesValue(_)) => serde_json::Value::String("[binary data]".to_string()),
                Some(Value::NullValue(_)) => serde_json::Value::Null,
                Some(Value::ArrayValue(_)) => serde_json::Value::String("[array]".to_string()),
                Some(Value::ObjectValue(_)) => serde_json::Value::String("[object]".to_string()),
                None => return None,
            };
            Some((key.clone(), json_value))
        })
        .collect()
}

/// Convert serde_json::Value map to SqlValue map for compatibility
fn convert_json_map_to_sql_value_map(
    json_map: HashMap<String, serde_json::Value>,
) -> HashMap<String, crate::proto::proximadb_v1::SqlValue> {
    json_map
        .into_iter()
        .map(|(key, json_value)| {
            let sql_value = match json_value {
                serde_json::Value::String(s) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)),
                },
                serde_json::Value::Number(n) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(
                        n.as_f64().unwrap_or(0.0),
                    )),
                },
                serde_json::Value::Bool(b) => crate::proto::proximadb_v1::SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)),
                },
                _ => crate::proto::proximadb_v1::SqlValue { value: None },
            };
            (key, sql_value)
        })
        .collect()
}

/// DEPRECATED: Legacy search result structure - Use OptimizedSearchRecord instead
/// 
/// **MIGRATION NOTICE**: This type is deprecated in favor of OptimizedSearchRecord
/// which uses SqlValue for metadata (better performance, proto v1 compatibility).
/// 
/// Legacy unified search result structure - replaces 13+ duplicates across schema_types and other files
#[derive(Debug, Clone, PartialEq, Default)]
#[deprecated(since = "1.0.0", note = "Use OptimizedSearchRecord with SqlValue metadata")]
pub struct InternalSearchResult {
    /// Vector/document identifier
    pub id: String,
    /// Alternative identifier field for compatibility with schema_types
    pub vector_id: Option<String>,
    /// Similarity score (higher = more similar)
    pub score: f32,
    /// Distance value (lower = more similar, if different from score)  
    pub similarity: Option<f32>,
    /// Result rank (1-based) - use u16 since ranks are typically small
    /// Original vector data (optional for bandwidth optimization)
    pub vector: Option<Vec<f32>>,
    /// Associated metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Debug information for result
    pub debug_info: Option<SearchDebugInfo>,
    /// Version for MVCC (multi-version concurrency control) - use u32 to match proto VectorRecord  
    pub version: Option<u32>,
    /// Record timestamp for version resolution (earliest wins for same version) - use u32 for seconds since epoch (unsigned)
    pub timestamp: Option<u32>,
    /// Update timestamp (if different from creation timestamp)
    pub updated_at: Option<u32>,
    /// TTL expiration timestamp (seconds since epoch)  
    pub expires_at: Option<u32>,
    /// Original source content that generated this vector (essential for SearchVectorRecord)
    pub source: Option<SourceContent>,
    /// Expanded context for RAG applications (surrounding chunks)
    pub expanded_context: Vec<SourceContent>,

    // Unified search pipeline integration
    /// Semantic distance information with metric awareness (replaces multiple adapters)
    pub semantic_similarity: Option<SimilarityResult>,
    /// Quantization information if applicable
    pub quantization_info: Option<QuantizationInfo>,
    /// Engine-specific optimization stats (replaces multiple result types)
    pub engine_stats: Option<EngineStats>,

    // Additional fields for compatibility with existing code
    /// Index path for result tracking
    pub index_path: Option<String>,
    // Creation timestamp (as DateTime) - removed as duplicate, use the u32 timestamp instead
    // pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Debug information for search results
#[derive(Debug, Clone)]
pub struct SearchDebugInfo {
    /// Algorithm used for this result
    pub algorithm: String,
    /// Number of candidates evaluated
    pub candidates_evaluated: u32,
    /// Time spent processing this result (microseconds)
    pub processing_time_us: u64,
}

/// Quantization information for search results
#[derive(Debug, Clone)]
pub struct QuantizationInfo {
    /// Quantization level used
    pub level: UnifiedQuantizationLevel,
    /// Compression ratio achieved
    pub compression_ratio: f32,
    /// Accuracy retained (percentage)
    pub accuracy_retained: f32,
    /// Column name in Parquet file
    pub name: Option<String>,
}

/// Engine-specific optimization statistics
#[derive(Debug, Clone)]
pub struct EngineStats {
    /// Strategy used (DirectArrow, MetadataFiltered, QuantizedTwoStage, Hybrid)
    pub strategy_used: String,
    /// Bytes read from storage
    pub bytes_read: usize,
    /// Number of seek operations
    pub seek_operations: usize,
    /// Number of HTTP range requests (cloud storage)
    pub range_requests: usize,
    /// Cache hit/miss statistics
    pub cache_hits: usize,
    pub cache_misses: usize,
    /// Deduplication savings
    pub deduplication_savings: usize,
}

impl InternalSearchResult {
    /// Create a basic search result
    #[deprecated(since = "1.0.0", note = "Use OptimizedSearchRecord::new() instead")]
    pub fn new(id: String, similarity: f32) -> Self {
        Self {
            id,
            vector_id: None,
            score: similarity,
            similarity: None,
            // rank removed -  None,
            vector: None,
            metadata: HashMap::new(),
            debug_info: None,
            version: None,
            timestamp: None,
            updated_at: None,
            expires_at: None,
            source: None,
            expanded_context: Vec::new(),
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
        }
    }

    /// Create search result with metadata
    pub fn with_metadata(
        id: String,
        similarity: f32,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            id,
            vector_id: None,
            score: similarity,
            similarity: None,
            // rank removed -  None,
            vector: None,
            metadata,
            debug_info: None,
            version: None,
            timestamp: None,
            updated_at: None,
            expires_at: None,
            source: None,
            expanded_context: Vec::new(),
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
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

    /// Add semantic distance information (eliminates adapter conversions)
    pub fn with_semantic_distance(mut self, semantic_distance: SimilarityResult) -> Self {
        self.semantic_similarity = Some(semantic_distance.clone());
        // Update core score/distance fields for compatibility
        self.score = semantic_distance.normalized_score;
        self.similarity = Some(semantic_distance.rank_value);
        self
    }

    /// Add quantization information for Parquet column integration
    pub fn with_quantization_info(mut self, quantization_info: QuantizationInfo) -> Self {
        self.quantization_info = Some(quantization_info);
        self
    }

    /// Add engine-specific optimization statistics (eliminates multiple result types)
    pub fn with_engine_stats(mut self, engine_stats: EngineStats) -> Self {
        self.engine_stats = Some(engine_stats);
        self
    }

    /// Create search result directly from semantic distance computation
    pub fn from_semantic_distance(
        id: String,
        vector_id: Option<String>,
        semantic_similarity: SimilarityResult,
        vector: Option<Vec<f32>>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            id,
            vector_id,
            score: semantic_similarity.normalized_score,
            similarity: Some(semantic_similarity.rank_value),
            vector,
            metadata,
            debug_info: None,
            version: None,
            timestamp: None,
            updated_at: None,
            expires_at: None,
            source: None,
            expanded_context: Vec::new(),
            semantic_similarity: Some(semantic_similarity),
            quantization_info: None,
            engine_stats: None,
            index_path: None,
        }
    }

    /// Create search result from VectorRecord with score - preserves all source information
    pub fn from_vector_record(
        record: &crate::proto::proximadb_v1::VectorRecord,
        score: f32,
    ) -> Self {
        // Convert SqlValue metadata to serde_json::Value for InternalSearchResult
        let metadata = convert_sql_value_map_to_json_map(&record.metadata);

        Self {
            id: record.id.clone(),
            vector_id: Some(record.id.clone()),
            score,
            similarity: None,
            vector: Some(record.vector.clone()),
            metadata,
            debug_info: None,
            version: record.version.map(|v| v as u32),
            timestamp: Some(record.timestamp as u32),
            updated_at: record.updated_at.map(|v| v as u32),
            expires_at: record.expires_at.map(|v| v as u32),
            source: record.source.as_ref().map(|s| SourceContent {
                data: Some(crate::proto::proximadb_v1::source_content::Data::TextContent(s.clone())),
            }), // Convert String to SourceContent
            expanded_context: Vec::new(),  // Empty by default, can be populated later
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
        }
    }

    /// Create a simple search result with just ID, score and defaults for other fields
    pub fn simple(id: String, similarity: f32) -> Self {
        Self {
            id,
            vector_id: None,
            score: similarity,
            similarity: None,
            // rank removed -  None,
            vector: None,
            metadata: HashMap::new(),
            debug_info: None,
            version: None,
            timestamp: None,
            updated_at: None,
            expires_at: None,
            source: None,
            expanded_context: Vec::new(),
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
        }
    }

    /// Convert to SearchVectorRecord for proto response - handles include flags properly
    pub fn to_search_vector_record(
        &self,
        include_vector: bool,
        include_metadata: bool,
        include_source: bool,
    ) -> crate::proto::proximadb_v1::SearchVectorRecord {
        use crate::proto::proximadb_v1::{MetadataItem, SearchVectorRecord, metadata_item::Value};

        // Convert metadata to SqlValue format for SearchVectorRecord
        let metadata = if include_metadata {
            convert_json_map_to_sql_value_map(self.metadata.clone())
        } else {
            HashMap::new()
        };

        SearchVectorRecord {
            id: self.id.clone(),
            vector: if include_vector {
                self.vector.clone().unwrap_or_default()
            } else {
                Vec::new()
            },
            metadata,
            score: self.score as f64,
            similarity: self.similarity,
            version: self.version.map(|v| v as i64),
            timestamp: self.timestamp.map(|v| v as i64),
            source: if include_source {
                self.source.as_ref().and_then(|sc| match &sc.data {
                    Some(crate::proto::proximadb_v1::source_content::Data::TextContent(text)) => Some(text.clone()),
                    Some(crate::proto::proximadb_v1::source_content::Data::BinaryContent(_)) => Some("[Binary Content]".to_string()),
                    Some(crate::proto::proximadb_v1::source_content::Data::ExternalReference(_)) => Some("[External Reference]".to_string()),
                    None => None,
                })
            } else {
                None
            }, // Convert SourceContent to String
            expanded_context: if include_source {
                self.expanded_context
                    .iter()
                    .map(|sc| match &sc.data {
                        Some(crate::proto::proximadb_v1::source_content::Data::TextContent(text)) => {
                            text.clone()
                        }
                        Some(crate::proto::proximadb_v1::source_content::Data::BinaryContent(_)) => {
                            "[Binary Content]".to_string()
                        }
                        Some(crate::proto::proximadb_v1::source_content::Data::ExternalReference(_)) => {
                            "[External Reference]".to_string()
                        }
                        None => "[Empty Content]".to_string(),
                    })
                    .collect()
            } else {
                Vec::new()
            },
            semantic_similarity: self.semantic_similarity.as_ref().map(|s| s.similarity_score),
            quantization_info: self.quantization_info.as_ref().map(|q| format!("{:?}", q)),
            engine_stats: self.engine_stats.as_ref().map(|stats| {
                let mut map = HashMap::new();
                map.insert("strategy".to_string(), stats.strategy_used.clone());
                map.insert("bytes_read".to_string(), stats.bytes_read.to_string());
                map
            }).unwrap_or_default(),
            index_path: self.index_path.clone(),
        }
    }

    /// Convert to v1 SearchVectorRecord for proto response
    pub fn to_search_vector_record_v1(
        &self,
        include_vector: bool,
        include_metadata: bool,
    ) -> crate::proto::proximadb_v1::SearchVectorRecord {
        // Convert metadata map to v1 SqlValue map if requested
        let mut metadata: std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue> =
            std::collections::HashMap::new();
        if include_metadata {
            for (key, value) in &self.metadata {
                let sql_value = match value {
                    serde_json::Value::String(s) => crate::proto::proximadb_v1::SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                            s.clone(),
                        )),
                    },
                    serde_json::Value::Number(n) => crate::proto::proximadb_v1::SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(
                            n.as_f64().unwrap_or(0.0),
                        )),
                    },
                    serde_json::Value::Bool(b) => crate::proto::proximadb_v1::SqlValue {
                        value: Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(*b)),
                    },
                    _ => crate::proto::proximadb_v1::SqlValue { value: None },
                };
                metadata.insert(key.clone(), sql_value);
            }
        }

        crate::proto::proximadb_v1::SearchVectorRecord {
            id: self.id.clone(),
            vector: if include_vector {
                self.vector.clone().unwrap_or_default()
            } else {
                Vec::new()
            },
            metadata,
            score: self.score as f64,
            version: self.version.map(|v| v as i64),
            similarity: self.similarity,
            timestamp: self.timestamp.map(|v| v as i64),
            source: self.source.as_ref().and_then(|sc| match &sc.data {
                Some(crate::proto::proximadb_v1::source_content::Data::TextContent(text)) => Some(text.clone()),
                Some(crate::proto::proximadb_v1::source_content::Data::BinaryContent(_)) => Some("[Binary Content]".to_string()),
                Some(crate::proto::proximadb_v1::source_content::Data::ExternalReference(_)) => Some("[External Reference]".to_string()),
                None => None,
            }),
            expanded_context: self.expanded_context
                .iter()
                .map(|sc| match &sc.data {
                    Some(crate::proto::proximadb_v1::source_content::Data::TextContent(text)) => text.clone(),
                    Some(crate::proto::proximadb_v1::source_content::Data::BinaryContent(_)) => "[Binary Content]".to_string(),
                    Some(crate::proto::proximadb_v1::source_content::Data::ExternalReference(_)) => "[External Reference]".to_string(),
                    None => "[Empty Content]".to_string(),
                })
                .collect(),
            semantic_similarity: self.semantic_similarity.as_ref().map(|s| s.similarity_score),
            quantization_info: self.quantization_info.as_ref().map(|q| format!("{:?}", q)),
            engine_stats: self.engine_stats.as_ref().map(|stats| {
                let mut map = HashMap::new();
                map.insert("strategy".to_string(), stats.strategy_used.clone());
                map.insert("bytes_read".to_string(), stats.bytes_read.to_string());
                map
            }).unwrap_or_default(),
            index_path: self.index_path.clone(),
        }
    }

    /// ==================== UNIFIED SIMILARITY SCORING ====================
    /// Standardized distance-to-similarity conversion for consistent ranking
    /// across all storage engines and WAL search

    /// Convert raw distance to normalized similarity score (higher = more similar)
    /// This ensures consistent ranking when merging results from different sources
    ///
    /// Supports all 13 ProximaDB distance metrics with semantic normalization:
    /// Core: Cosine, Euclidean, DotProduct, Manhattan  
    /// Extended: Hamming, Jaccard, Chebyshev, Canberra, Minkowski, Angular, BrayCurtis, Hellinger, Custom
    pub fn standardized_distance_to_similarity(
        distance: f32,
        metric: &crate::compute::distance_computation::DistanceMetric,
    ) -> f32 {
        use crate::compute::distance_computation::DistanceMetric;
        match metric {
            DistanceMetric::Cosine => {
                // Cosine distance is in [0, 2], similarity = 1 - distance/2 for normalized range
                if distance.is_infinite() {
                    0.0 // Zero vectors get worst similarity score
                } else {
                    (1.0 - (distance.max(0.0).min(2.0) / 2.0)).max(0.0)
                }
            }
            DistanceMetric::Euclidean => {
                // Euclidean distance: similarity = 1 / (1 + distance)
                1.0 / (1.0 + distance)
            }
            DistanceMetric::DotProduct => {
                // Dot product is similarity metric (higher = more similar)
                // For normalized vectors, dot product is in [-1, 1], convert to [0, 1]
                (distance + 1.0) / 2.0
            }
            DistanceMetric::Manhattan => {
                // Manhattan distance: similarity = 1 / (1 + distance)
                1.0 / (1.0 + distance)
            }
            DistanceMetric::Hamming => {
                // Hamming distance: number of differing positions
                // Similarity = 1 - (distance / max_possible_distance)
                // For continuous vectors, use exponential decay
                (-distance).exp()
            }
            DistanceMetric::Jaccard => {
                // Jaccard distance is in [0, 1], similarity = 1 - distance
                (1.0 - distance).max(0.0)
            }
            DistanceMetric::Chebyshev => {
                // Chebyshev distance (L∞ norm): similarity = 1 / (1 + distance)
                1.0 / (1.0 + distance)
            }
            DistanceMetric::Canberra => {
                // Canberra distance: weighted Manhattan, similarity = 1 / (1 + distance)
                1.0 / (1.0 + distance)
            }
            DistanceMetric::Minkowski => {
                // Minkowski distance (p=3): similarity = 1 / (1 + distance)
                1.0 / (1.0 + distance)
            }
            DistanceMetric::Angular => {
                // Angular distance is normalized to [0, 1], similarity = 1 - distance
                (1.0 - distance).max(0.0)
            }
            DistanceMetric::BrayCurtis => {
                // Bray-Curtis distance is in [0, 1], similarity = 1 - distance
                (1.0 - distance).max(0.0)
            }
            DistanceMetric::Hellinger => {
                // Hellinger distance is in [0, 1], similarity = 1 - distance
                (1.0 - distance).max(0.0)
            }
            DistanceMetric::Custom => {
                // Custom metric: use generic exponential decay
                (-distance).exp()
            }
            DistanceMetric::Unspecified => {
                // Unspecified defaults to cosine similarity conversion
                if distance.is_infinite() {
                    0.0
                } else {
                    (1.0 - (distance.max(0.0).min(2.0) / 2.0)).max(0.0)
                }
            }
        }
    }

    /// Create InternalSearchResult with standardized similarity scoring
    /// This should be used by ALL storage engines for consistent ranking
    pub fn from_distance_standard(
        id: String,
        raw_distance: f32,
        metric: &crate::compute::distance_computation::DistanceMetric,
        vector: Option<Vec<f32>>,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        let similarity_score = Self::standardized_distance_to_similarity(raw_distance, metric);

        Self {
            id,
            vector_id: None,
            score: similarity_score, // IMPORTANT: Always use similarity for consistent ranking
            similarity: Some(similarity_score),
            vector,
            metadata,
            debug_info: None,
            version: None,
            timestamp: None,
            updated_at: None,
            expires_at: None,
            source: None,
            expanded_context: Vec::new(),
            semantic_similarity: None,
            quantization_info: None,
            engine_stats: None,
            index_path: None,
        }
    }
    
    /// Convert to OptimizedSearchRecord (MIGRATION METHOD)
    /// 
    /// This method enables migration from InternalSearchResult to OptimizedSearchRecord
    /// for better performance and proto v1 compatibility.
    #[deprecated(since = "1.0.0", note = "Migrate to using OptimizedSearchRecord directly")]
    pub fn to_optimized_record(self) -> OptimizedSearchRecord {
        OptimizedSearchRecord {
            id: self.id,
            vector_id: self.vector_id,
            score: self.score,
            similarity: self.similarity,
            vector: self.vector.map(Arc::new),
            // Convert serde_json::Value metadata to SqlValue metadata (critical for proto v1)
            metadata: crate::core::conversions::json_map_to_sql_values(self.metadata),
            debug_info: self.debug_info,
            version: self.version.map(|v| v as i64), // Convert u32 to i64
            timestamp: self.timestamp.map(|t| t as i64), // Convert u32 to i64 for proto compatibility
            updated_at: self.updated_at.map(|t| t as i64),
            expires_at: self.expires_at.map(|t| t as i64),
            source: self.source,
            expanded_context: self.expanded_context,
            // New fields in OptimizedSearchRecord for comprehensive vector processing
            semantic_similarity: None, // Will be populated by advanced similarity engines
            quantization_info: None,   // Will be populated by quantization engines
            engine_stats: None,        // Will be populated by storage engines
            index_path: None,          // Will be populated by indexing system
        }
    }
}

// Type alias for backwards compatibility during migration
// This allows existing code to compile while we complete the migration
#[deprecated(since = "1.0.0", note = "Use OptimizedSearchRecord directly for better performance")]
pub type LegacyInternalSearchResult = InternalSearchResult;

/// Optimized search record structure with performance improvements
/// This variant uses Arc for vectors and TypedMetadata for better performance
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OptimizedSearchRecord {
    /// Vector/document identifier
    pub id: String,
    /// Alternative identifier field for compatibility
    pub vector_id: Option<String>,
    /// Similarity score (higher = more similar)
    pub score: f32,
    /// Distance value (lower = more similar, if different from score)  
    pub similarity: Option<f32>,
    /// Original vector data (using Arc to avoid cloning)
    pub vector: Option<Arc<Vec<f32>>>,
    /// Associated metadata (using HashMap<String, SqlValue> for full SQL type support and superior performance)
    pub metadata: std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    /// Debug information for result
    pub debug_info: Option<SearchDebugInfo>,
    /// Version for MVCC
    pub version: Option<i64>,
    /// Record timestamp
    pub timestamp: Option<i64>,
    /// Update timestamp
    pub updated_at: Option<i64>,
    /// TTL expiration timestamp
    pub expires_at: Option<i64>,
    /// Original source content
    pub source: Option<SourceContent>,
    /// Expanded context for RAG applications
    pub expanded_context: Vec<SourceContent>,
    /// Semantic distance information
    pub semantic_similarity: Option<SimilarityResult>,
    /// Quantization information
    pub quantization_info: Option<QuantizationInfo>,
    /// Engine-specific optimization stats
    pub engine_stats: Option<EngineStats>,
    /// Index path for result tracking
    pub index_path: Option<String>,
}

// Custom Serialize for OptimizedSearchRecord
impl Serialize for OptimizedSearchRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Convert to InternalSearchResult for serialization
        self.clone().to_internal().serialize(serializer)
    }
}

// Custom Deserialize for OptimizedSearchRecord
impl<'de> Deserialize<'de> for OptimizedSearchRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize as InternalSearchResult then convert
        let internal = InternalSearchResult::deserialize(deserializer)?;
        Ok(OptimizedSearchRecord::from_internal(internal))
    }
}

impl OptimizedSearchRecord {
    /// Create a new OptimizedSearchRecord with just ID and score
    pub fn new(id: String, score: f32) -> Self {
        Self {
            id,
            score,
            ..Default::default()
        }
    }

    /// Standardized distance-to-similarity conversion for consistent ranking across all metrics
    /// This is a static method that can be used without an instance
    pub fn standardized_distance_to_similarity(
        distance: f32,
        metric: &crate::compute::distance_computation::DistanceMetric,
    ) -> f32 {
        use crate::compute::distance_computation::DistanceMetric;
        match metric {
            DistanceMetric::Cosine => {
                // Cosine distance is in [0, 2], similarity = 1 - distance/2 for normalized range
                if distance.is_infinite() {
                    0.0 // Zero vectors get worst similarity score
                } else {
                    1.0 - (distance / 2.0).min(1.0).max(0.0)
                }
            }
            DistanceMetric::Euclidean => {
                // Euclidean distance is in [0, ∞), convert using 1/(1+d) for [0,1] range
                1.0 / (1.0 + distance)
            }
            DistanceMetric::DotProduct => {
                // Dot product similarity is already in similarity form (higher = better)
                // Just ensure it's in [0,1] range
                ((distance + 1.0) / 2.0).min(1.0).max(0.0)
            }
            DistanceMetric::Manhattan => {
                // Manhattan distance is in [0, ∞), use exponential decay
                (-distance / 10.0).exp()
            }
            _ => {
                // Default conversion for other metrics
                1.0 / (1.0 + distance)
            }
        }
    }

    /// Create with ID, score and vector
    pub fn with_vector(id: String, score: f32, vector: Vec<f32>) -> Self {
        Self {
            id,
            score,
            vector: Some(Arc::new(vector)),
            ..Default::default()
        }
    }

    /// Create with ID, score and shared vector (Arc)
    pub fn with_arc_vector(id: String, score: f32, vector: Arc<Vec<f32>>) -> Self {
        Self {
            id,
            score,
            vector: Some(vector),
            ..Default::default()
        }
    }

    /// Builder method to add vector
    pub fn add_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(Arc::new(vector));
        self
    }

    /// Builder method to add metadata (using HashMap<String, SqlValue> for full SQL type support)
    pub fn with_metadata(
        mut self,
        metadata: std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    ) -> Self {
        self.metadata = metadata;
        self
    }

    /// Builder method to add similarity
    pub fn with_similarity(mut self, similarity: f32) -> Self {
        self.similarity = Some(similarity);
        self
    }

    /// Builder method to add version info
    pub fn with_version_info(mut self, version: i64, timestamp: i64) -> Self {
        self.version = Some(version);
        self.timestamp = Some(timestamp);
        self
    }

    /// Builder method to add source content
    pub fn with_source(mut self, source: SourceContent) -> Self {
        self.source = Some(source);
        self
    }

    /// Convert from InternalSearchResult for migration
    pub fn from_internal(result: InternalSearchResult) -> Self {
        Self {
            id: result.id,
            vector_id: result.vector_id,
            score: result.score,
            similarity: result.similarity,
            vector: result.vector.map(|v| Arc::new(v)),
            metadata: convert_json_map_to_sql_value_map(result.metadata),
            debug_info: result.debug_info,
            version: result.version.map(|v| v as i64),
            timestamp: result.timestamp.map(|v| v as i64),
            updated_at: result.updated_at.map(|v| v as i64),
            expires_at: result.expires_at.map(|v| v as i64),
            source: result.source,
            expanded_context: result.expanded_context,
            semantic_similarity: result.semantic_similarity,
            quantization_info: result.quantization_info,
            engine_stats: result.engine_stats,
            index_path: result.index_path,
        }
    }

    /// Convert to InternalSearchResult for compatibility
    pub fn to_internal(self) -> InternalSearchResult {
        InternalSearchResult {
            id: self.id,
            vector_id: self.vector_id,
            score: self.score,
            similarity: self.similarity,
            vector: self.vector.map(|arc| (*arc).clone()),
            metadata: convert_sql_value_map_to_json_map(&self.metadata),
            debug_info: self.debug_info,
            version: self.version.map(|v| v as u32),
            timestamp: self.timestamp.map(|v| v as u32),
            updated_at: self.updated_at.map(|v| v as u32),
            expires_at: self.expires_at.map(|v| v as u32),
            source: self.source,
            expanded_context: self.expanded_context,
            semantic_similarity: self.semantic_similarity,
            quantization_info: self.quantization_info,
            engine_stats: self.engine_stats,
            index_path: self.index_path,
        }
    }
}

/// Collection of search results with metadata
/// Using Arc<[OptimizedSearchRecord]> for immutable, zero-copy sharing of results
#[derive(Debug, Clone)]
pub struct SearchResultSet {
    /// Individual search results - immutable for performance
    pub results: Arc<[OptimizedSearchRecord]>,
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

impl SearchResultSet {
    /// Create a SearchResultSet from a Vec<OptimizedSearchRecord>
    pub fn from_vec(
        results: Vec<OptimizedSearchRecord>,
        total_count: u64,
        query_id: Option<String>,
        processing_time_us: u64,
        algorithm: String,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            results: Arc::from(results.into_boxed_slice()),
            total_count,
            query_id,
            processing_time_us,
            algorithm,
            metadata,
        }
    }

    /// Create an empty SearchResultSet
    pub fn empty(algorithm: String) -> Self {
        Self {
            results: Arc::from(Vec::new().into_boxed_slice()),
            total_count: 0,
            query_id: None,
            processing_time_us: 0,
            algorithm,
            metadata: HashMap::new(),
        }
    }
}

/// Helper module for serializing Arc<[T]>
mod arc_slice_serde {
    use super::OptimizedSearchRecord;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;

    pub fn serialize<S>(
        results: &Arc<[OptimizedSearchRecord]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        results.as_ref().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Arc<[OptimizedSearchRecord]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec = Vec::<OptimizedSearchRecord>::deserialize(deserializer)?;
        Ok(Arc::from(vec.into_boxed_slice()))
    }
}

// Manual trait implementations for ordering (HashMap doesn't implement Ord)
impl Eq for InternalSearchResult {}

impl PartialOrd for InternalSearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InternalSearchResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Order by score in REVERSE order (higher scores first)
        // For distance metrics, lower is better, so this gives us better results first
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| self.id.cmp(&other.id)) // Tie-break by ID for consistency
    }
}

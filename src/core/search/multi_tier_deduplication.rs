//! Multi-Tier Vector Deduplication for ProximaDB
//!
//! Handles ID-based deduplication across storage tiers:
//! 1. Unflushed WAL data (memtable) - highest priority
//! 2. Flushed SST files - medium priority  
//! 3. Compacted storage - lowest priority
//!
//! The merge strategy ensures that for vectors with the same ID:
//! - Unflushed data overrides flushed data
//! - Flushed data overrides compacted data
//! - Records without IDs are included without deduplication

use crate::core::search::mvcc_resolution::MvccResolver;
use crate::core::{MetadataQuery, MetadataQueryEngine};
use crate::proto::proximadb_v1::VectorRecord;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// Vector search result with storage tier metadata
#[derive(Debug, Clone)]
/// A search candidate from a specific storage tier awaiting deduplication
pub struct TieredSearchCandidate {
    /// The matched vector record
    pub vector_record: VectorRecord,
    /// Similarity score to the query vector
    pub similarity: f32,
    /// Data freshness tier this result came from
    pub tier: DataFreshnessTier,
    /// Storage engine that produced this result
    pub engine: DeduplicationStorageEngine,
    /// Timestamp when this record was written
    pub timestamp: DateTime<Utc>,
    /// Write sequence number for ordering within a tier
    pub sequence: u64,
    /// File path for flushed/compacted results (SST or Parquet)
    pub file_path: Option<String>,
}

/// Data freshness tier hierarchy for deduplication priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DataFreshnessTier {
    /// Lowest priority - final compacted storage
    Compacted = 0,
    /// Medium priority - flushed but not yet compacted
    Flushed = 1,
    /// Highest priority - WAL data still in memtable
    Unflushed = 2,
}

/// Storage engine type for search result context (includes WAL for unflushed data)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeduplicationStorageEngine {
    /// SST files (LSM-Tree storage)
    SST,
    /// VIPER with Parquet files
    VIPER,
    /// WAL memtable (unflushed)
    WAL,
}

/// Metadata filter for client-side filtering
pub type MetadataFilter = HashMap<String, JsonValue>;

/// Multi-tier deduplication manager with advanced metadata filtering support
pub struct MultiTierDeduplicator {
    /// Track latest version of each vector ID across tiers
    id_to_latest: HashMap<String, TieredSearchCandidate>,
    /// Results without IDs (no deduplication possible)
    results_without_id: Vec<TieredSearchCandidate>,
    /// Simple metadata filters for backward compatibility
    metadata_filters: Option<MetadataFilter>,
    /// Advanced logical metadata query for complex filtering
    metadata_query: Option<MetadataQuery>,
    /// Query engine for evaluating metadata queries
    query_engine: MetadataQueryEngine,
    /// Target k for early termination optimization
    target_k: Option<usize>,
    /// Track if we've reached k unique results
    early_termination_possible: bool,
    /// Whether results need to be ordered by score (disables early termination)
    requires_ordering: bool,
}

impl MultiTierDeduplicator {
    /// Create a new deduplicator with default settings
    pub fn new() -> Self {
        Self {
            id_to_latest: HashMap::new(),
            results_without_id: Vec::new(),
            metadata_filters: None,
            metadata_query: None,
            query_engine: MetadataQueryEngine::new(),
            target_k: None,
            early_termination_possible: false,
            requires_ordering: true, // Default to true for safety
        }
    }

    /// Create with target k for early termination optimization
    pub fn with_k(k: usize) -> Self {
        Self {
            id_to_latest: HashMap::new(),
            results_without_id: Vec::new(),
            metadata_filters: None,
            metadata_query: None,
            query_engine: MetadataQueryEngine::new(),
            target_k: Some(k),
            early_termination_possible: false,
            requires_ordering: true,
        }
    }

    /// Create with simple metadata filters for backward compatibility
    pub fn with_filters(metadata_filters: MetadataFilter) -> Self {
        Self {
            id_to_latest: HashMap::new(),
            results_without_id: Vec::new(),
            metadata_filters: Some(metadata_filters),
            metadata_query: None,
            query_engine: MetadataQueryEngine::new(),
            target_k: None,
            early_termination_possible: false,
            requires_ordering: true,
        }
    }

    /// Create with advanced logical metadata query
    pub fn with_query(metadata_query: MetadataQuery) -> Self {
        Self {
            id_to_latest: HashMap::new(),
            results_without_id: Vec::new(),
            metadata_filters: None,
            metadata_query: Some(metadata_query),
            query_engine: MetadataQueryEngine::new(),
            target_k: None,
            early_termination_possible: false,
            requires_ordering: true,
        }
    }

    /// Create with both simple filters and logical query (query takes precedence)
    pub fn with_filters_and_query(
        metadata_filters: MetadataFilter,
        metadata_query: MetadataQuery,
    ) -> Self {
        Self {
            id_to_latest: HashMap::new(),
            results_without_id: Vec::new(),
            metadata_filters: Some(metadata_filters),
            metadata_query: Some(metadata_query),
            query_engine: MetadataQueryEngine::new(),
            target_k: None,
            early_termination_possible: false,
            requires_ordering: true,
        }
    }

    /// Set target k for early termination
    pub fn set_target_k(&mut self, k: usize) {
        self.target_k = Some(k);
    }

    /// Set whether results require ordering (disables early termination)
    pub fn set_requires_ordering(&mut self, requires_ordering: bool) {
        self.requires_ordering = requires_ordering;
        if requires_ordering {
            // Disable early termination if ordering is required
            self.early_termination_possible = false;
        }
    }

    /// Check if early termination is allowed
    pub fn can_terminate_early(&self) -> bool {
        !self.requires_ordering && self.target_k.is_some()
    }

    /// Check if early termination has been triggered
    pub fn is_early_terminated(&self) -> bool {
        self.early_termination_possible
    }

    /// Check if a vector record matches the metadata filters or query
    fn matches_filters(&mut self, vector_record: &VectorRecord) -> bool {
        // If we have a logical metadata query, use that (takes precedence)
        if let Some(ref query) = self.metadata_query {
            let json_metadata = crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(
                &vector_record.metadata,
            );
            match self.query_engine.evaluate(query, &json_metadata) {
                Ok(result) => {
                    if !result {
                        tracing::debug!(
                            "🔍 Query filter: Vector {} did not match logical query",
                            if vector_record.id.is_empty() {
                                "<no-id>"
                            } else {
                                &vector_record.id
                            }
                        );
                    }
                    return result;
                }
                Err(e) => {
                    tracing::warn!(
                        "🚨 Query evaluation error for vector {}: {}",
                        if vector_record.id.is_empty() {
                            "<no-id>"
                        } else {
                            &vector_record.id
                        },
                        e
                    );
                    return false; // Fail safe on query evaluation error
                }
            }
        }

        // Fall back to simple filters for backward compatibility
        match &self.metadata_filters {
            None => true, // No filters - all records match
            Some(filters) => {
                // Apply each filter
                let metadata = &vector_record.metadata;
                for (key, expected_value) in filters {
                    match metadata.iter().find(|(k, _)| k.as_str() == key) {
                        Some((_, item)) => {
                            // Convert metadata value to JSON for comparison
                            let actual_json = match &item.value {
                                Some(
                                    crate::proto::proximadb_v1::sql_value::Value::StringValue(s),
                                ) => serde_json::Value::String(s.clone()),
                                Some(
                                    crate::proto::proximadb_v1::sql_value::Value::NumberValue(n),
                                ) => serde_json::Number::from_f64(*n).map_or_else(
                                    || serde_json::Value::String(n.to_string()),
                                    serde_json::Value::Number,
                                ),
                                Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(
                                    b,
                                )) => serde_json::Value::Bool(*b),
                                Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(
                                    i,
                                )) => serde_json::Value::Number(serde_json::Number::from(*i)),
                                Some(crate::proto::proximadb_v1::sql_value::Value::BytesValue(
                                    _,
                                )) => serde_json::Value::String("[binary]".to_string()),
                                Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(
                                    _,
                                )) => serde_json::Value::Null,
                                Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(
                                    _,
                                )) => serde_json::Value::String("[array]".to_string()),
                                Some(
                                    crate::proto::proximadb_v1::sql_value::Value::ObjectValue(_),
                                ) => serde_json::Value::String("[object]".to_string()),
                                None => serde_json::Value::Null,
                            };
                            // Compare values (strict equality for now)
                            if &actual_json != expected_value {
                                tracing::debug!(
                                    "🔍 Simple filter mismatch: {} expected {:?}, got {:?}",
                                    key,
                                    expected_value,
                                    actual_json
                                );
                                return false;
                            }
                        }
                        None => {
                            tracing::debug!(
                                "🔍 Simple filter mismatch: {} not found in metadata_info",
                                key
                            );
                            return false; // Required metadata key missing
                        }
                    }
                }
                true // All filters passed
            }
        }
    }

    /// Add search results from a specific storage tier
    pub fn add_tier_results(&mut self, results: Vec<TieredSearchCandidate>) {
        // Check if early termination is already possible before processing
        // Only skip if we don't require ordering AND we've already reached k
        if self.early_termination_possible && !self.requires_ordering {
            tracing::debug!(
                "🚀 Early termination: Already have {} unique results, skipping {} new results",
                self.target_k.unwrap_or(0),
                results.len()
            );
            return;
        }

        for result in results {
            // Apply metadata filters first
            if !self.matches_filters(&result.vector_record) {
                tracing::debug!(
                    "🚫 Filter: Skipping vector {} due to metadata filter mismatch",
                    if result.vector_record.id.is_empty() {
                        "<no-id>"
                    } else {
                        &result.vector_record.id
                    }
                );
                continue;
            }

            if result.vector_record.id.is_empty() {
                // No ID - include directly (no deduplication possible)
                self.results_without_id.push(result);
            } else {
                // ID-based deduplication across and within tiers
                let vector_id = result.vector_record.id.clone().clone();
                let should_replace = match self.id_to_latest.get(&vector_id) {
                    Some(existing) => {
                        // Multi-criteria replacement logic (in order of priority):
                        if result.tier > existing.tier {
                            // 1. Higher tier always wins (unflushed > flushed > compacted)
                            true
                        } else if result.tier < existing.tier {
                            // Lower tier never wins
                            false
                        } else {
                            // Same tier - use fine-grained ordering
                            if result.sequence > existing.sequence {
                                // 2. Higher sequence number (newer operation)
                                true
                            } else if result.sequence < existing.sequence {
                                // Older sequence number
                                false
                            } else {
                                // Same sequence - use centralized MVCC resolution
                                let resolver = MvccResolver::new();
                                resolver
                                    .compare_records(&result.vector_record, &existing.vector_record)
                            }
                        }
                    }
                    None => true, // First occurrence
                };

                if should_replace {
                    if let Some(existing) = self.id_to_latest.get(&vector_id) {
                        tracing::debug!(
                            "🔄 Dedup: Replacing vector {} from {:?}/{:?} (seq:{}, v:{}, ts:{}) with {:?}/{:?} (seq:{}, v:{}, ts:{})",
                            vector_id,
                            existing.tier,
                            existing.engine,
                            existing.sequence,
                            existing.vector_record.version.unwrap_or(0),
                            existing.timestamp.timestamp_millis(),
                            result.tier,
                            result.engine,
                            result.sequence,
                            result.vector_record.version.unwrap_or(0),
                            result.timestamp.timestamp_millis()
                        );
                    } else {
                        tracing::debug!(
                            "✅ Dedup: Adding new vector {} from {:?}/{:?} (seq:{}, v:{}, ts:{})",
                            vector_id,
                            result.tier,
                            result.engine,
                            result.sequence,
                            result.vector_record.version.unwrap_or(0),
                            result.timestamp.timestamp_millis()
                        );
                    }
                    self.id_to_latest.insert(vector_id.clone(), result);
                } else {
                    if let Some(existing) = self.id_to_latest.get(&vector_id) {
                        tracing::debug!(
                            "🚫 Dedup: Skipping older vector {} from {:?}/{:?} (seq:{}, v:{}, ts:{}), keeping {:?}/{:?} (seq:{}, v:{}, ts:{})",
                            vector_id,
                            result.tier,
                            result.engine,
                            result.sequence,
                            result.vector_record.version.unwrap_or(0),
                            result.timestamp.timestamp_millis(),
                            existing.tier,
                            existing.engine,
                            existing.sequence,
                            existing.vector_record.version.unwrap_or(0),
                            existing.timestamp.timestamp_millis()
                        );
                    }
                }
            }

            // Check for early termination after each addition
            // Only terminate early if we don't require ordering
            if !self.requires_ordering
                && let Some(k) = self.target_k
            {
                let current_unique_count = self.id_to_latest.len() + self.results_without_id.len();
                if current_unique_count >= k {
                    self.early_termination_possible = true;
                    tracing::info!(
                        "🚀 Early termination triggered: Reached {} unique results (target k={}, ordering not required)",
                        current_unique_count,
                        k
                    );
                    return; // Stop processing more results
                }
            }
        }
    }

    /// Get final deduplicated results sorted by score
    pub fn get_final_results(self, k: usize) -> Vec<TieredSearchCandidate> {
        let mut final_results = Vec::new();

        // Get current time for tombstone detection
        let current_time_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Capture lengths before moving
        let unique_ids_count = self.id_to_latest.len();
        let without_id_count = self.results_without_id.len();

        // Add latest version of each ID, filtering out tombstones
        // Tombstone design: empty vector + expires_at in past (including 0)
        let mut tombstones_filtered = 0;
        for candidate in self.id_to_latest.into_values() {
            let is_tombstone = candidate.vector_record.vector.is_empty()
                && candidate
                    .vector_record
                    .expires_at
                    .is_some_and(|e| e <= current_time_secs);
            if is_tombstone {
                tombstones_filtered += 1;
                tracing::debug!(
                    "🗑️ Filtering tombstone from final results: {}",
                    candidate.vector_record.id
                );
            } else {
                final_results.push(candidate);
            }
        }

        // Add non-ID results (these should not be tombstones but filter just in case)
        for candidate in self.results_without_id {
            let is_tombstone = candidate.vector_record.vector.is_empty()
                && candidate
                    .vector_record
                    .expires_at
                    .is_some_and(|e| e <= current_time_secs);
            if !is_tombstone {
                final_results.push(candidate);
            } else {
                tombstones_filtered += 1;
            }
        }

        // Sort by similarity (descending - higher similarity is better)
        final_results.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit to k results
        final_results.truncate(k);

        tracing::info!(
            "🎯 Multi-tier deduplication complete: {} unique IDs, {} without ID, {} tombstones filtered, {} final results",
            unique_ids_count,
            without_id_count,
            tombstones_filtered,
            final_results.len()
        );

        final_results
    }

    /// Get deduplication statistics
    pub fn get_stats(&self) -> DeduplicationStats {
        DeduplicationStats {
            unique_ids: self.id_to_latest.len(),
            records_without_id: self.results_without_id.len(),
            total_records: self.id_to_latest.len() + self.results_without_id.len(),
        }
    }
}

/// Statistics for deduplication process
#[derive(Debug, Clone)]
pub struct DeduplicationStats {
    /// Number of unique vector IDs after deduplication
    pub unique_ids: usize,
    /// Number of records that had no ID (could not be deduplicated)
    pub records_without_id: usize,
    /// Total records in the final result set
    pub total_records: usize,
}

impl Default for MultiTierDeduplicator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_tier_deduplication() {
        let mut deduplicator = MultiTierDeduplicator::new();

        let now = chrono::Utc::now();

        // Add compacted result
        let compacted_result = TieredSearchCandidate {
            vector_record: VectorRecord {
                id: "vector_1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: HashMap::new(),
                timestamp: Some(now.timestamp()),
                updated_at: Some(now.timestamp()),
                expires_at: None,
                version: Some(1),
                source: None,
            },
            similarity: 0.5,
            tier: DataFreshnessTier::Compacted,
            engine: DeduplicationStorageEngine::VIPER,
            timestamp: now,
            sequence: 100,
            file_path: Some("/data/compacted.parquet".to_string()),
        };

        // Add flushed result (should override compacted)
        let flushed_result = TieredSearchCandidate {
            vector_record: VectorRecord {
                id: "vector_1".to_string(),
                vector: vec![1.1, 2.1, 3.1],
                metadata: HashMap::new(),
                timestamp: Some(now.timestamp()),
                updated_at: Some(now.timestamp()),
                expires_at: None,
                version: Some(2),
                source: None,
            },
            similarity: 0.4,
            tier: DataFreshnessTier::Flushed,
            engine: DeduplicationStorageEngine::SST,
            timestamp: now,
            sequence: 200,
            file_path: Some("/data/flushed.sstable".to_string()),
        };

        // Add unflushed result (should override flushed)
        let unflushed_result = TieredSearchCandidate {
            vector_record: VectorRecord {
                id: "vector_1".to_string(),
                vector: vec![1.2, 2.2, 3.2],
                metadata: HashMap::new(),
                timestamp: Some(now.timestamp()),
                updated_at: Some(now.timestamp()),
                expires_at: None,
                version: Some(3),
                source: None,
            },
            similarity: 0.3,
            tier: DataFreshnessTier::Unflushed,
            engine: DeduplicationStorageEngine::WAL,
            timestamp: now,
            sequence: 300,
            file_path: None, // WAL data not in files
        };

        deduplicator.add_tier_results(vec![compacted_result]);
        deduplicator.add_tier_results(vec![flushed_result]);
        deduplicator.add_tier_results(vec![unflushed_result]);

        let final_results = deduplicator.get_final_results(10);

        assert_eq!(final_results.len(), 1);
        assert_eq!(final_results[0].tier, DataFreshnessTier::Unflushed);
        assert_eq!(final_results[0].vector_record.version, Some(3));
        assert_eq!(final_results[0].similarity, 0.3);
    }

    #[test]
    fn test_same_tier_ordering() {
        let mut deduplicator = MultiTierDeduplicator::new();
        let now = chrono::Utc::now();

        // Add two unflushed results with same sequence but different versions
        let unflushed_v1 = TieredSearchCandidate {
            vector_record: VectorRecord {
                id: "vector_1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: HashMap::new(),
                timestamp: Some(now.timestamp()),
                updated_at: Some(now.timestamp()),
                expires_at: None,
                version: Some(1),
                source: None,
            },
            similarity: 0.5,
            tier: DataFreshnessTier::Unflushed,
            engine: DeduplicationStorageEngine::WAL,
            timestamp: now,
            sequence: 100,
            file_path: None,
        };

        let unflushed_v2 = TieredSearchCandidate {
            vector_record: VectorRecord {
                id: "vector_1".to_string(),
                vector: vec![1.1, 2.1, 3.1],
                metadata: HashMap::new(),
                timestamp: Some(now.timestamp()),
                updated_at: Some(now.timestamp()),
                expires_at: None,
                version: Some(2), // Higher version
                source: None,
            },
            similarity: 0.4,
            tier: DataFreshnessTier::Unflushed,
            engine: DeduplicationStorageEngine::WAL,
            timestamp: now,
            sequence: 100, // Same sequence
            file_path: None,
        };

        // Add v1 first, then v2 - v2 should win due to higher version
        deduplicator.add_tier_results(vec![unflushed_v1]);
        deduplicator.add_tier_results(vec![unflushed_v2]);

        let final_results = deduplicator.get_final_results(10);

        assert_eq!(final_results.len(), 1);
        assert_eq!(final_results[0].vector_record.version, Some(2)); // v2 should win
        assert_eq!(final_results[0].similarity, 0.4);
    }

    #[test]
    fn test_early_termination_without_ordering() {
        let mut deduplicator = MultiTierDeduplicator::with_k(2);
        deduplicator.set_requires_ordering(false); // No ordering required - can terminate early

        let now = chrono::Utc::now();

        // Create 5 candidates but we only need 2
        let mut candidates = Vec::new();
        for i in 0..5 {
            candidates.push(TieredSearchCandidate {
                vector_record: VectorRecord {
                    id: format!("vector_{}", i),
                    vector: vec![i as f32, 0.0, 0.0],
                    metadata: HashMap::new(),
                    timestamp: Some(now.timestamp()),
                    updated_at: Some(now.timestamp()),
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
                similarity: i as f32,
                tier: DataFreshnessTier::Unflushed,
                engine: DeduplicationStorageEngine::WAL,
                timestamp: now,
                sequence: 100 + i as u64,
                file_path: None,
            });
        }

        // Add first 2 results
        deduplicator.add_tier_results(candidates[0..2].to_vec());
        assert_eq!(deduplicator.early_termination_possible, true);

        // Try to add more - should be skipped due to early termination
        deduplicator.add_tier_results(candidates[2..5].to_vec());

        let final_results = deduplicator.get_final_results(2);

        // Should only have 2 results due to early termination
        assert_eq!(final_results.len(), 2);
    }

    #[test]
    fn test_no_early_termination_with_ordering() {
        let mut deduplicator = MultiTierDeduplicator::with_k(2);
        deduplicator.set_requires_ordering(true); // Ordering required - must process all

        let now = chrono::Utc::now();

        // Create 5 candidates
        let mut candidates = Vec::new();
        for i in 0..5 {
            candidates.push(TieredSearchCandidate {
                vector_record: VectorRecord {
                    id: format!("vector_{}", i),
                    vector: vec![i as f32, 0.0, 0.0],
                    metadata: HashMap::new(),
                    timestamp: Some(now.timestamp()),
                    updated_at: Some(now.timestamp()),
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
                similarity: (5 - i) as f32, // Reverse scores - best results come last
                tier: DataFreshnessTier::Unflushed,
                engine: DeduplicationStorageEngine::WAL,
                timestamp: now,
                sequence: 100 + i as u64,
                file_path: None,
            });
        }

        // Add all results - early termination should NOT occur
        deduplicator.add_tier_results(candidates[0..2].to_vec());
        assert_eq!(deduplicator.early_termination_possible, false); // Should not terminate

        deduplicator.add_tier_results(candidates[2..5].to_vec());

        let final_results = deduplicator.get_final_results(2);

        // Should have best 2 results (highest scores with descending sort)
        assert_eq!(final_results.len(), 2);
        assert_eq!(final_results[0].similarity, 5.0); // vector_0 has similarity 5
        assert_eq!(final_results[1].similarity, 4.0); // vector_1 has similarity 4
    }

    // --- Inlined from tests/unit/search/multi_tier_deduplication_tests.rs ---

    use crate::proto::proximadb_v1::{SqlValue, sql_value};
    use chrono::Duration;
    use serde_json::json;

    #[test]
    fn test_storage_tier_ordering() {
        // Verify tier priority ordering
        assert!(DataFreshnessTier::Unflushed > DataFreshnessTier::Flushed);
        assert!(DataFreshnessTier::Flushed > DataFreshnessTier::Compacted);
        assert!(DataFreshnessTier::Unflushed > DataFreshnessTier::Compacted);

        // Verify numeric values
        assert_eq!(DataFreshnessTier::Compacted as u8, 0);
        assert_eq!(DataFreshnessTier::Flushed as u8, 1);
        assert_eq!(DataFreshnessTier::Unflushed as u8, 2);
    }

    #[test]
    fn test_basic_deduplication() {
        let mut deduplicator = MultiTierDeduplicator::new();

        // Create a base vector record
        let base_record = VectorRecord {
            id: "vec1".to_string(),
            vector: vec![1.0, 0.0, 0.0],
            metadata: {
                let mut metadata = std::collections::HashMap::new();
                metadata.insert(
                    "type".to_string(),
                    SqlValue {
                        value: Some(sql_value::Value::StringValue("test".to_string())),
                    },
                );
                metadata
            },
            timestamp: Some(Utc::now().timestamp_micros()),
            updated_at: Some(Utc::now().timestamp_micros()),
            expires_at: None,
            version: Some(1),
            source: None,
        };

        // Add same vector from different tiers
        let results = vec![
            TieredSearchCandidate {
                vector_record: base_record.clone(),
                similarity: 0.8,
                tier: DataFreshnessTier::Compacted,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now() - Duration::hours(2),
                sequence: 100,
                file_path: Some("/data/compacted.db".to_string()),
            },
            TieredSearchCandidate {
                vector_record: {
                    let mut rec = base_record.clone();
                    rec.version = Some(2);
                    rec
                },
                similarity: 0.85,
                tier: DataFreshnessTier::Flushed,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now() - Duration::hours(1),
                sequence: 200,
                file_path: Some("/data/flushed.db".to_string()),
            },
        ];

        deduplicator.add_tier_results(results);
        let merged = deduplicator.get_final_results(10);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].vector_record.version, Some(2)); // Should get the newer version
        assert_eq!(merged[0].similarity, 0.85);
    }

    #[test]
    fn test_deduplication_without_ids() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let mut deduplicator = MultiTierDeduplicator::new();

        // Create vectors without IDs (immutable vectors)
        let results = vec![
            TieredSearchCandidate {
                vector_record: VectorRecord {
                    id: String::new(),
                    vector: vec![1.0, 0.0, 0.0],
                    metadata: std::collections::HashMap::new(),
                    timestamp: Some(Utc::now().timestamp_micros()),
                    updated_at: Some(Utc::now().timestamp_micros()),
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
                similarity: 0.9,
                tier: DataFreshnessTier::Flushed,
                engine: DeduplicationStorageEngine::VIPER,
                timestamp: Utc::now(),
                sequence: 100,
                file_path: Some("/data/viper/vectors.parquet".to_string()),
            },
            TieredSearchCandidate {
                vector_record: VectorRecord {
                    id: String::new(),
                    vector: vec![0.0, 1.0, 0.0],
                    metadata: std::collections::HashMap::new(),
                    timestamp: Some(Utc::now().timestamp_micros()),
                    updated_at: Some(Utc::now().timestamp_micros()),
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
                similarity: 0.85,
                tier: DataFreshnessTier::Flushed,
                engine: DeduplicationStorageEngine::VIPER,
                timestamp: Utc::now(),
                sequence: 101,
                file_path: Some("/data/viper/vectors.parquet".to_string()),
            },
        ];

        deduplicator.add_tier_results(results);
        let merged = deduplicator.get_final_results(10);

        // Both vectors should be included (no deduplication for ID-less vectors)
        // Results are sorted by score in descending order (highest score first)
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].similarity, 0.9); // Highest score comes first
        assert_eq!(merged[1].similarity, 0.85); // Lower score comes second
    }

    #[test]
    fn test_metadata_filtering() {
        let mut deduplicator = MultiTierDeduplicator::with_filters({
            let mut filters = HashMap::new();
            filters.insert("category".to_string(), json!("science"));
            filters.insert("published".to_string(), json!("true")); // String comparison
            filters
        });

        let records = vec![
            VectorRecord {
                id: "doc1".to_string(),
                vector: vec![1.0, 0.0],
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert(
                        "category".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue("science".to_string())),
                        },
                    );
                    metadata.insert(
                        "published".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue("true".to_string())),
                        },
                    );
                    metadata
                },
                timestamp: Some(Utc::now().timestamp_micros()),
                updated_at: Some(Utc::now().timestamp_micros()),
                expires_at: None,
                version: Some(1),
                source: None,
            },
            VectorRecord {
                id: "doc2".to_string(),
                vector: vec![0.0, 1.0],
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert(
                        "category".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue("history".to_string())),
                        },
                    );
                    metadata.insert(
                        "published".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue("true".to_string())),
                        },
                    );
                    metadata
                },
                timestamp: Some(Utc::now().timestamp_micros()),
                updated_at: Some(Utc::now().timestamp_micros()),
                expires_at: None,
                version: Some(1),
                source: None,
            },
        ];

        let results: Vec<TieredSearchCandidate> = records
            .into_iter()
            .enumerate()
            .map(|(i, record)| TieredSearchCandidate {
                vector_record: record,
                similarity: 0.9 - (i as f32 * 0.1),
                tier: DataFreshnessTier::Flushed,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: i as u64,
                file_path: None,
            })
            .collect();

        deduplicator.add_tier_results(results);
        let merged = deduplicator.get_final_results(10);

        // Only doc1 should match the filters
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].vector_record.id, "doc1".to_string());
    }

    #[test]
    fn test_simple_metadata_query() {
        // Test with simple filters only (logical queries can be tested separately)
        let mut deduplicator = MultiTierDeduplicator::with_filters({
            let mut filters = HashMap::new();
            filters.insert("language".to_string(), json!("en"));
            filters
        });

        let records = vec![
            VectorRecord {
                id: "doc1".to_string(),
                vector: vec![1.0, 0.0],
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert(
                        "language".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue("en".to_string())),
                        },
                    );
                    metadata.insert(
                        "category".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue("tech".to_string())),
                        },
                    );
                    metadata
                },
                timestamp: Some(Utc::now().timestamp_micros()),
                updated_at: Some(Utc::now().timestamp_micros()),
                expires_at: None,
                version: Some(1),
                source: None,
            },
            VectorRecord {
                id: "doc2".to_string(),
                vector: vec![0.0, 1.0],
                metadata: {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert(
                        "language".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue("fr".to_string())),
                        },
                    );
                    metadata.insert(
                        "category".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::StringValue("tech".to_string())),
                        },
                    );
                    metadata
                },
                timestamp: Some(Utc::now().timestamp_micros()),
                updated_at: Some(Utc::now().timestamp_micros()),
                expires_at: None,
                version: Some(1),
                source: None,
            },
        ];

        let results: Vec<TieredSearchCandidate> = records
            .into_iter()
            .enumerate()
            .map(|(i, record)| TieredSearchCandidate {
                vector_record: record,
                similarity: 0.9 - (i as f32 * 0.1),
                tier: DataFreshnessTier::Flushed,
                engine: DeduplicationStorageEngine::VIPER,
                timestamp: Utc::now(),
                sequence: i as u64,
                file_path: None,
            })
            .collect();

        deduplicator.add_tier_results(results);
        let merged = deduplicator.get_final_results(10);

        // Only doc1 should match (language=en)
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].vector_record.id, "doc1".to_string());
    }

    #[test]
    fn test_mixed_engine_deduplication() {
        let mut deduplicator = MultiTierDeduplicator::new();

        let base_record = VectorRecord {
            id: "vec1".to_string(),
            vector: vec![1.0, 0.0, 0.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(Utc::now().timestamp_micros()),
            updated_at: Some(Utc::now().timestamp_micros()),
            expires_at: None,
            version: Some(1),
            source: None,
        };

        // Add results from different engines
        let results = vec![
            TieredSearchCandidate {
                vector_record: base_record.clone(),
                similarity: 0.8,
                tier: DataFreshnessTier::Compacted,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now() - Duration::hours(2),
                sequence: 100,
                file_path: Some("/data/lsm/compacted.db".to_string()),
            },
            TieredSearchCandidate {
                vector_record: {
                    let mut rec = base_record.clone();
                    rec.version = Some(2);
                    rec
                },
                similarity: 0.85,
                tier: DataFreshnessTier::Compacted,
                engine: DeduplicationStorageEngine::VIPER,
                timestamp: Utc::now() - Duration::hours(1),
                sequence: 200,
                file_path: Some("/data/viper/cluster.parquet".to_string()),
            },
            TieredSearchCandidate {
                vector_record: {
                    let mut rec = base_record.clone();
                    rec.version = Some(3);
                    rec
                },
                similarity: 0.9,
                tier: DataFreshnessTier::Unflushed,
                engine: DeduplicationStorageEngine::WAL,
                timestamp: Utc::now(),
                sequence: 300,
                file_path: None,
            },
        ];

        deduplicator.add_tier_results(results);
        let merged = deduplicator.get_final_results(10);

        // Should get the unflushed WAL version (highest priority)
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].vector_record.version, Some(3));
        assert_eq!(merged[0].similarity, 0.9);
    }

    #[test]
    fn test_k_limit_enforcement() {
        let mut deduplicator = MultiTierDeduplicator::new();

        // Add 20 unique results
        let mut results = Vec::new();
        for i in 0..20 {
            results.push(TieredSearchCandidate {
                vector_record: VectorRecord {
                    id: format!("vec{}", i),
                    vector: vec![i as f32, 0.0, 0.0],
                    metadata: std::collections::HashMap::new(),
                    timestamp: Some(Utc::now().timestamp_micros()),
                    updated_at: Some(Utc::now().timestamp_micros()),
                    expires_at: None,
                    version: Some(1),
                    source: None,
                },
                similarity: (i as f32 * 0.01), // Increasing scores (ascending order)
                tier: DataFreshnessTier::Flushed,
                engine: DeduplicationStorageEngine::SST,
                timestamp: Utc::now(),
                sequence: i as u64,
                file_path: Some(format!("/data/file_{}.db", i)),
            });
        }

        deduplicator.add_tier_results(results);

        // Request only top 10
        let merged = deduplicator.get_final_results(10);

        assert_eq!(merged.len(), 10);
        // Results are sorted by score in descending order (highest score first)
        // Top 10 results should be vec19 (0.19) to vec10 (0.10)
        assert_eq!(merged[0].vector_record.id, "vec19".to_string()); // Highest score (0.19)
        assert_eq!(
            merged[merged.len() - 1].vector_record.id,
            "vec10".to_string()
        ); // 10th highest score (0.10)
    }

    #[test]
    fn test_complex_deduplication_scenario() {
        let mut deduplicator = MultiTierDeduplicator::new();

        // Scenario: Multiple versions of same vectors across different tiers
        let mut results = Vec::new();

        // Vector A: versions in all tiers
        for (version, tier, engine, hours_ago) in vec![
            (
                1,
                DataFreshnessTier::Compacted,
                DeduplicationStorageEngine::SST,
                24,
            ),
            (
                2,
                DataFreshnessTier::Flushed,
                DeduplicationStorageEngine::SST,
                12,
            ),
            (
                3,
                DataFreshnessTier::Unflushed,
                DeduplicationStorageEngine::WAL,
                0,
            ),
        ] {
            results.push(TieredSearchCandidate {
                vector_record: VectorRecord {
                    id: "vecA".to_string(),
                    vector: vec![1.0, 0.0, 0.0],
                    metadata: {
                        let mut metadata = std::collections::HashMap::new();
                        metadata.insert(
                            "version".to_string(),
                            SqlValue {
                                value: Some(sql_value::Value::StringValue(version.to_string())),
                            },
                        );
                        metadata
                    },
                    timestamp: Some(Utc::now().timestamp_micros()),
                    updated_at: Some(Utc::now().timestamp_micros()),
                    expires_at: None,
                    version: Some(version),
                    source: None,
                },
                similarity: 0.95,
                tier,
                engine,
                timestamp: Utc::now() - Duration::hours(hours_ago),
                sequence: version as u64 * 100,
                file_path: Some(format!("/data/tier_{}.db", version)),
            });
        }

        // Vector B: only in compacted and flushed
        for (version, tier, engine, hours_ago) in vec![
            (
                1,
                DataFreshnessTier::Compacted,
                DeduplicationStorageEngine::VIPER,
                20,
            ),
            (
                2,
                DataFreshnessTier::Flushed,
                DeduplicationStorageEngine::VIPER,
                8,
            ),
        ] {
            results.push(TieredSearchCandidate {
                vector_record: VectorRecord {
                    id: "vecB".to_string(),
                    vector: vec![0.0, 1.0, 0.0],
                    metadata: {
                        let mut metadata = std::collections::HashMap::new();
                        metadata.insert(
                            "version".to_string(),
                            SqlValue {
                                value: Some(sql_value::Value::StringValue(version.to_string())),
                            },
                        );
                        metadata
                    },
                    timestamp: Some(Utc::now().timestamp_micros()),
                    updated_at: Some(Utc::now().timestamp_micros()),
                    expires_at: None,
                    version: Some(version),
                    source: None,
                },
                similarity: 0.90,
                tier,
                engine,
                timestamp: Utc::now() - Duration::hours(hours_ago),
                sequence: version as u64 * 100 + 50,
                file_path: Some(format!("/data/viper_{}.parquet", version)),
            });
        }

        // Vector C: no ID (immutable)
        results.push(TieredSearchCandidate {
            vector_record: VectorRecord {
                id: String::new(),
                vector: vec![0.0, 0.0, 1.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(Utc::now().timestamp_micros()),
                updated_at: Some(Utc::now().timestamp_micros()),
                expires_at: None,
                version: Some(1),
                source: None,
            },
            similarity: 0.85,
            tier: DataFreshnessTier::Flushed,
            engine: DeduplicationStorageEngine::VIPER,
            timestamp: Utc::now() - Duration::hours(4),
            sequence: 1000,
            file_path: Some("/data/immutable.parquet".to_string()),
        });

        deduplicator.add_tier_results(results);
        let merged = deduplicator.get_final_results(10);

        // Should get:
        // - vecA version 3 (unflushed)
        // - vecB version 2 (flushed)
        // - vecC (no ID)
        assert_eq!(merged.len(), 3);

        // Verify vecA is version 3
        let vec_a = merged
            .iter()
            .find(|r| r.vector_record.id == "vecA".to_string())
            .unwrap();
        assert_eq!(vec_a.vector_record.version, Some(3));

        // Verify vecB is version 2
        let vec_b = merged
            .iter()
            .find(|r| r.vector_record.id == "vecB".to_string())
            .unwrap();
        assert_eq!(vec_b.vector_record.version, Some(2));

        // Verify vecC is included
        let vec_c = merged
            .iter()
            .find(|r| r.vector_record.id.is_empty())
            .unwrap();
        assert_eq!(vec_c.similarity, 0.85);
    }

    // Unit tests for early termination logic (from early_termination_test.rs)
    #[test]
    fn test_early_termination_logic() {
        // Test 1: With ordering required (like REST/gRPC search) - no early termination
        {
            let mut dedup = MultiTierDeduplicator::with_k(2);
            dedup.set_requires_ordering(true); // REST/gRPC always need ordering

            let now = Utc::now();
            let mut candidates = vec![];

            // Create 5 candidates with varying scores
            for i in 0..5 {
                candidates.push(TieredSearchCandidate {
                    vector_record: crate::proto::proximadb_v1::VectorRecord {
                        id: format!("vec_{}", i),
                        vector: vec![i as f32],
                        metadata: std::collections::HashMap::new(),
                        timestamp: Some(now.timestamp_millis()),
                        updated_at: Some(now.timestamp_millis()),
                        expires_at: None,
                        version: Some(1),
                        source: Some("test".to_string()),
                    },
                    similarity: (5 - i) as f32, // Best scores come last
                    tier: DataFreshnessTier::Unflushed,
                    engine: DeduplicationStorageEngine::WAL,
                    timestamp: now,
                    sequence: i as u64,
                    file_path: None,
                });
            }

            // Add all candidates - should process all despite having k=2
            dedup.add_tier_results(candidates);

            let results = dedup.get_final_results(2);
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].similarity, 5.0); // Best score (highest)
            assert_eq!(results[1].similarity, 4.0); // Second best
        }

        // Test 2: Without ordering (SQL without ORDER BY) - early termination enabled
        {
            let mut dedup = MultiTierDeduplicator::with_k(2);
            dedup.set_requires_ordering(false); // SQL without ORDER BY

            let now = Utc::now();
            let mut candidates = vec![];

            // Create candidates
            for i in 0..5 {
                candidates.push(TieredSearchCandidate {
                    vector_record: crate::proto::proximadb_v1::VectorRecord {
                        id: format!("vec_{}", i),
                        vector: vec![i as f32],
                        metadata: std::collections::HashMap::new(),
                        timestamp: Some(now.timestamp_millis()),
                        updated_at: Some(now.timestamp_millis()),
                        expires_at: None,
                        version: Some(1),
                        source: Some("test".to_string()),
                    },
                    similarity: i as f32,
                    tier: DataFreshnessTier::Unflushed,
                    engine: DeduplicationStorageEngine::WAL,
                    timestamp: now,
                    sequence: i as u64,
                    file_path: None,
                });
            }

            // Add first batch
            dedup.add_tier_results(candidates[0..2].to_vec());
            assert!(dedup.is_early_terminated());

            // Try to add more - should be skipped
            dedup.add_tier_results(candidates[2..5].to_vec());

            let results = dedup.get_final_results(10); // Ask for more than we have
            assert_eq!(results.len(), 2); // Only got 2 due to early termination
        }
    }

    #[test]
    fn test_sql_query_behavior() {
        // Simulate SQL query patterns

        // Case 1: SQL with ORDER BY VECTOR_SIMILARITY - requires ordering
        {
            let mut dedup = MultiTierDeduplicator::with_k(10);
            dedup.set_requires_ordering(true); // ORDER BY present
            assert!(!dedup.can_terminate_early());
        }

        // Case 2: SQL without ORDER BY - can terminate early
        {
            let mut dedup = MultiTierDeduplicator::with_k(10);
            dedup.set_requires_ordering(false); // No ORDER BY
            assert!(dedup.can_terminate_early());
        }

        // Case 3: SQL with metadata filter only - can terminate early
        {
            let mut dedup = MultiTierDeduplicator::with_k(100);
            dedup.set_requires_ordering(false);
            assert!(dedup.can_terminate_early());
        }
    }

    #[test]
    fn test_grpc_rest_always_ordered() {
        // gRPC and REST endpoints always expect ordered results
        let mut dedup = MultiTierDeduplicator::with_k(25);
        dedup.set_requires_ordering(true); // Always true for gRPC/REST

        assert!(!dedup.can_terminate_early());
        // Field is private, verify through behavior
    }
}

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
    pub vector_record: VectorRecord,
    pub similarity: f32,
    pub tier: DataFreshnessTier,
    pub engine: DeduplicationStorageEngine,
    pub timestamp: DateTime<Utc>,
    pub sequence: u64,
    /// File path for flushed/compacted results (SST or Parquet)
    pub file_path: Option<String>,
}

/// Data freshness tier hierarchy for deduplication priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DataFreshnessTier {
    Compacted = 0, // Lowest priority - final compacted storage
    Flushed = 1,   // Medium priority - flushed but not compacted
    Unflushed = 2, // Highest priority - WAL data in memtable
}

/// Storage engine type for search result context (includes WAL for unflushed data)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeduplicationStorageEngine {
    SST,   // SST files (LSM-Tree storage)
    VIPER, // VIPER with Parquet files
    WAL,   // WAL memtable (unflushed)
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
                                ) => serde_json::Number::from_f64(*n)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or_else(|| serde_json::Value::String(n.to_string())),
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
            if !self.requires_ordering {
                if let Some(k) = self.target_k {
                    let current_unique_count =
                        self.id_to_latest.len() + self.results_without_id.len();
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
                    .map_or(false, |e| e <= current_time_secs);
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
                    .map_or(false, |e| e <= current_time_secs);
            if !is_tombstone {
                final_results.push(candidate);
            } else {
                tombstones_filtered += 1;
            }
        }

        // Sort by similarity (descending - higher similarity is better)
        final_results.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap());

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
    pub unique_ids: usize,
    pub records_without_id: usize,
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
}

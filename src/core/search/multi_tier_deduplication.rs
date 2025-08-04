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

use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use crate::core::{VectorRecord, MetadataQuery, MetadataQueryEngine};

/// Vector search result with storage tier metadata
#[derive(Debug, Clone)]
/// A search candidate from a specific storage tier awaiting deduplication
pub struct TieredSearchCandidate {
    pub vector_record: VectorRecord,
    pub score: f32,
    pub tier: StorageTier,
    pub engine: DeduplicationStorageEngine,
    pub timestamp: DateTime<Utc>,
    pub sequence: u64,
    /// File path for flushed/compacted results (SST or Parquet)
    pub file_path: Option<String>,
}

/// Storage tier hierarchy for deduplication priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StorageTier {
    Compacted = 0,    // Lowest priority - final compacted storage
    Flushed = 1,      // Medium priority - flushed but not compacted
    Unflushed = 2,    // Highest priority - WAL data in memtable
}

/// Storage engine type for search result context (includes WAL for unflushed data)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeduplicationStorageEngine {
    SST,    // SST files (LSM-Tree storage)
    VIPER,  // VIPER with Parquet files  
    WAL,    // WAL memtable (unflushed)
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
    pub fn with_filters_and_query(metadata_filters: MetadataFilter, metadata_query: MetadataQuery) -> Self {
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
            let json_metadata = crate::core::proto_metadata_helper::proto_metadata_to_json(&vector_record.metadata);
            match self.query_engine.evaluate(query, &json_metadata) {
                Ok(result) => {
                    if !result {
                        tracing::debug!(
                            "🔍 Query filter: Vector {} did not match logical query",
                            vector_record.id.as_deref().unwrap_or("unknown")
                        );
                    }
                    return result;
                }
                Err(e) => {
                    tracing::warn!(
                        "🚨 Query evaluation error for vector {}: {}",
                        vector_record.id.as_deref().unwrap_or("unknown"), e
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
                    match metadata.iter().find(|item| &item.key == key) {
                        Some(item) => {
                            // Convert metadata value to JSON for comparison
                            let actual_json = match &item.value {
                                Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) => serde_json::Value::String(s.clone()),
                                Some(crate::proto::proximadb::metadata_item::Value::NumberValue(n)) => {
                                    serde_json::Number::from_f64(*n)
                                        .map(serde_json::Value::Number)
                                        .unwrap_or_else(|| serde_json::Value::String(n.to_string()))
                                },
                                Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)) => serde_json::Value::Bool(*b),
                                None => serde_json::Value::Null,
                            };
                            // Compare values (strict equality for now)
                            if &actual_json != expected_value {
                                tracing::debug!(
                                    "🔍 Simple filter mismatch: {} expected {:?}, got {:?}",
                                    key, expected_value, actual_json
                                );
                                return false;
                            }
                        }
                        None => {
                            tracing::debug!("🔍 Simple filter mismatch: {} not found in metadata", key);
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
                    result.vector_record.id.as_deref().unwrap_or("unknown")
                );
                continue;
            }

            if result.vector_record.id.is_none() || result.vector_record.id.as_deref().unwrap_or("").is_empty() {
                // No ID - include directly (no deduplication possible)
                self.results_without_id.push(result);
            } else {
                // ID-based deduplication across and within tiers
                let vector_id = result.vector_record.id.as_deref().unwrap_or("").to_string();
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
                                // Same sequence - use version and timestamp
                                if result.vector_record.version > existing.vector_record.version {
                                    // 3. Higher version number (explicit versioning)
                                    true
                                } else if result.vector_record.version < existing.vector_record.version {
                                    // Lower version
                                    false
                                } else {
                                    // Same version - use timestamp as final tiebreaker
                                    result.timestamp > existing.timestamp
                                }
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
                            existing.tier, existing.engine, existing.sequence, existing.vector_record.version.unwrap_or(0), existing.timestamp.timestamp_millis(),
                            result.tier, result.engine, result.sequence, result.vector_record.version.unwrap_or(0), result.timestamp.timestamp_millis()
                        );
                    } else {
                        tracing::debug!(
                            "✅ Dedup: Adding new vector {} from {:?}/{:?} (seq:{}, v:{}, ts:{})",
                            vector_id,
                            result.tier, result.engine, result.sequence, result.vector_record.version.unwrap_or(0), result.timestamp.timestamp_millis()
                        );
                    }
                    self.id_to_latest.insert(vector_id.clone(), result);
                } else {
                    if let Some(existing) = self.id_to_latest.get(&vector_id) {
                        tracing::debug!(
                            "🚫 Dedup: Skipping older vector {} from {:?}/{:?} (seq:{}, v:{}, ts:{}), keeping {:?}/{:?} (seq:{}, v:{}, ts:{})",
                            vector_id,
                            result.tier, result.engine, result.sequence, result.vector_record.version.unwrap_or(0), result.timestamp.timestamp_millis(),
                            existing.tier, existing.engine, existing.sequence, existing.vector_record.version.unwrap_or(0), existing.timestamp.timestamp_millis()
                        );
                    }
                }
            }
            
            // Check for early termination after each addition
            // Only terminate early if we don't require ordering
            if !self.requires_ordering {
                if let Some(k) = self.target_k {
                    let current_unique_count = self.id_to_latest.len() + self.results_without_id.len();
                    if current_unique_count >= k {
                        self.early_termination_possible = true;
                        tracing::info!(
                            "🚀 Early termination triggered: Reached {} unique results (target k={}, ordering not required)",
                            current_unique_count, k
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
        
        // Capture lengths before moving
        let unique_ids_count = self.id_to_latest.len();
        let without_id_count = self.results_without_id.len();
        
        // Add latest version of each ID using into_values() to avoid clone
        final_results.extend(self.id_to_latest.into_values());
        
        // Add non-ID results
        final_results.extend(self.results_without_id);

        // Sort by score (descending - higher score is better)
        final_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        
        // Limit to k results
        final_results.truncate(k);

        tracing::info!(
            "🎯 Multi-tier deduplication complete: {} unique IDs, {} without ID, {} final results",
            unique_ids_count,
            without_id_count,
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
                id: Some("vector_1".to_string()),
                vector: vec![1.0, 2.0, 3.0],
                metadata: vec![],
                timestamp: now.timestamp() as u32,
                updated_at: Some(now.timestamp() as u32),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            },
            score: 0.5,
            tier: StorageTier::Compacted,
            engine: DeduplicationStorageEngine::VIPER,
            timestamp: now,
            sequence: 100,
            file_path: Some("/data/compacted.parquet".to_string()),
        };
        
        // Add flushed result (should override compacted)
        let flushed_result = TieredSearchCandidate {
            vector_record: VectorRecord {
                id: Some("vector_1".to_string()),
                vector: vec![1.1, 2.1, 3.1],
                metadata: vec![],
                timestamp: now.timestamp() as u32,
                updated_at: Some(now.timestamp() as u32),
                expires_at: None,
                version: Some(2),
                rank: None,
                score: None,
                distance: None,
            },
            score: 0.4,
            tier: StorageTier::Flushed,
            engine: DeduplicationStorageEngine::SST,
            timestamp: now,
            sequence: 200,
            file_path: Some("/data/flushed.sst".to_string()),
        };
        
        // Add unflushed result (should override flushed)
        let unflushed_result = TieredSearchCandidate {
            vector_record: VectorRecord {
                id: Some("vector_1".to_string()),
                vector: vec![1.2, 2.2, 3.2],
                metadata: vec![],
                timestamp: now.timestamp() as u32,
                updated_at: Some(now.timestamp() as u32),
                expires_at: None,
                version: Some(3),
                rank: None,
                score: None,
                distance: None,
            },
            score: 0.3,
            tier: StorageTier::Unflushed,
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
        assert_eq!(final_results[0].tier, StorageTier::Unflushed);
        assert_eq!(final_results[0].vector_record.version, Some(3));
        assert_eq!(final_results[0].score, 0.3);
    }

    #[test]
    fn test_same_tier_ordering() {
        let mut deduplicator = MultiTierDeduplicator::new();
        let now = chrono::Utc::now();
        
        // Add two unflushed results with same sequence but different versions
        let unflushed_v1 = TieredSearchCandidate {
            vector_record: VectorRecord {
                id: Some("vector_1".to_string()),
                vector: vec![1.0, 2.0, 3.0],
                metadata: vec![],
                timestamp: now.timestamp() as u32,
                updated_at: Some(now.timestamp() as u32),
                expires_at: None,
                version: Some(1),
                rank: None,
                score: None,
                distance: None,
            },
            score: 0.5,
            tier: StorageTier::Unflushed,
            engine: DeduplicationStorageEngine::WAL,
            timestamp: now,
            sequence: 100,
            file_path: None,
        };
        
        let unflushed_v2 = TieredSearchCandidate {
            vector_record: VectorRecord {
                id: Some("vector_1".to_string()),
                vector: vec![1.1, 2.1, 3.1],
                metadata: vec![],
                timestamp: now.timestamp() as u32,
                updated_at: Some(now.timestamp() as u32),
                expires_at: None,
                version: Some(2), // Higher version
                rank: None,
                score: None,
                distance: None,
            },
            score: 0.4,
            tier: StorageTier::Unflushed,
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
        assert_eq!(final_results[0].score, 0.4);
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
                    id: Some(format!("vector_{}", i)),
                    vector: vec![i as f32, 0.0, 0.0],
                    metadata: vec![],
                    timestamp: now.timestamp() as u32,
                    updated_at: Some(now.timestamp() as u32),
                    expires_at: None,
                    version: Some(1),
                    rank: None,
                    score: None,
                    distance: None,
                },
                score: i as f32,
                tier: StorageTier::Unflushed,
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
                    id: Some(format!("vector_{}", i)),
                    vector: vec![i as f32, 0.0, 0.0],
                    metadata: vec![],
                    timestamp: now.timestamp() as u32,
                    updated_at: Some(now.timestamp() as u32),
                    expires_at: None,
                    version: Some(1),
                    rank: None,
                    score: None,
                    distance: None,
                },
                score: (5 - i) as f32, // Reverse scores - best results come last
                tier: StorageTier::Unflushed,
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
        assert_eq!(final_results[0].score, 5.0); // vector_0 has score 5
        assert_eq!(final_results[1].score, 4.0); // vector_1 has score 4
    }
}
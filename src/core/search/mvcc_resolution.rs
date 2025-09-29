//! MVCC (Multi-Version Concurrency Control) Resolution for ProximaDB
//!
//! Centralized logic for resolving vector versions according to MVCC rules:
//! 1. Records with expires_at < current_time are considered deleted
//! 2. Any record with non-empty ID and expires_at < current_time marks all versions deleted
//! 3. Records with empty/null/blank/none ID are append-only (no versioning)
//! 4. For records with same ID, highest version without gaps wins
//! 5. Versions start from 1; None/null/empty version is treated as version 1
//! 6. For same version, earliest timestamp wins

use crate::proto::proximadb_v1::VectorRecord;
use std::collections::HashMap;
use tracing::debug;

/// MVCC resolution result for a single record
#[derive(Debug, Clone)]
pub struct MvccResolutionResult {
    /// Whether this record should be included in results
    pub include: bool,
    /// Reason for exclusion (if applicable)
    pub exclusion_reason: Option<String>,
    /// The resolved record (if included)
    pub record: Option<VectorRecord>,
}

/// MVCC resolver for vector records
pub struct MvccResolver {
    /// Current timestamp for expiry checks
    current_timestamp: u32,
}

impl MvccResolver {
    /// Create a new MVCC resolver
    pub fn new() -> Self {
        Self {
            current_timestamp: chrono::Utc::now().timestamp() as u32,
        }
    }

    /// Create with specific timestamp (for testing)
    pub fn with_timestamp(timestamp: u32) -> Self {
        Self {
            current_timestamp: timestamp,
        }
    }

    /// Resolve a batch of vector records according to MVCC rules
    pub fn resolve_batch(&self, records: Vec<VectorRecord>) -> Vec<VectorRecord> {
        // Separate records by ID presence
        let mut id_groups: HashMap<String, Vec<VectorRecord>> = HashMap::new();
        let mut append_only_records = Vec::new();

        for record in records {
            let id = &record.id;
            if !id.is_empty() {
                if id == "null" || id == "none" || id.trim().is_empty() {
                    // Treat as append-only
                    append_only_records.push(record);
                } else {
                    id_groups
                        .entry(id.clone())
                        .or_insert_with(Vec::new)
                        .push(record);
                }
            } else {
                // No ID = append-only
                append_only_records.push(record);
            }
        }

        let mut resolved = Vec::new();

        // Process each ID group
        for (id, mut versions) in id_groups {
            // Check if any version is expired (marks all versions as deleted)
            let has_expired = versions.iter().any(|r| self.is_expired(r));

            if has_expired {
                debug!(
                    "MVCC: All versions of ID '{}' are deleted due to expiry",
                    id
                );
                continue; // Skip this ID entirely
            }

            // Sort by version, then timestamp
            versions.sort_by(|a, b| {
                let ver_a = a.version.unwrap_or(1);
                let ver_b = b.version.unwrap_or(1);

                ver_a.cmp(&ver_b).then_with(|| {
                    // For same version, earliest timestamp wins
                    a.timestamp.cmp(&b.timestamp)
                })
            });

            // Validate version continuity and find the latest valid version
            // Version sequences must start at 0 or 1, otherwise it's a gap from start
            let starting_version = if let Some(first_record) = versions.first() {
                first_record.version.unwrap_or(1)
            } else {
                1
            };

            // Check if there's a gap from the beginning (must start with 0 or 1)
            if starting_version > 1 {
                debug!(
                    "MVCC: Version gap from start for ID '{}': starts with version {} instead of 0 or 1",
                    id, starting_version
                );
                continue; // Skip this ID entirely
            }

            let mut expected_version = starting_version;
            let mut last_valid: Option<VectorRecord> = None;

            for record in versions {
                let version = record.version.unwrap_or(1);

                if version == expected_version {
                    // This version is continuous
                    last_valid = Some(record);
                    expected_version += 1;
                } else if version > expected_version {
                    // Version gap detected - stop processing this ID
                    debug!(
                        "MVCC: Version gap detected for ID '{}': expected {}, found {}",
                        id, expected_version, version
                    );
                    break;
                } else {
                    // Version < expected_version, this is an older version, skip it
                    debug!(
                        "MVCC: Skipping older version {} for ID '{}' (expected {})",
                        version, id, expected_version
                    );
                }
            }

            if let Some(record) = last_valid {
                debug!(
                    "MVCC: Selected version {} for ID '{}'",
                    record.version.unwrap_or(0),
                    id
                );
                resolved.push(record);
            }
        }

        // Add all append-only records (also check for expiry)
        for record in append_only_records {
            if !self.is_expired(&record) {
                resolved.push(record);
            }
        }

        resolved
    }

    /// Check if a record is expired
    pub fn is_expired(&self, record: &VectorRecord) -> bool {
        if let Some(expires_at) = record.expires_at {
            expires_at < self.current_timestamp as i64
        } else {
            false
        }
    }

    /// Compare two records for the same ID and determine which should win
    /// Returns true if record1 should win, false if record2 should win
    pub fn compare_records(&self, record1: &VectorRecord, record2: &VectorRecord) -> bool {
        // Check expiry first
        let r1_expired = self.is_expired(record1);
        let r2_expired = self.is_expired(record2);

        if r1_expired && !r2_expired {
            return false; // r2 wins
        }
        if !r1_expired && r2_expired {
            return true; // r1 wins
        }
        if r1_expired && r2_expired {
            return false; // Both expired, doesn't matter
        }

        // Compare versions - treat None as version 1
        let v1 = record1.version.unwrap_or(1);
        let v2 = record2.version.unwrap_or(1);

        if v1 > v2 {
            true
        } else if v1 < v2 {
            false
        } else {
            // Same version - earliest timestamp wins
            record1.timestamp <= record2.timestamp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_record(
        id: Option<String>,
        version: Option<u32>,
        timestamp: u32,
        expires_at: Option<u32>,
    ) -> VectorRecord {
        // Use empty string to represent append-only semantics when id is None
        crate::proto::proximadb_v1::VectorRecord {
            id: id.unwrap_or_default(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: std::collections::HashMap::new(),
            timestamp: timestamp as i64,
            updated_at: Some(timestamp as i64),
            expires_at: expires_at.map(|t| t as i64),
            version,
            source: None,
        }
    }

    #[test]
    fn test_version_continuity() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            create_record(Some("v1".to_string()), Some(1), 100, None),
            create_record(Some("v1".to_string()), Some(3), 300, None), // Gap!
            create_record(Some("v1".to_string()), Some(2), 200, None),
        ];

        let resolved = resolver.resolve_batch(records);

        // Should only include version 3 (highest in continuous sequence 1,2,3)
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].version, Some(3));
    }

    #[test]
    fn test_expiry_handling() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            create_record(Some("v1".to_string()), Some(1), 100, None),
            create_record(Some("v1".to_string()), Some(2), 200, Some(500)), // Expired!
            create_record(Some("v1".to_string()), Some(3), 300, None),
        ];

        let resolved = resolver.resolve_batch(records);

        // All versions should be excluded due to one expired version
        assert_eq!(resolved.len(), 0);
    }

    #[test]
    fn test_append_only_records() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            create_record(None, None, 100, None),
            create_record(Some("".to_string()), None, 200, None),
            create_record(Some("null".to_string()), None, 300, None),
            create_record(Some("  ".to_string()), None, 400, None),
        ];

        let resolved = resolver.resolve_batch(records);

        // All should be included as append-only
        assert_eq!(resolved.len(), 4);
    }

    #[test]
    fn test_same_version_timestamp_resolution() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            create_record(Some("v1".to_string()), Some(1), 200, None),
            create_record(Some("v1".to_string()), Some(1), 100, None), // Earlier timestamp
            create_record(Some("v1".to_string()), Some(1), 300, None),
        ];

        let resolved = resolver.resolve_batch(records);

        // Should pick the one with earliest timestamp
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].timestamp, 100);
    }

    #[test]
    fn test_multiple_ids_different_versions() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            create_record(Some("id1".to_string()), Some(1), 100, None),
            create_record(Some("id1".to_string()), Some(2), 200, None),
            create_record(Some("id2".to_string()), Some(1), 150, None),
            create_record(Some("id2".to_string()), Some(3), 350, None), // Gap at version 2
            create_record(Some("id3".to_string()), Some(5), 500, None), // Gap from 1-4
        ];

        let resolved = resolver.resolve_batch(records);

        // Should have: id1 v2, id2 v1 (gap stops at v1), id3 none (gap from start)
        assert_eq!(resolved.len(), 2);

        let mut by_id: std::collections::HashMap<String, &VectorRecord> =
            std::collections::HashMap::new();
        for record in &resolved {
            by_id.insert(record.id.clone(), record);
        }
        assert_eq!(by_id.get("id1").unwrap().version, Some(2));
        assert_eq!(by_id.get("id2").unwrap().version, Some(1));
        assert!(!by_id.contains_key("id3")); // Excluded due to gap
    }

    #[test]
    fn test_version_none_treated_as_one() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            create_record(Some("id1".to_string()), None, 100, None), // Should be treated as version 1
            create_record(Some("id1".to_string()), Some(2), 200, None),
            create_record(Some("id2".to_string()), None, 150, None), // Should be treated as version 1
            create_record(Some("id2".to_string()), None, 120, None), // Duplicate version 1, earlier timestamp wins
        ];

        let resolved = resolver.resolve_batch(records);

        assert_eq!(resolved.len(), 2);

        let mut by_id: std::collections::HashMap<String, &VectorRecord> =
            std::collections::HashMap::new();
        for record in &resolved {
            by_id.insert(record.id.clone(), record);
        }
        assert_eq!(by_id.get("id1").unwrap().version, Some(2));
        assert_eq!(by_id.get("id2").unwrap().version, None); // Original None preserved
        assert_eq!(by_id.get("id2").unwrap().timestamp, 120); // Earlier timestamp wins
    }

    #[test]
    fn test_continuous_vs_non_continuous_versions() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            // ID1: Continuous versions 1,2,3
            create_record(Some("id1".to_string()), Some(1), 100, None),
            create_record(Some("id1".to_string()), Some(2), 200, None),
            create_record(Some("id1".to_string()), Some(3), 300, None),
            // ID2: Gap at version 2 (1,3,4)
            create_record(Some("id2".to_string()), Some(1), 110, None),
            create_record(Some("id2".to_string()), Some(3), 330, None), // Gap!
            create_record(Some("id2".to_string()), Some(4), 440, None),
            // ID3: Starting with version 3 (gap from 1-2)
            create_record(Some("id3".to_string()), Some(3), 330, None),
            create_record(Some("id3".to_string()), Some(4), 440, None),
        ];

        let resolved = resolver.resolve_batch(records);

        assert_eq!(resolved.len(), 2); // Only id1 and id2 should be included

        let mut by_id: std::collections::HashMap<String, &VectorRecord> =
            std::collections::HashMap::new();
        for record in &resolved {
            by_id.insert(record.id.clone(), record);
        }
        assert_eq!(by_id.get("id1").unwrap().version, Some(3)); // Continuous, gets highest
        assert_eq!(by_id.get("id2").unwrap().version, Some(1)); // Gap at 2, stops at 1
        assert!(!by_id.contains_key("id3")); // Gap from start, excluded
    }

    #[test]
    fn test_mixed_append_only_and_versioned() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            // Append-only records (various ways to represent no ID)
            create_record(None, None, 100, None),
            create_record(Some("".to_string()), Some(5), 200, None), // Empty ID = append-only
            create_record(Some("null".to_string()), Some(1), 300, None), // "null" = append-only
            create_record(Some("  ".to_string()), None, 400, None),  // Blank = append-only
            // Versioned records
            create_record(Some("real_id".to_string()), Some(1), 150, None),
            create_record(Some("real_id".to_string()), Some(2), 250, None),
        ];

        let resolved = resolver.resolve_batch(records);

        // Should have 4 append-only + 1 versioned = 5 total
        assert_eq!(resolved.len(), 5);

        // Count by type
        let mut append_only_count = 0;
        let mut versioned_count = 0;

        for record in &resolved {
            let id = record.id.as_str();
            if id.is_empty() || id.trim().is_empty() || id == "null" || id == "none" {
                append_only_count += 1;
            } else {
                versioned_count += 1;
            }
        }

        assert_eq!(append_only_count, 4);
        assert_eq!(versioned_count, 1);

        // Find the versioned record
        let versioned_record = resolved
            .iter()
            .find(|r| r.id.as_str() == "real_id")
            .unwrap();
        assert_eq!(versioned_record.version, Some(2)); // Should get highest version
    }

    #[test]
    fn test_expired_records_mark_entire_id_deleted() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            // ID1: Has one expired version - all should be deleted
            create_record(Some("id1".to_string()), Some(1), 100, None),
            create_record(Some("id1".to_string()), Some(2), 200, Some(500)), // Expired
            create_record(Some("id1".to_string()), Some(3), 300, None),
            // ID2: No expired versions - should be included
            create_record(Some("id2".to_string()), Some(1), 110, None),
            create_record(Some("id2".to_string()), Some(2), 210, None),
            // ID3: Expired version is not the latest
            create_record(Some("id3".to_string()), Some(1), 120, Some(800)), // Not expired
            create_record(Some("id3".to_string()), Some(2), 220, Some(500)), // Expired
            create_record(Some("id3".to_string()), Some(3), 320, None),
            // Append-only records should not be affected by expiry rules
            create_record(None, None, 130, Some(500)), // Expired append-only
        ];

        let resolved = resolver.resolve_batch(records);

        // Should have: only id2 (no expired versions)
        // id1 and id3 should be completely excluded due to expired versions
        // append-only expired record should also be excluded
        assert_eq!(resolved.len(), 1);

        let mut by_id: std::collections::HashMap<String, &VectorRecord> =
            std::collections::HashMap::new();
        for record in &resolved {
            by_id.insert(record.id.clone(), record);
        }

        assert_eq!(by_id.len(), 1);
        // Expired append-only is excluded
        assert!(!by_id.contains_key(""));
        assert!(by_id.contains_key("id2"));
        assert_eq!(by_id.get("id2").unwrap().version, Some(2));
    }

    #[test]
    fn test_compare_records_function() {
        let resolver = MvccResolver::with_timestamp(1000);

        // Test version comparison
        let record_v1 = create_record(Some("id".to_string()), Some(1), 100, None);
        let record_v2 = create_record(Some("id".to_string()), Some(2), 200, None);

        assert!(resolver.compare_records(&record_v2, &record_v1)); // v2 should win over v1
        assert!(!resolver.compare_records(&record_v1, &record_v2)); // v1 should not win over v2

        // Test timestamp comparison for same version
        let record_early = create_record(Some("id".to_string()), Some(1), 100, None);
        let record_late = create_record(Some("id".to_string()), Some(1), 200, None);

        assert!(resolver.compare_records(&record_early, &record_late)); // Earlier timestamp wins
        assert!(!resolver.compare_records(&record_late, &record_early)); // Later timestamp loses

        // Test expiry handling
        let record_expired = create_record(Some("id".to_string()), Some(2), 100, Some(500)); // Expired
        let record_valid = create_record(Some("id".to_string()), Some(1), 200, None);

        assert!(resolver.compare_records(&record_valid, &record_expired)); // Valid should win over expired
        assert!(!resolver.compare_records(&record_expired, &record_valid)); // Expired should not win over valid

        // Test None version treated as 1
        let record_none_version = create_record(Some("id".to_string()), None, 100, None);
        let record_v1_explicit = create_record(Some("id".to_string()), Some(1), 200, None);

        assert!(resolver.compare_records(&record_none_version, &record_v1_explicit)); // Earlier timestamp wins for same version
        assert!(!resolver.compare_records(&record_v1_explicit, &record_none_version)); // Later timestamp loses
    }

    #[test]
    fn test_edge_case_zero_version() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            create_record(Some("id1".to_string()), Some(0), 100, None), // Version 0 should still work
            create_record(Some("id1".to_string()), Some(1), 200, None),
        ];

        let resolved = resolver.resolve_batch(records);

        // Version 0 is valid, should get version 1 as next continuous version
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].version, Some(1));
    }

    #[test]
    fn test_large_version_numbers() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            create_record(Some("id1".to_string()), Some(1), 100, None),
            create_record(Some("id1".to_string()), Some(2), 200, None),
            create_record(Some("id1".to_string()), Some(u32::MAX), 300, None), // Very large version gap
        ];

        let resolved = resolver.resolve_batch(records);

        // Should stop at version 2 due to gap to MAX
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].version, Some(2));
    }

    #[test]
    fn test_empty_batch() {
        let resolver = MvccResolver::with_timestamp(1000);
        let records: Vec<VectorRecord> = vec![];

        let resolved = resolver.resolve_batch(records);
        assert_eq!(resolved.len(), 0);
    }

    #[test]
    fn test_all_records_expired() {
        let resolver = MvccResolver::with_timestamp(1000);

        let records = vec![
            create_record(Some("id1".to_string()), Some(1), 100, Some(500)), // Expired
            create_record(Some("id2".to_string()), Some(1), 200, Some(600)), // Expired
            create_record(None, None, 300, Some(700)),                       // Expired append-only
        ];

        let resolved = resolver.resolve_batch(records);

        // All records should be excluded due to expiry
        assert_eq!(resolved.len(), 0);
    }
}

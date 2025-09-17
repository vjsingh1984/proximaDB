//! Unit Tests for MVCC Logic
//!
//! Tests the core MVCC version resolution logic independently of storage engines

use proximadb::proto::proximadb_v1::SearchResult;
use serde_json::json;
use std::collections::HashMap;

/// Test helper to create a SearchResult with specific ID, version, and timestamp
fn create_search_result(id: &str, version: u32, timestamp: u32, score: f32) -> SearchResult {
    SearchResult {
        id: id.to_string(),
        vector_id: Some(format!("vec_{}", id)),
        score,
        distance: Some(1.0 - score),
        rank: None,
        vector: Some(vec![0.1; 128]),
        metadata: HashMap::new(),
        debug_info: None,
        semantic_distance: None,
        quantization_info: None,
        engine_stats: None,
        index_path: None,
        version: Some(version),
        timestamp: Some(timestamp),
        created_at: None,
    }
}

/// Test the MVCC logic directly
fn apply_mvcc_logic(results: Vec<SearchResult>) -> Vec<SearchResult> {
    use std::collections::HashMap;

    // Group results by ID
    let mut id_groups: HashMap<String, Vec<SearchResult>> = HashMap::new();
    let mut results_without_id = Vec::new();

    for result in results {
        if result.id.is_empty() {
            // Vectors without IDs are append-only, no deduplication
            results_without_id.push(result);
        } else {
            id_groups
                .entry(result.id.clone())
                .or_insert_with(Vec::new)
                .push(result);
        }
    }

    // Process each ID group
    let mut deduplicated = Vec::new();

    for (_id, mut versions) in id_groups {
        // Sort by version, then timestamp (earliest first for same version)
        versions.sort_by(|a, b| {
            let version_a = a.version.unwrap_or(1);
            let version_b = b.version.unwrap_or(1);

            version_a.cmp(&version_b).then_with(|| {
                // For same version, earliest timestamp wins
                let ts_a = a.timestamp.unwrap_or(u32::MAX);
                let ts_b = b.timestamp.unwrap_or(u32::MAX);
                ts_a.cmp(&ts_b)
            })
        });

        // Validate version continuity
        let mut expected_version = 1;
        let mut last_valid: Option<SearchResult> = None;

        for result in versions {
            let version = result.version.unwrap_or(1);

            if version == expected_version {
                // Check for duplicate version - keep earliest timestamp
                if let Some(ref existing) = last_valid {
                    if existing.version == result.version {
                        let existing_ts = existing.timestamp.unwrap_or(u32::MAX);
                        let current_ts = result.timestamp.unwrap_or(u32::MAX);
                        if current_ts < existing_ts {
                            last_valid = Some(result);
                        }
                        continue;
                    }
                }
                last_valid = Some(result);
                expected_version += 1;
            } else if version > expected_version {
                // Version gap detected - stop processing
                break;
            }
            // Skip older versions
        }

        if let Some(result) = last_valid {
            deduplicated.push(result);
        }
    }

    // Add back results without IDs
    deduplicated.extend(results_without_id);

    // Sort by score
    deduplicated.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    deduplicated
}

#[test]
fn test_mvcc_normal_progression() {
    let results = vec![
        create_search_result("doc1", 1, 100, 0.9),
        create_search_result("doc1", 2, 200, 0.8),
        create_search_result("doc1", 3, 300, 0.7),
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].id, "doc1");
    assert_eq!(deduplicated[0].version, Some(3));
}

#[test]
fn test_mvcc_version_gap() {
    let results = vec![
        create_search_result("doc2", 1, 100, 0.9),
        create_search_result("doc2", 2, 200, 0.8),
        create_search_result("doc2", 4, 400, 0.6), // Gap at v3
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].id, "doc2");
    assert_eq!(deduplicated[0].version, Some(2)); // Stops at gap
}

#[test]
fn test_mvcc_duplicate_versions() {
    let results = vec![
        create_search_result("doc3", 1, 300, 0.7),
        create_search_result("doc3", 1, 100, 0.9), // Earlier timestamp
        create_search_result("doc3", 1, 200, 0.8),
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].id, "doc3");
    assert_eq!(deduplicated[0].version, Some(1));
    assert_eq!(deduplicated[0].timestamp, Some(100)); // Earliest wins
}

#[test]
fn test_mvcc_multiple_documents() {
    let results = vec![
        create_search_result("doc4", 1, 100, 0.95),
        create_search_result("doc4", 2, 200, 0.94),
        create_search_result("doc5", 1, 150, 0.85),
        create_search_result("doc5", 2, 250, 0.84),
        create_search_result("doc5", 3, 350, 0.83),
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 2);

    // Should be sorted by score
    assert_eq!(deduplicated[0].id, "doc4");
    assert_eq!(deduplicated[0].version, Some(2));

    assert_eq!(deduplicated[1].id, "doc5");
    assert_eq!(deduplicated[1].version, Some(3));
}

#[test]
fn test_mvcc_append_only_vectors() {
    let results = vec![
        create_search_result("", 1, 100, 0.9),
        create_search_result("", 1, 200, 0.8),
        create_search_result("", 1, 300, 0.7),
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 3); // All preserved
}

#[test]
fn test_mvcc_mixed_with_and_without_ids() {
    let results = vec![
        create_search_result("doc6", 1, 100, 0.95),
        create_search_result("doc6", 2, 200, 0.94),
        create_search_result("", 1, 150, 0.85),
        create_search_result("", 1, 250, 0.84),
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 3); // 1 for doc6, 2 for append-only

    let with_id = deduplicated.iter().filter(|r| !r.id.is_empty()).count();
    let without_id = deduplicated.iter().filter(|r| r.id.is_empty()).count();
    assert_eq!(with_id, 1);
    assert_eq!(without_id, 2);
}

#[test]
fn test_mvcc_complex_scenario() {
    let results = vec![
        // Doc A: Normal progression
        create_search_result("docA", 1, 100, 0.9),
        create_search_result("docA", 2, 200, 0.9),
        create_search_result("docA", 3, 300, 0.9),
        // Doc B: Version gap
        create_search_result("docB", 1, 100, 0.8),
        create_search_result("docB", 2, 200, 0.8),
        create_search_result("docB", 4, 400, 0.8),
        // Doc C: Duplicate versions
        create_search_result("docC", 1, 300, 0.7),
        create_search_result("docC", 1, 100, 0.7), // Earlier
        create_search_result("docC", 1, 200, 0.7),
        create_search_result("docC", 2, 400, 0.7),
        // No ID vectors
        create_search_result("", 1, 500, 0.6),
        create_search_result("", 1, 600, 0.5),
    ];

    let deduplicated = apply_mvcc_logic(results);

    // Should have: docA v3, docB v2, docC v2, and 2 no-ID vectors = 5 total
    assert_eq!(deduplicated.len(), 5);

    // Verify specific results
    let doc_a = deduplicated.iter().find(|r| r.id == "docA").unwrap();
    assert_eq!(doc_a.version, Some(3));

    let doc_b = deduplicated.iter().find(|r| r.id == "docB").unwrap();
    assert_eq!(doc_b.version, Some(2)); // Stopped at gap

    let doc_c = deduplicated.iter().find(|r| r.id == "docC").unwrap();
    assert_eq!(doc_c.version, Some(2));

    let no_id_count = deduplicated.iter().filter(|r| r.id.is_empty()).count();
    assert_eq!(no_id_count, 2);
}

/// Test SST-style compaction logic
fn should_replace_record(
    existing_version: u32,
    existing_ts: u32,
    new_version: u32,
    new_ts: u32,
    has_id: bool,
) -> bool {
    if !has_id {
        // For append-only records, newer always wins
        return new_ts > existing_ts;
    }

    // For records with IDs, apply MVCC version resolution
    match new_version.cmp(&existing_version) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => {
            // Same version - earliest timestamp wins
            new_ts < existing_ts
        }
        std::cmp::Ordering::Less => false,
    }
}

/// Test MVCC version resolution logic used during SST compaction
///
/// Validates the logic for determining which record version should be kept
/// when multiple versions of the same record exist during compaction.
#[test]
fn test_mvcc_version_resolution_logic_for_compaction() {
    // Test with ID - higher version wins
    assert!(should_replace_record(1, 100, 2, 200, true));
    assert!(!should_replace_record(2, 200, 1, 100, true));

    // Test with ID - same version, earlier timestamp wins
    assert!(should_replace_record(1, 200, 1, 100, true));
    assert!(!should_replace_record(1, 100, 1, 200, true));

    // Test without ID - newer timestamp wins
    assert!(should_replace_record(1, 100, 1, 200, false));
    assert!(!should_replace_record(1, 200, 1, 100, false));
}

#[test]
fn test_version_continuity_validation() {
    // Helper to check if versions are continuous starting from 1
    fn is_version_continuous(versions: &[u32]) -> bool {
        if versions.is_empty() {
            return true;
        }

        let mut sorted = versions.to_vec();
        sorted.sort();

        if sorted[0] != 1 {
            return false;
        }

        for i in 1..sorted.len() {
            if sorted[i] != sorted[i - 1] + 1 {
                return false;
            }
        }

        true
    }

    // Test continuous versions
    assert!(is_version_continuous(&[1, 2, 3, 4, 5]));

    // Test with gap
    assert!(!is_version_continuous(&[1, 2, 4, 5]));

    // Test single version
    assert!(is_version_continuous(&[1]));

    // Test empty
    assert!(is_version_continuous(&[]));

    // Test starting from non-1
    assert!(!is_version_continuous(&[2, 3, 4]));
}

#[test]
fn test_mvcc_edge_cases() {
    // Test with missing version field (defaults to 1)
    let mut result = create_search_result("doc1", 1, 100, 0.9);
    result.version = None;

    let results = vec![result];
    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);

    // Test with missing timestamp field
    let mut result = create_search_result("doc2", 1, 100, 0.9);
    result.timestamp = None;

    let results = vec![result, create_search_result("doc2", 1, 200, 0.8)];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    // Should pick the one with timestamp 200 since None defaults to u32::MAX
    assert_eq!(deduplicated[0].timestamp, Some(200));
}

#[test]
fn test_mvcc_result_sorting() {
    let results = vec![
        create_search_result("doc1", 1, 100, 0.5),
        create_search_result("doc2", 1, 100, 0.9),
        create_search_result("doc3", 1, 100, 0.7),
        create_search_result("doc4", 1, 100, 0.3),
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 4);

    // Verify sorted by score (descending)
    assert_eq!(deduplicated[0].score, 0.9); // doc2
    assert_eq!(deduplicated[1].score, 0.7); // doc3
    assert_eq!(deduplicated[2].score, 0.5); // doc1
    assert_eq!(deduplicated[3].score, 0.3); // doc4
}

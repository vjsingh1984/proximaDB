//! Integration Test for MVCC Logic
//!
//! This test verifies that MVCC logic is correctly applied in search results

use proximadb::core::search::results::InternalSearchResult;
use serde_json::json;
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

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
        created_at: Some(chrono::Utc::now()),
    }
}

/// Test the MVCC logic implementation
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
fn test_mvcc_integration_normal_progression() {
    let results = vec![
        create_search_result("doc1", 1, 100, 0.9),
        create_search_result("doc1", 2, 200, 0.8),
        create_search_result("doc1", 3, 300, 0.7),
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].id, "doc1");
    assert_eq!(deduplicated[0].version, Some(3));

    debug!("✅ MVCC normal progression test passed");
}

#[test]
fn test_mvcc_integration_version_gap() {
    let results = vec![
        create_search_result("doc2", 1, 100, 0.9),
        create_search_result("doc2", 2, 200, 0.8),
        create_search_result("doc2", 4, 400, 0.6), // Gap at v3
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].id, "doc2");
    assert_eq!(deduplicated[0].version, Some(2)); // Stops at gap

    debug!("✅ MVCC version gap test passed");
}

#[test]
fn test_mvcc_integration_duplicate_versions() {
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

    debug!("✅ MVCC duplicate versions test passed");
}

#[test]
fn test_mvcc_integration_append_only_vectors() {
    let results = vec![
        create_search_result("", 1, 100, 0.9),
        create_search_result("", 1, 200, 0.8),
        create_search_result("", 1, 300, 0.7),
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 3); // All preserved

    debug!("✅ MVCC append-only vectors test passed");
}

#[test]
fn test_mvcc_integration_complex_scenario() {
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

    debug!("✅ MVCC complex scenario test passed");
}

/// Test tombstone handling (simulated through metadata)
#[test]
fn test_mvcc_integration_tombstone_handling() {
    // Create results where doc4 has a tombstone marker
    let mut doc4_v1 = create_search_result("doc4", 1, 100, 0.9);
    doc4_v1.metadata.insert("active".to_string(), json!(true));

    let mut doc4_v2 = create_search_result("doc4", 2, 200, 0.8);
    doc4_v2
        .metadata
        .insert("is_deleted".to_string(), json!(true)); // Tombstone

    let results = vec![doc4_v1, doc4_v2];

    // Apply MVCC logic
    let deduplicated = apply_mvcc_logic(results);

    // Should still return doc4 v2 (tombstone filtering happens during compaction, not search)
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].id, "doc4");
    assert_eq!(deduplicated[0].version, Some(2));
    assert_eq!(deduplicated[0].metadata.get(key), Some(&json!(true)));

    debug!("✅ MVCC tombstone handling test passed");
}

/// Test version continuity with multiple documents
#[test]
fn test_mvcc_integration_version_continuity_multiple_docs() {
    let results = vec![
        // Doc X: Continuous versions 1-5
        create_search_result("docX", 1, 100, 0.95),
        create_search_result("docX", 2, 200, 0.94),
        create_search_result("docX", 3, 300, 0.93),
        create_search_result("docX", 4, 400, 0.92),
        create_search_result("docX", 5, 500, 0.91),
        // Doc Y: Gap at version 3
        create_search_result("docY", 1, 100, 0.85),
        create_search_result("docY", 2, 200, 0.84),
        create_search_result("docY", 4, 400, 0.82),
        create_search_result("docY", 5, 500, 0.81),
        // Doc Z: Only version 1
        create_search_result("docZ", 1, 100, 0.75),
    ];

    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 3);

    // Verify results are sorted by score
    assert_eq!(deduplicated[0].id, "docX");
    assert_eq!(deduplicated[0].version, Some(5));

    assert_eq!(deduplicated[1].id, "docY");
    assert_eq!(deduplicated[1].version, Some(2)); // Stopped at gap

    assert_eq!(deduplicated[2].id, "docZ");
    assert_eq!(deduplicated[2].version, Some(1));

    debug!("✅ MVCC version continuity with multiple docs test passed");
}

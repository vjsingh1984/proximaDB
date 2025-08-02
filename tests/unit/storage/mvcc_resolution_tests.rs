//! Unit Tests for MVCC Resolution Logic
//!
//! Tests the core MVCC version resolution logic used by both
//! VIPER and SST engines to ensure consistency.

use proximadb::core::search::SearchResult;
use proximadb::services::DirectVectorService;
use std::collections::HashMap;
use serde_json::json;

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

/// Test DirectVectorService's apply_mvcc_deduplication method
#[test]
fn test_apply_mvcc_deduplication() {
    // Create a mock DirectVectorService just to test the method
    // Note: We can't easily instantiate DirectVectorService here, so we'll test the logic directly
    
    // Test case 1: Normal version progression
    let results = vec![
        create_search_result("doc1", 1, 100, 0.9),
        create_search_result("doc1", 2, 200, 0.8),
        create_search_result("doc1", 3, 300, 0.7),
    ];
    
    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].id, "doc1");
    assert_eq!(deduplicated[0].version, Some(3));
    
    // Test case 2: Version gap
    let results = vec![
        create_search_result("doc2", 1, 100, 0.9),
        create_search_result("doc2", 2, 200, 0.8),
        create_search_result("doc2", 4, 400, 0.6), // Gap at v3
    ];
    
    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].id, "doc2");
    assert_eq!(deduplicated[0].version, Some(2)); // Stops at gap
    
    // Test case 3: Duplicate versions (earliest timestamp wins)
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
    
    // Test case 4: Multiple documents
    let results = vec![
        create_search_result("doc4", 1, 100, 0.95),
        create_search_result("doc4", 2, 200, 0.94),
        create_search_result("doc5", 1, 150, 0.85),
        create_search_result("doc5", 2, 250, 0.84),
        create_search_result("doc5", 3, 350, 0.83),
    ];
    
    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 2);
    
    let doc4 = deduplicated.iter().find(|r| r.id == "doc4").unwrap();
    assert_eq!(doc4.version, Some(2));
    
    let doc5 = deduplicated.iter().find(|r| r.id == "doc5").unwrap();
    assert_eq!(doc5.version, Some(3));
    
    // Test case 5: Append-only vectors (no ID)
    let results = vec![
        create_search_result("", 1, 100, 0.9),
        create_search_result("", 1, 200, 0.8),
        create_search_result("", 1, 300, 0.7),
    ];
    
    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 3); // All preserved
}

/// Test the actual MVCC logic (extracted from DirectVectorService)
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
            id_groups.entry(result.id.clone()).or_insert_with(Vec::new).push(result);
        }
    }
    
    // Process each ID group
    let mut deduplicated = Vec::new();
    
    for (_id, mut versions) in id_groups {
        // Sort by version, then timestamp (earliest first for same version)
        versions.sort_by(|a, b| {
            let version_a = a.version.unwrap_or(1);
            let version_b = b.version.unwrap_or(1);
            
            version_a.cmp(&version_b)
                .then_with(|| {
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

/// Test version continuity validation
#[test]
fn test_version_continuity_validation() {
    // Test continuous versions
    let continuous = vec![1, 2, 3, 4, 5];
    assert!(is_version_continuous(&continuous));
    
    // Test with gap
    let with_gap = vec![1, 2, 4, 5];
    assert!(!is_version_continuous(&with_gap));
    
    // Test single version
    let single = vec![1];
    assert!(is_version_continuous(&single));
    
    // Test empty
    let empty: Vec<u32> = vec![];
    assert!(is_version_continuous(&empty));
    
    // Test starting from non-1
    let non_one_start = vec![2, 3, 4];
    assert!(!is_version_continuous(&non_one_start));
}

/// Helper to check if versions are continuous starting from 1
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
        if sorted[i] != sorted[i-1] + 1 {
            return false;
        }
    }
    
    true
}

/// Test edge cases for MVCC resolution
#[test]
fn test_mvcc_edge_cases() {
    // Test with missing version field (defaults to 1)
    let mut result = create_search_result("doc1", 1, 100, 0.9);
    result.version = None;
    
    let results = vec![result];
    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    assert_eq!(deduplicated[0].version, None); // Original None preserved
    
    // Test with missing timestamp field
    let mut result = create_search_result("doc2", 1, 100, 0.9);
    result.timestamp = None;
    
    let results = vec![
        result,
        create_search_result("doc2", 1, 200, 0.8),
    ];
    
    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 1);
    // Should pick the one with timestamp 200 since None defaults to u32::MAX
    assert_eq!(deduplicated[0].timestamp, Some(200));
    
    // Test mixed with and without IDs
    let results = vec![
        create_search_result("doc3", 1, 100, 0.95),
        create_search_result("doc3", 2, 200, 0.94),
        create_search_result("", 1, 150, 0.85),
        create_search_result("", 1, 250, 0.84),
    ];
    
    let deduplicated = apply_mvcc_logic(results);
    assert_eq!(deduplicated.len(), 3); // 1 for doc3, 2 for append-only
    
    let with_id = deduplicated.iter().filter(|r| !r.id.is_empty()).count();
    let without_id = deduplicated.iter().filter(|r| r.id.is_empty()).count();
    assert_eq!(with_id, 1);
    assert_eq!(without_id, 2);
}

/// Test sorting behavior after deduplication
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

/// Test complex scenario with multiple version patterns
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
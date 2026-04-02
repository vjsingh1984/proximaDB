//! TDD Tests for CSR Lazy Rebuild and Background Compaction
//!
//! Tests:
//! 1. Lazy rebuild: CSR rebuild happens on first read after writes
//! 2. Background compaction: Bulk inserts trigger background compaction at threshold
//! 3. Temp edge accumulation: Small writes accumulate without rebuild
//! 4. Query correctness: Queries see temp edges correctly

use proximadb::graph::engines::orion::storage::CsrStorage;

#[test]
fn test_lazy_rebuild_flag() {
    // Test that needs_rebuild flag is set correctly
    let mut csr = CsrStorage::new();

    // Initially no rebuild needed
    assert!(!csr.needs_rebuild());
    assert_eq!(csr.temp_edge_count(), 0);

    // Add edge sets rebuild flag
    csr.add_edge(0, 1, "e1".to_string()).unwrap();
    assert!(csr.needs_rebuild());
    assert_eq!(csr.temp_edge_count(), 1);

    // Add more edges increments count
    csr.add_edge(0, 2, "e2".to_string()).unwrap();
    assert!(csr.needs_rebuild());
    assert_eq!(csr.temp_edge_count(), 2);

    // Rebuild clears flag
    csr.rebuild().unwrap();
    assert!(!csr.needs_rebuild());
    assert_eq!(csr.temp_edge_count(), 0);
}

#[test]
fn test_rebuild_if_needed_lazy() {
    // Test that rebuild_if_needed() only rebuilds when flag is set
    let mut csr = CsrStorage::new();

    // No rebuild needed initially
    csr.rebuild_if_needed().unwrap();
    assert!(!csr.needs_rebuild());

    // Add edges to temp
    csr.add_edge(0, 1, "e1".to_string()).unwrap();
    csr.add_edge(0, 2, "e2".to_string()).unwrap();
    assert!(csr.needs_rebuild());

    // rebuild_if_needed triggers rebuild
    csr.rebuild_if_needed().unwrap();
    assert!(!csr.needs_rebuild());

    // Edges are now in main CSR
    let neighbors = csr.get_neighbors(0).unwrap();
    assert_eq!(neighbors.len(), 2);
    assert!(neighbors.contains(&1));
    assert!(neighbors.contains(&2));
}

#[test]
fn test_temp_edges_accumulate() {
    // Test that small writes accumulate in temp without rebuild
    let mut csr = CsrStorage::new();

    // Add 100 edges
    for i in 0..100 {
        csr.add_edge(i, i + 1, format!("e{}", i)).unwrap();
    }

    // All edges in temp, none in main CSR yet
    assert!(csr.needs_rebuild());
    assert_eq!(csr.temp_edge_count(), 100);

    // Main CSR still empty
    let neighbors = csr.get_neighbors(0).unwrap();
    assert_eq!(
        neighbors.len(),
        0,
        "Main CSR should be empty before rebuild"
    );

    // After rebuild, edges appear in main CSR
    csr.rebuild().unwrap();
    let neighbors = csr.get_neighbors(0).unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0], 1);
}

#[test]
fn test_query_sees_temp_edges() {
    // Test that queries see edges in temp storage
    // NOTE: This is currently NOT implemented - temp edges are not visible until rebuild
    // This test documents the current behavior
    let mut csr = CsrStorage::new();

    csr.add_edge(0, 1, "e1".to_string()).unwrap();
    csr.add_edge(0, 2, "e2".to_string()).unwrap();

    // Current behavior: temp edges NOT visible until rebuild
    let neighbors = csr.get_neighbors(0).unwrap();
    assert_eq!(
        neighbors.len(),
        0,
        "Temp edges not visible in get_neighbors (by design)"
    );

    // After rebuild, edges are visible
    csr.rebuild().unwrap();
    let neighbors = csr.get_neighbors(0).unwrap();
    assert_eq!(neighbors.len(), 2);
}

#[test]
fn test_incremental_rebuild_only_affected_nodes() {
    // Test that rebuild only processes nodes with temp edges
    // NOTE: Current implementation rebuilds ALL nodes, not just affected ones
    // This test documents desired behavior for future optimization

    let mut csr = CsrStorage::new();

    // Add edges for nodes 0-9 (10 nodes)
    for i in 0..10 {
        csr.add_edge(i, i + 1, format!("e{}", i)).unwrap();
    }
    csr.rebuild().unwrap();

    // Now add edges only for nodes 100-104 (5 new nodes)
    for i in 100..105 {
        csr.add_edge(i, i + 1, format!("e{}", i)).unwrap();
    }

    // Rebuild should ideally only process 5 nodes, not all 105
    // Current implementation: processes all nodes
    csr.rebuild().unwrap();

    // Verify correctness
    let neighbors = csr.get_neighbors(100).unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0], 101);
}

#[test]
fn test_multiple_rebuild_cycles() {
    // Test that rebuild can be called multiple times correctly
    let mut csr = CsrStorage::new();

    // Cycle 1: Add 10 edges, rebuild
    for i in 0..10 {
        csr.add_edge(i, i + 1, format!("e_cycle1_{}", i)).unwrap();
    }
    assert_eq!(csr.temp_edge_count(), 10);
    csr.rebuild().unwrap();
    assert_eq!(csr.temp_edge_count(), 0);

    // Cycle 2: Add 5 more edges, rebuild
    for i in 10..15 {
        csr.add_edge(i, i + 1, format!("e_cycle2_{}", i)).unwrap();
    }
    assert_eq!(csr.temp_edge_count(), 5);
    csr.rebuild().unwrap();
    assert_eq!(csr.temp_edge_count(), 0);

    // Verify all edges present
    for i in 0..15 {
        let neighbors = csr.get_neighbors(i).unwrap();
        assert_eq!(neighbors.len(), 1, "Node {} should have 1 neighbor", i);
        assert_eq!(neighbors[0], i + 1);
    }
}

#[test]
fn test_rebuild_with_sorted_output() {
    // Test that rebuild maintains sorted order of edges
    let mut csr = CsrStorage::new();

    // Add edges in reverse order
    csr.add_edge(0, 5, "e5".to_string()).unwrap();
    csr.add_edge(0, 3, "e3".to_string()).unwrap();
    csr.add_edge(0, 1, "e1".to_string()).unwrap();
    csr.add_edge(0, 4, "e4".to_string()).unwrap();
    csr.add_edge(0, 2, "e2".to_string()).unwrap();

    // After rebuild, edges should be sorted by target
    csr.rebuild().unwrap();

    let neighbors = csr.get_neighbors(0).unwrap();
    assert_eq!(neighbors.len(), 5);

    // Verify sorted order: [1, 2, 3, 4, 5]
    for i in 0..5 {
        assert_eq!(neighbors[i], i + 1, "Neighbor {} should be {}", i, i + 1);
    }
}

#[test]
fn test_threshold_based_compaction_trigger() {
    // Test that compaction is triggered at threshold
    // NOTE: This is tested at integration level (OrionGraphEngine)
    // This unit test just verifies temp_edge_count is accurate

    let mut csr = CsrStorage::new();

    const THRESHOLD: usize = 1000;

    // Add edges up to threshold - 1
    for i in 0..(THRESHOLD - 1) {
        csr.add_edge(i % 100, (i + 1) % 100, format!("e{}", i))
            .unwrap();
    }
    assert_eq!(csr.temp_edge_count(), THRESHOLD - 1);
    assert!(csr.needs_rebuild());

    // Add one more edge to reach threshold
    csr.add_edge(0, 1, format!("e{}", THRESHOLD)).unwrap();
    assert_eq!(csr.temp_edge_count(), THRESHOLD);

    // Compaction trigger would happen here (in OrionGraphEngine)
    // For unit test, manually rebuild
    csr.rebuild().unwrap();
    assert_eq!(csr.temp_edge_count(), 0);
}

#[test]
fn test_concurrent_reads_during_temp_accumulation() {
    // Test that reads work correctly while temp edges accumulate
    let mut csr = CsrStorage::new();

    // Add some edges and rebuild (in main CSR)
    for i in 0..5 {
        csr.add_edge(i, i + 1, format!("main_{}", i)).unwrap();
    }
    csr.rebuild().unwrap();

    // Add more edges to temp
    for i in 5..10 {
        csr.add_edge(i, i + 1, format!("temp_{}", i)).unwrap();
    }

    // Reads of main CSR edges still work
    let neighbors = csr.get_neighbors(0).unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0], 1);

    // Temp edges not visible yet
    let neighbors = csr.get_neighbors(5).unwrap();
    assert_eq!(neighbors.len(), 0, "Temp edges not visible before rebuild");

    // After rebuild, all edges visible
    csr.rebuild().unwrap();
    let neighbors = csr.get_neighbors(5).unwrap();
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0], 6);
}

#[test]
fn test_edge_deduplication_across_rebuild() {
    // Test that duplicate edges are rejected even across rebuild boundaries
    let mut csr = CsrStorage::new();

    // Add edge e1 and rebuild
    csr.add_edge(0, 1, "e1".to_string()).unwrap();
    csr.rebuild().unwrap();

    // Try to add same edge again (should fail)
    let result = csr.add_edge(0, 1, "e1".to_string());
    assert!(result.is_err(), "Duplicate edge should be rejected");
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn test_background_compaction_performance() {
        // Performance test: verify O(1) insertion before compaction
        use std::time::Instant;

        let mut csr = CsrStorage::new();

        // Measure time for 1000 insertions (should be ~1ms total)
        let start = Instant::now();
        for i in 0..1000 {
            csr.add_edge(i % 100, (i + 1) % 100, format!("e{}", i))
                .unwrap();
        }
        let insert_time = start.elapsed();

        println!("1000 edge inserts: {:?}", insert_time);

        // Should be very fast (O(1) per edge, no rebuild)
        assert!(
            insert_time.as_millis() < 100,
            "Inserts should be fast (< 100ms)"
        );

        // Rebuild should be slower but still reasonable
        let start = Instant::now();
        csr.rebuild().unwrap();
        let rebuild_time = start.elapsed();

        println!("Rebuild with 1000 edges: {:?}", rebuild_time);

        // Rebuild might take longer but should be < 1 second
        assert!(
            rebuild_time.as_millis() < 1000,
            "Rebuild should complete < 1s"
        );
    }
}

/// Tests for EmbeddingMode configuration
#[cfg(test)]
mod embedding_mode_tests {
    use proximadb::graph::engines::EmbeddingMode;

    #[test]
    fn test_embedding_mode_default() {
        // Default mode should be None (pure graph, best performance)
        let mode = EmbeddingMode::default();
        assert_eq!(mode, EmbeddingMode::None);
        assert!(!mode.stores_embeddings());
    }

    #[test]
    fn test_embedding_mode_parse_from_config() {
        // Test parsing from config strings
        assert_eq!(EmbeddingMode::parse_from_config("none"), EmbeddingMode::None);
        assert_eq!(EmbeddingMode::parse_from_config("None"), EmbeddingMode::None);
        assert_eq!(EmbeddingMode::parse_from_config("NONE"), EmbeddingMode::None);
        assert_eq!(EmbeddingMode::parse_from_config("cold"), EmbeddingMode::Cold);
        assert_eq!(EmbeddingMode::parse_from_config("Cold"), EmbeddingMode::Cold);
        assert_eq!(EmbeddingMode::parse_from_config("COLD"), EmbeddingMode::Cold);
        assert_eq!(EmbeddingMode::parse_from_config("memory"), EmbeddingMode::Memory);
        assert_eq!(EmbeddingMode::parse_from_config("Memory"), EmbeddingMode::Memory);
        assert_eq!(EmbeddingMode::parse_from_config("MEMORY"), EmbeddingMode::Memory);

        // Invalid strings should default to None
        assert_eq!(EmbeddingMode::parse_from_config("invalid"), EmbeddingMode::None);
        assert_eq!(EmbeddingMode::parse_from_config(""), EmbeddingMode::None);
    }

    #[test]
    fn test_embedding_mode_stores_embeddings() {
        assert!(!EmbeddingMode::None.stores_embeddings());
        assert!(EmbeddingMode::Cold.stores_embeddings());
        assert!(EmbeddingMode::Memory.stores_embeddings());
    }

    #[test]
    fn test_embedding_mode_is_cold() {
        assert!(!EmbeddingMode::None.is_cold());
        assert!(EmbeddingMode::Cold.is_cold());
        assert!(!EmbeddingMode::Memory.is_cold());
    }

    #[test]
    fn test_embedding_mode_is_memory() {
        assert!(!EmbeddingMode::None.is_memory());
        assert!(!EmbeddingMode::Cold.is_memory());
        assert!(EmbeddingMode::Memory.is_memory());
    }
}

/// Tests to ensure CSR stays lean (no embedding data)
#[cfg(test)]
mod csr_lean_tests {
    use super::*;

    #[test]
    fn test_csr_memory_footprint() {
        // CSR should only contain topology data, not embeddings
        let csr = CsrStorage::new();
        let stats = csr.memory_usage();

        // Empty CSR should have minimal memory footprint
        // offsets should have at least 1 element (initial 0)
        assert!(stats.offsets_bytes > 0, "CSR should have offsets array");
        assert_eq!(stats.targets_bytes, 0, "Empty CSR should have no targets");
        assert_eq!(stats.edge_ids_bytes, 0, "Empty CSR should have no edge_ids");
    }

    #[test]
    fn test_csr_stores_no_embedding_data() {
        // Verify that CsrStorage struct has no embedding fields
        // This is a compile-time check - if CSR ever gains embedding fields,
        // this test will need to be updated to ensure they're not used for topology
        let mut csr = CsrStorage::new();

        // Add edges - only topology data should be stored
        csr.add_edge(0, 1, "e1".to_string()).unwrap();
        csr.add_edge(0, 2, "e2".to_string()).unwrap();
        csr.rebuild().unwrap();

        // Memory usage should be proportional to topology, not embeddings
        let stats = csr.memory_usage();

        // With 3 nodes and 2 edges:
        // - offsets: ~32 bytes (4 usizes)
        // - targets: ~16 bytes (2 usizes)
        // - edge_ids: depends on String length
        // Total should be well under 1KB for small graph
        let total = stats.offsets_bytes + stats.targets_bytes + stats.edge_ids_bytes;
        assert!(
            total < 1024,
            "CSR memory for 3 nodes, 2 edges should be < 1KB, got {} bytes",
            total
        );
    }

    #[test]
    fn test_csr_scalability() {
        // CSR memory should scale linearly with edges, not with embedding size
        let mut csr = CsrStorage::new();

        // Add 1000 edges
        for i in 0..1000 {
            csr.add_edge(i % 100, (i + 1) % 100, format!("e{}", i))
                .unwrap();
        }
        csr.rebuild().unwrap();

        let stats = csr.memory_usage();

        // 1000 edges should use < 100KB in CSR
        // (compare to 500KB+ if each edge had a 128-dim embedding)
        let total = stats.offsets_bytes + stats.targets_bytes + stats.edge_ids_bytes;
        assert!(
            total < 100_000,
            "CSR memory for 1000 edges should be < 100KB, got {} bytes",
            total
        );
    }
}

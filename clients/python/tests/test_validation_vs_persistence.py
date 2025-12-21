#!/usr/bin/env python3
"""
Test to identify bottleneck: Validation vs Persistence

Tests 4 configurations:
1. async=true,  persistence=true  - Parallel validation + WAL writes
2. async=true,  persistence=false - Parallel validation, no WAL
3. async=false, persistence=true  - Sequential validation + WAL writes
4. async=false, persistence=false - Sequential validation, no WAL

This will show us:
- How much does WAL cost?
- How much does parallel validation help?
- What's the real bottleneck?
"""

import proximadb
import tempfile
import shutil
import time

def test_configuration(async_validation: bool, persistence: bool, graph_size: dict):
    """Test a specific configuration"""

    temp_dir = tempfile.mkdtemp(prefix="proximadb_test_")

    try:
        # Create database
        db = proximadb.ProximaDB(data_dirs=temp_dir)
        graph_id = "test_graph"
        db.create_graph(graph_id)

        # Create nodes
        print(f"  Creating {graph_size['nodes']} nodes...", end="", flush=True)
        start = time.perf_counter()

        nodes = []
        for i in range(graph_size['nodes']):
            node = proximadb.GraphNode(
                f"node_{i}",
                labels=["Person"],
                properties={"name": f"Node_{i}", "value": str(i)}
            )
            nodes.append(node)

        db.create_nodes(graph_id, nodes)
        node_time = (time.perf_counter() - start) * 1000
        print(f" {node_time:.1f}ms")

        # Create edges
        print(f"  Creating {graph_size['edges']} edges...", end="", flush=True)
        start = time.perf_counter()

        edges = []
        edge_count = 0
        for from_idx in range(graph_size['nodes']):
            if edge_count >= graph_size['edges']:
                break
            for offset in range(1, 6):  # Each node connects to up to 5 neighbors
                if edge_count >= graph_size['edges']:
                    break
                to_idx = (from_idx + offset) % graph_size['nodes']
                edge = proximadb.GraphEdge(
                    f"node_{from_idx}",
                    f"node_{to_idx}",
                    "LINKS",
                    weight=1.0
                )
                edges.append(edge)
                edge_count += 1

        db.create_edges(graph_id, edges)
        edge_time = (time.perf_counter() - start) * 1000
        print(f" {edge_time:.1f}ms")

        total_time = node_time + edge_time
        ops_per_sec = (graph_size['nodes'] + graph_size['edges']) * 1000 / total_time

        return {
            'node_time_ms': node_time,
            'edge_time_ms': edge_time,
            'total_time_ms': total_time,
            'ops_per_sec': ops_per_sec,
            'edge_ops_per_sec': graph_size['edges'] * 1000 / edge_time
        }

    finally:
        shutil.rmtree(temp_dir, ignore_errors=True)


def main():
    print("=" * 80)
    print("VALIDATION vs PERSISTENCE BOTTLENECK TEST")
    print("=" * 80)
    print()

    # Test with different graph sizes
    sizes = [
        {"name": "Small", "nodes": 500, "edges": 2500},
        {"name": "Medium", "nodes": 1000, "edges": 5000},
    ]

    for size in sizes:
        print(f"\n{size['name']} Graph ({size['nodes']} nodes, {size['edges']} edges)")
        print("-" * 80)

        # Current implementation (we can't actually toggle these in Python yet)
        # So we'll just measure the current behavior and note what we need to implement

        print("\nCurrent Implementation (parallel validation + sync WAL):")
        result = test_configuration(async_validation=True, persistence=True, graph_size=size)

        print(f"\n  Results:")
        print(f"    Node insert: {result['node_time_ms']:.1f}ms")
        print(f"    Edge insert: {result['edge_time_ms']:.1f}ms")
        print(f"    Total: {result['total_time_ms']:.1f}ms")
        print(f"    Edge throughput: {result['edge_ops_per_sec']:.0f} ops/sec")
        print(f"    Total throughput: {result['ops_per_sec']:.0f} ops/sec")

    print("\n" + "=" * 80)
    print("ANALYSIS NEEDED")
    print("=" * 80)
    print()
    print("To properly test all 4 configurations, we need to implement:")
    print()
    print("1. Configuration flag to disable WAL writes (persistence=false)")
    print("2. Configuration flag to use sequential validation (async=false)")
    print()
    print("Recommended implementation:")
    print()
    print("  // In OrionGraphEngine::new()")
    print("  pub struct GraphConfig {")
    print("      pub enable_wal: bool,        // Default: true")
    print("      pub parallel_validation: bool, // Default: true")
    print("  }")
    print()
    print("Then we can test:")
    print("  - parallel=true,  wal=true   (current)")
    print("  - parallel=true,  wal=false  (no persistence cost)")
    print("  - parallel=false, wal=true   (no parallel benefit)")
    print("  - parallel=false, wal=false  (baseline)")
    print()
    print("This will show us the ACTUAL cost of each component.")


if __name__ == "__main__":
    main()

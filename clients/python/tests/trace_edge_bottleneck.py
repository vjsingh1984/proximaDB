#!/usr/bin/env python3
"""Trace edge insertion bottleneck with detailed timing"""

import shutil
import tempfile
import time

import proximadb

temp_dir = tempfile.mkdtemp(prefix="proximadb_trace_")
print(f"Temp dir: {temp_dir}")

try:
    db = proximadb.ProximaDB(data_dirs=temp_dir)
    graph_id = "trace"
    db.create_graph(graph_id)

    # Create nodes first
    print("\n=== PHASE 1: Node Creation ===")
    start = time.perf_counter()
    nodes = [
        proximadb.GraphNode(f"n{i}", labels=["P"], properties={"v": str(i)})
        for i in range(1000)
    ]
    prep_time = (time.perf_counter() - start) * 1000
    print(f"  Python node prep: {prep_time:.1f}ms")

    start = time.perf_counter()
    db.create_nodes(graph_id, nodes)
    rust_node_time = (time.perf_counter() - start) * 1000
    print(
        f"  Rust create_nodes: {rust_node_time:.1f}ms ({1000*1000/rust_node_time:.0f} ops/sec)"
    )

    # Create edges in batches to understand scaling
    print("\n=== PHASE 2: Edge Creation (Batch Analysis) ===")

    edge_counter = 0
    batch_sizes = [100, 500, 1000, 2000, 5000]

    for batch_size in batch_sizes:
        # Create NEW graph each time to avoid duplicate edges
        new_graph_id = f"trace_{batch_size}"
        db.create_graph(new_graph_id)

        # Create nodes for this graph
        nodes = [
            proximadb.GraphNode(f"n{i}", labels=["P"], properties={"v": str(i)})
            for i in range(1000)
        ]
        db.create_nodes(new_graph_id, nodes)

        # Prepare unique edges for this batch
        edges = []
        for i in range(batch_size):
            edges.append(
                proximadb.GraphEdge(
                    f"n{i % 1000}",
                    f"n{(i + 1) % 1000}",
                    f"LINK_{i}",  # Unique edge type to avoid duplicates
                    weight=1.0,
                )
            )

        start = time.perf_counter()
        try:
            db.create_edges(new_graph_id, edges)
            elapsed = (time.perf_counter() - start) * 1000
            ops_per_sec = batch_size * 1000 / elapsed
            print(
                f"  Batch {batch_size:>5}: {elapsed:>8.1f}ms ({ops_per_sec:>8.0f} ops/sec)"
            )
        except Exception as e:
            elapsed = (time.perf_counter() - start) * 1000
            print(f"  Batch {batch_size:>5}: FAILED after {elapsed:.1f}ms - {e}")

    # Test with the benchmark style (5000 edges at once)
    print("\n=== PHASE 3: Benchmark-style Test (1000 nodes, 5000 edges) ===")
    bm_graph_id = "benchmark"
    db.create_graph(bm_graph_id)

    # Create 1000 nodes
    nodes = [
        proximadb.GraphNode(f"n{i}", labels=["P"], properties={"v": str(i)})
        for i in range(1000)
    ]
    db.create_nodes(bm_graph_id, nodes)

    # Create 5000 unique edges (5 per node)
    edges = []
    for i in range(1000):
        for offset in range(1, 6):
            edges.append(
                proximadb.GraphEdge(
                    f"n{i}", f"n{(i + offset) % 1000}", "LINK", weight=1.0
                )
            )

    print(f"  Prepared {len(edges)} edges")

    start = time.perf_counter()
    db.create_edges(bm_graph_id, edges)
    elapsed = (time.perf_counter() - start) * 1000
    ops_per_sec = len(edges) * 1000 / elapsed
    print(f"  create_edges: {elapsed:.1f}ms ({ops_per_sec:.0f} ops/sec)")

finally:
    shutil.rmtree(temp_dir, ignore_errors=True)

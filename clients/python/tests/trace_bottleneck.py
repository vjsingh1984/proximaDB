#!/usr/bin/env python3
"""Trace where time is spent in edge insertion"""

import shutil
import tempfile
import time

import proximadb

temp_dir = tempfile.mkdtemp(prefix="proximadb_trace_")
try:
    db = proximadb.ProximaDB(data_dirs=temp_dir)
    graph_id = "trace"
    db.create_graph(graph_id)

    # Create nodes
    print("Creating 1000 nodes...")
    start = time.perf_counter()
    nodes = [
        proximadb.GraphNode(f"n{i}", labels=["P"], properties={"v": str(i)})
        for i in range(1000)
    ]
    db.create_nodes(graph_id, nodes)
    node_time = (time.perf_counter() - start) * 1000
    print(f"  Nodes: {node_time:.1f}ms\n")

    # Create edges (measure Python-side time)
    print("Creating 5000 edges...")
    print("  Step 1: Creating Python edge objects...")
    start = time.perf_counter()
    edges = []
    count = 0
    for i in range(1000):
        if count >= 5000:
            break
        for offset in range(1, 6):
            if count >= 5000:
                break
            edges.append(
                proximadb.GraphEdge(f"n{i}", f"n{(i+offset)%1000}", "L", weight=1.0)
            )
            count += 1
    prep_time = (time.perf_counter() - start) * 1000
    print(f"    {prep_time:.1f}ms\n")

    print("  Step 2: Calling db.create_edges() (Rust side)...")
    start = time.perf_counter()
    db.create_edges(graph_id, edges)
    rust_time = (time.perf_counter() - start) * 1000
    print(f"    {rust_time:.1f}ms ({5000*1000/rust_time:.0f} ops/sec)\n")

    # Try a query to see if lazy rebuild happens
    print("  Step 3: Query to trigger lazy rebuild...")
    start = time.perf_counter()
    result = db.query_neighbors(graph_id, "n0", max_hops=1)
    query_time = (time.perf_counter() - start) * 1000
    print(f"    {query_time:.1f}ms (found {len(result)} neighbors)\n")

    print(f"Total: {node_time + prep_time + rust_time + query_time:.1f}ms")
    print(f"\nBreakdown:")
    print(f"  Python edge prep:  {prep_time:>8.1f}ms")
    print(f"  Rust edge insert:  {rust_time:>8.1f}ms  <- Main bottleneck")
    print(f"  First query:       {query_time:>8.1f}ms")

finally:
    shutil.rmtree(temp_dir, ignore_errors=True)

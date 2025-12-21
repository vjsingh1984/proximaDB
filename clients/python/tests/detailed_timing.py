#!/usr/bin/env python3
"""
Detailed timing analysis to find the real bottleneck
"""

import proximadb
import tempfile
import shutil
import time
import os

# Enable detailed logging
os.environ['RUST_LOG'] = 'info,proximadb::graph=debug'

temp_dir = tempfile.mkdtemp(prefix="proximadb_timing_")

try:
    db = proximadb.ProximaDB(data_dirs=temp_dir)
    graph_id = "timing_test"
    db.create_graph(graph_id)

    # Create nodes
    print("Creating 1000 nodes...")
    nodes = [proximadb.GraphNode(f"n{i}", labels=["P"], properties={"v": str(i)}) for i in range(1000)]

    start = time.perf_counter()
    db.create_nodes(graph_id, nodes)
    node_time = (time.perf_counter() - start) * 1000
    print(f"Nodes: {node_time:.1f}ms ({1000*1000/node_time:.0f} ops/sec)")
    print()

    # Create edges with timing breakdown
    print("Creating 5000 edges...")

    # Step 1: Create edge objects
    start = time.perf_counter()
    edges = []
    count = 0
    for i in range(1000):
        if count >= 5000:
            break
        for offset in range(1, 6):
            if count >= 5000:
                break
            edges.append(proximadb.GraphEdge(f"n{i}", f"n{(i+offset)%1000}", "L", weight=1.0))
            count += 1
    prep_time = (time.perf_counter() - start) * 1000
    print(f"  Edge object creation: {prep_time:.1f}ms")

    # Step 2: Insert edges (this is where the time is spent)
    start = time.perf_counter()
    db.create_edges(graph_id, edges)
    insert_time = (time.perf_counter() - start) * 1000
    print(f"  Edge insertion: {insert_time:.1f}ms ({5000*1000/insert_time:.0f} ops/sec)")
    print()

    total = node_time + prep_time + insert_time
    print(f"Total: {total:.1f}ms")
    print()
    print("Breakdown:")
    print(f"  Node creation:        {node_time:>8.1f}ms ({node_time/total*100:>5.1f}%)")
    print(f"  Edge object prep:     {prep_time:>8.1f}ms ({prep_time/total*100:>5.1f}%)")
    print(f"  Edge insertion:       {insert_time:>8.1f}ms ({insert_time/total*100:>5.1f}%)")
    print()
    print("Look for debug logs above to see time spent in:")
    print("  - WAL writes")
    print("  - Validation")
    print("  - CSR index building")
    print("  - Memory operations")

finally:
    shutil.rmtree(temp_dir, ignore_errors=True)

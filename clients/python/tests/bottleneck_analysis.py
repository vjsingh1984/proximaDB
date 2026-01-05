#!/usr/bin/env python3
"""
Bottleneck Analysis: Validation vs Persistence

Tests all 4 combinations to identify the real bottleneck:
1. parallel=true,  wal=true  - Current implementation
2. parallel=true,  wal=false - No persistence cost (shows pure validation time)
3. parallel=false, wal=true  - No parallel benefit (shows sequential cost)
4. parallel=false, wal=false - Baseline (minimum time)

This will reveal:
- How much does WAL cost?
- How much does parallel validation help?
- What's the real bottleneck?
"""

import os
import shutil
import subprocess
import tempfile
import time

import proximadb


def run_test(parallel: bool, wal: bool, nodes: int, edges: int):
    """Run test with specific configuration"""

    # Set environment variables
    env = os.environ.copy()
    if not wal:
        env["PROXIMADB_DISABLE_WAL"] = "1"
    else:
        env.pop("PROXIMADB_DISABLE_WAL", None)

    if not parallel:
        env["PROXIMADB_SEQUENTIAL_VALIDATION"] = "1"
    else:
        env.pop("PROXIMADB_SEQUENTIAL_VALIDATION", None)

    # Create test script
    test_script = f"""
import proximadb
import tempfile
import shutil
import time

temp_dir = tempfile.mkdtemp(prefix="proximadb_test_")
try:
    db = proximadb.ProximaDB(data_dirs=temp_dir)
    graph_id = "test"
    db.create_graph(graph_id)

    # Create nodes
    start = time.perf_counter()
    nodes = [proximadb.GraphNode(f"n{{i}}", labels=["P"], properties={{"v": str(i)}}) for i in range({nodes})]
    db.create_nodes(graph_id, nodes)
    node_time = time.perf_counter() - start

    # Create edges
    start = time.perf_counter()
    edges = []
    count = 0
    for i in range({nodes}):
        if count >= {edges}:
            break
        for offset in range(1, 6):
            if count >= {edges}:
                break
            edges.append(proximadb.GraphEdge(f"n{{i}}", f"n{{(i+offset)%{nodes}}}", "L", weight=1.0))
            count += 1
    db.create_edges(graph_id, edges)
    edge_time = time.perf_counter() - start

    print(f"{{node_time*1000:.1f}},{{edge_time*1000:.1f}}")
finally:
    shutil.rmtree(temp_dir, ignore_errors=True)
"""

    # Run in subprocess with env vars
    result = subprocess.run(
        ["python3", "-c", test_script], env=env, capture_output=True, text=True
    )

    if result.returncode != 0:
        print(f"    Error: {result.stderr}")
        return None, None

    try:
        node_time, edge_time = map(float, result.stdout.strip().split(","))
        return node_time, edge_time
    except:
        print(f"    Parse error: {result.stdout}")
        return None, None


def main():
    print("=" * 80)
    print("BOTTLENECK ANALYSIS: Validation vs Persistence")
    print("=" * 80)
    print()

    # Test configuration
    nodes = 1000
    edges = 5000

    print(f"Graph: {nodes} nodes, {edges} edges")
    print()

    configurations = [
        ("parallel=true,  wal=true ", True, True),  # Current (baseline)
        ("parallel=true,  wal=false", True, False),  # No WAL cost
        ("parallel=false, wal=true ", False, True),  # No parallel benefit
        ("parallel=false, wal=false", False, False),  # Minimum time
    ]

    results = []

    for name, parallel, wal in configurations:
        print(f"Testing {name}...", flush=True)
        node_time, edge_time = run_test(parallel, wal, nodes, edges)

        if node_time is None:
            print(f"  FAILED")
            results.append((name, None, None, None, None))
        else:
            total = node_time + edge_time
            edge_ops = edges * 1000 / edge_time
            total_ops = (nodes + edges) * 1000 / total

            print(f"  Nodes: {node_time:.1f}ms")
            print(f"  Edges: {edge_time:.1f}ms")
            print(f"  Total: {total:.1f}ms")
            print(f"  Edge throughput: {edge_ops:.0f} ops/sec")
            print()

            results.append((name, node_time, edge_time, total, edge_ops))

    # Analysis
    print("=" * 80)
    print("RESULTS SUMMARY")
    print("=" * 80)
    print()

    print(
        f"{'Configuration':<25} {'Nodes':>10} {'Edges':>10} {'Total':>10} {'Edge ops/sec':>15}"
    )
    print("-" * 80)

    baseline = None
    for name, node_time, edge_time, total, edge_ops in results:
        if node_time is None:
            print(f"{name:<25} {'FAILED':>10}")
        else:
            print(
                f"{name:<25} {node_time:>9.1f}ms {edge_time:>9.1f}ms {total:>9.1f}ms {edge_ops:>15.0f}"
            )
            if "parallel=true,  wal=true" in name:
                baseline = (node_time, edge_time, total)

    print()
    print("=" * 80)
    print("BOTTLENECK ANALYSIS")
    print("=" * 80)
    print()

    if len([r for r in results if r[1] is not None]) == 4:
        # Extract times
        parallel_wal = results[0]  # parallel=true,  wal=true
        parallel_no_wal = results[1]  # parallel=true,  wal=false
        seq_wal = results[2]  # parallel=false, wal=true
        seq_no_wal = results[3]  # parallel=false, wal=false

        # Calculate costs
        wal_cost_parallel = parallel_wal[2] - parallel_no_wal[2]
        wal_cost_seq = seq_wal[2] - seq_no_wal[2]

        parallel_benefit_wal = seq_wal[2] - parallel_wal[2]
        parallel_benefit_no_wal = seq_no_wal[2] - parallel_no_wal[2]

        print(
            f"WAL Cost (with parallel validation):    {wal_cost_parallel:>8.1f}ms ({wal_cost_parallel/parallel_wal[2]*100:>5.1f}%)"
        )
        print(
            f"WAL Cost (with sequential validation):  {wal_cost_seq:>8.1f}ms ({wal_cost_seq/seq_wal[2]*100:>5.1f}%)"
        )
        print()
        print(
            f"Parallel Benefit (with WAL):            {parallel_benefit_wal:>8.1f}ms ({parallel_benefit_wal/seq_wal[2]*100:>5.1f}% faster)"
        )
        print(
            f"Parallel Benefit (without WAL):         {parallel_benefit_no_wal:>8.1f}ms ({parallel_benefit_no_wal/seq_no_wal[2]*100:>5.1f}% faster)"
        )
        print()

        # Identify bottleneck
        if wal_cost_parallel > parallel_benefit_wal:
            print("🎯 BOTTLENECK IDENTIFIED: WAL writes")
            print(
                f"   WAL cost ({wal_cost_parallel:.1f}ms) > Parallel benefit ({parallel_benefit_wal:.1f}ms)"
            )
        else:
            print("🎯 BOTTLENECK IDENTIFIED: Validation")
            print(
                f"   Parallel benefit ({parallel_benefit_wal:.1f}ms) > WAL cost ({wal_cost_parallel:.1f}ms)"
            )

        print()
        print(
            f"Best case (parallel, no WAL): {parallel_no_wal[2]:.1f}ms ({(nodes+edges)*1000/parallel_no_wal[2]:.0f} ops/sec)"
        )
        print(
            f"Worst case (seq, with WAL):   {seq_wal[2]:.1f}ms ({(nodes+edges)*1000/seq_wal[2]:.0f} ops/sec)"
        )
        print(
            f"Current (parallel, with WAL): {parallel_wal[2]:.1f}ms ({(nodes+edges)*1000/parallel_wal[2]:.0f} ops/sec)"
        )
        print()
        print(
            f"Speedup from removing WAL:        {parallel_wal[2]/parallel_no_wal[2]:.2f}x"
        )
        print(f"Speedup from parallel validation: {seq_wal[2]/parallel_wal[2]:.2f}x")


if __name__ == "__main__":
    main()

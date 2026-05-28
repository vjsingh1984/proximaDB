#!/usr/bin/env python3
"""
All Databases Benchmark: ProximaDB vs Neo4j vs TigerGraph vs NetworkX vs igraph
Focus on core CRUD operations for apple-to-apples comparison
"""

import json
import random
import shutil
import tempfile
import time

import numpy as np

# Database imports
try:
    import proximadb

    PROXIMADB_AVAILABLE = True
    print(f"✓ ProximaDB v{proximadb.__version__}")
except ImportError:
    PROXIMADB_AVAILABLE = False
    print("✗ ProximaDB not available")

try:
    import networkx as nx

    NETWORKX_AVAILABLE = True
    print(f"✓ NetworkX v{nx.__version__}")
except ImportError:
    NETWORKX_AVAILABLE = False
    print("✗ NetworkX not available")

try:
    import igraph as ig

    IGRAPH_AVAILABLE = True
    print(f"✓ igraph v{ig.__version__}")
except ImportError:
    IGRAPH_AVAILABLE = False
    print("✗ igraph not available")

try:
    from neo4j import GraphDatabase

    NEO4J_AVAILABLE = True
    print("✓ Neo4j driver available")
except ImportError:
    NEO4J_AVAILABLE = False
    print("✗ Neo4j driver not available")

# Small graph only for Neo4j/TigerGraph to avoid timeouts
NUM_NODES = 1000
NUM_EDGES = 5000

print(f"\nBenchmark: {NUM_NODES:,} nodes, {NUM_EDGES:,} edges\n")

# Generate test data
random.seed(42)
np.random.seed(42)

nodes = []
for i in range(NUM_NODES):
    nodes.append(
        {
            "id": f"node_{i}",
            "name": f"Entity_{i}",
            "category": random.choice(["A", "B", "C"]),
            "score": random.randint(1, 100),
        }
    )

edges = []
edge_set = set()
for i in range(NUM_EDGES):
    from_idx = random.randint(0, NUM_NODES - 1)
    to_idx = (from_idx + random.randint(1, 100)) % NUM_NODES

    if (from_idx, to_idx) in edge_set or from_idx == to_idx:
        continue
    edge_set.add((from_idx, to_idx))

    edges.append(
        {
            "from": f"node_{from_idx}",
            "to": f"node_{to_idx}",
            "type": random.choice(["KNOWS", "REFERENCES", "CALLS"]),
            "weight": random.random(),
        }
    )

sample_nodes = [f"node_{random.randint(0, NUM_NODES-1)}" for _ in range(100)]

results = {}

# ============================================================================
# ProximaDB
# ============================================================================
if PROXIMADB_AVAILABLE:
    print("ProximaDB:")
    print("-" * 40)
    temp_dir = tempfile.mkdtemp(prefix="proximadb_bench_")
    db = proximadb.ProximaDB(data_dirs=temp_dir)
    graph_id = "benchmark_graph"
    db.create_graph(graph_id)

    # Bulk insert
    graph_nodes = [
        proximadb.GraphNode(
            n["id"],
            labels=["Entity"],
            properties={k: str(v) for k, v in n.items() if k != "id"},
        )
        for n in nodes
    ]
    graph_edges = [
        proximadb.GraphEdge(e["from"], e["to"], e["type"], weight=e["weight"])
        for e in edges
    ]

    start = time.perf_counter()
    db.create_nodes(graph_id, graph_nodes)
    db.create_edges(graph_id, graph_edges)
    bulk_time = (time.perf_counter() - start) * 1000
    print(
        f"  Bulk insert: {bulk_time:.1f}ms ({(NUM_NODES + NUM_EDGES) / (bulk_time / 1000):.0f} ops/sec)"
    )

    # Node lookup
    start = time.perf_counter()
    for node_id in sample_nodes:
        _ = db.get_node(graph_id, node_id)
    lookup_time = (time.perf_counter() - start) * 1000
    print(
        f"  Node lookup: {lookup_time:.1f}ms ({len(sample_nodes) / (lookup_time / 1000):.0f} ops/sec)"
    )

    # Neighbor query
    start = time.perf_counter()
    for node_id in sample_nodes[:50]:
        _ = db.get_outgoing_edges(graph_id, node_id)
    neighbor_time = (time.perf_counter() - start) * 1000
    print(
        f"  Neighbor query: {neighbor_time:.1f}ms ({50 / (neighbor_time / 1000):.0f} ops/sec)"
    )

    results["ProximaDB"] = {
        "bulk_insert_ms": bulk_time,
        "node_lookup_ms": lookup_time,
        "neighbor_query_ms": neighbor_time,
    }

    shutil.rmtree(temp_dir)
    print()

# ============================================================================
# NetworkX
# ============================================================================
if NETWORKX_AVAILABLE:
    print("NetworkX:")
    print("-" * 40)
    graph = nx.DiGraph()

    # Bulk insert
    start = time.perf_counter()
    for node in nodes:
        graph.add_node(node["id"], **{k: v for k, v in node.items() if k != "id"})
    for edge in edges:
        graph.add_edge(
            edge["from"], edge["to"], edge_type=edge["type"], weight=edge["weight"]
        )
    bulk_time = (time.perf_counter() - start) * 1000
    print(
        f"  Bulk insert: {bulk_time:.1f}ms ({(NUM_NODES + NUM_EDGES) / (bulk_time / 1000):.0f} ops/sec)"
    )

    # Node lookup
    start = time.perf_counter()
    for node_id in sample_nodes:
        _ = graph.nodes[node_id] if node_id in graph else None
    lookup_time = (time.perf_counter() - start) * 1000
    print(
        f"  Node lookup: {lookup_time:.1f}ms ({len(sample_nodes) / (lookup_time / 1000):.0f} ops/sec)"
    )

    # Neighbor query
    start = time.perf_counter()
    for node_id in sample_nodes[:50]:
        _ = list(graph.successors(node_id))
    neighbor_time = (time.perf_counter() - start) * 1000
    print(
        f"  Neighbor query: {neighbor_time:.1f}ms ({50 / (neighbor_time / 1000):.0f} ops/sec)"
    )

    results["NetworkX"] = {
        "bulk_insert_ms": bulk_time,
        "node_lookup_ms": lookup_time,
        "neighbor_query_ms": neighbor_time,
    }
    print()

# ============================================================================
# igraph
# ============================================================================
if IGRAPH_AVAILABLE:
    print("igraph:")
    print("-" * 40)
    graph = ig.Graph(directed=True)
    node_map = {}

    # Bulk insert
    start = time.perf_counter()
    graph.add_vertices(len(nodes))
    for i, node in enumerate(nodes):
        node_map[node["id"]] = i
        graph.vs[i]["name"] = node["id"]
        for k, v in node.items():
            if k != "id":
                graph.vs[i][k] = v

    edge_list = [(node_map[e["from"]], node_map[e["to"]]) for e in edges]
    graph.add_edges(edge_list)
    bulk_time = (time.perf_counter() - start) * 1000
    print(
        f"  Bulk insert: {bulk_time:.1f}ms ({(NUM_NODES + NUM_EDGES) / (bulk_time / 1000):.0f} ops/sec)"
    )

    # Node lookup
    start = time.perf_counter()
    for node_id in sample_nodes:
        idx = node_map.get(node_id)
        if idx is not None:
            _ = graph.vs[idx]
    lookup_time = (time.perf_counter() - start) * 1000
    print(
        f"  Node lookup: {lookup_time:.1f}ms ({len(sample_nodes) / (lookup_time / 1000):.0f} ops/sec)"
    )

    # Neighbor query
    start = time.perf_counter()
    for node_id in sample_nodes[:50]:
        idx = node_map.get(node_id)
        if idx is not None:
            _ = graph.neighbors(idx, mode="out")
    neighbor_time = (time.perf_counter() - start) * 1000
    print(
        f"  Neighbor query: {neighbor_time:.1f}ms ({50 / (neighbor_time / 1000):.0f} ops/sec)"
    )

    results["igraph"] = {
        "bulk_insert_ms": bulk_time,
        "node_lookup_ms": lookup_time,
        "neighbor_query_ms": neighbor_time,
    }
    print()

# ============================================================================
# Neo4j
# ============================================================================
if NEO4J_AVAILABLE:
    print("Neo4j (Docker):")
    print("-" * 40)
    try:
        driver = GraphDatabase.driver(
            "bolt://localhost:7687", auth=("neo4j", "benchmark")
        )

        # Clear database
        with driver.session() as session:
            session.run("MATCH (n) DETACH DELETE n")

        # Bulk insert
        start = time.perf_counter()
        with driver.session() as session:
            # Insert nodes
            for node in nodes:
                session.run(
                    "CREATE (n:Entity {id: $id, name: $name, category: $category, score: $score})",
                    id=node["id"],
                    name=node["name"],
                    category=node["category"],
                    score=node["score"],
                )

            # Insert edges
            for edge in edges:
                session.run(
                    f"MATCH (a:Entity {{id: $from}}), (b:Entity {{id: $to}}) "
                    f"CREATE (a)-[:{edge['type']} {{weight: $weight}}]->(b)",
                    **{
                        "from": edge["from"],
                        "to": edge["to"],
                        "weight": edge["weight"],
                    },
                )

        bulk_time = (time.perf_counter() - start) * 1000
        print(
            f"  Bulk insert: {bulk_time:.1f}ms ({(NUM_NODES + NUM_EDGES) / (bulk_time / 1000):.0f} ops/sec)"
        )

        # Node lookup
        start = time.perf_counter()
        with driver.session() as session:
            for node_id in sample_nodes:
                _ = session.run(
                    "MATCH (n:Entity {id: $id}) RETURN n", id=node_id
                ).single()
        lookup_time = (time.perf_counter() - start) * 1000
        print(
            f"  Node lookup: {lookup_time:.1f}ms ({len(sample_nodes) / (lookup_time / 1000):.0f} ops/sec)"
        )

        # Neighbor query
        start = time.perf_counter()
        with driver.session() as session:
            for node_id in sample_nodes[:50]:
                _ = list(
                    session.run(
                        "MATCH (n:Entity {id: $id})-[r]->(m) RETURN m", id=node_id
                    )
                )
        neighbor_time = (time.perf_counter() - start) * 1000
        print(
            f"  Neighbor query: {neighbor_time:.1f}ms ({50 / (neighbor_time / 1000):.0f} ops/sec)"
        )

        results["Neo4j"] = {
            "bulk_insert_ms": bulk_time,
            "node_lookup_ms": lookup_time,
            "neighbor_query_ms": neighbor_time,
        }

        driver.close()
        print()
    except Exception as e:
        print(f"  ✗ Error: {e}")
        print()

# ============================================================================
# Summary Table
# ============================================================================
print("\n" + "=" * 80)
print("SUMMARY")
print("=" * 80 + "\n")

if results:
    # Bulk Insert
    print("BULK INSERT (1,000 nodes + 5,000 edges)")
    print("-" * 80)
    print(
        f"{'Database':<15} {'Time (ms)':>12} {'Throughput (ops/sec)':>25} {'vs ProximaDB':>15}"
    )
    print("-" * 80)

    proximadb_bulk = results.get("ProximaDB", {}).get("bulk_insert_ms", float("inf"))
    sorted_bulk = sorted(results.items(), key=lambda x: x[1]["bulk_insert_ms"])

    for db, data in sorted_bulk:
        time_ms = data["bulk_insert_ms"]
        throughput = (NUM_NODES + NUM_EDGES) / (time_ms / 1000)
        speedup = proximadb_bulk / time_ms if proximadb_bulk != float("inf") else 1.0
        print(f"{db:<15} {time_ms:>12.1f} {throughput:>25,.0f} {speedup:>14.2f}x")

    print()

    # Node Lookup
    print("NODE LOOKUP (100 lookups)")
    print("-" * 80)
    print(
        f"{'Database':<15} {'Time (ms)':>12} {'Throughput (ops/sec)':>25} {'vs ProximaDB':>15}"
    )
    print("-" * 80)

    proximadb_lookup = results.get("ProximaDB", {}).get("node_lookup_ms", float("inf"))
    sorted_lookup = sorted(results.items(), key=lambda x: x[1]["node_lookup_ms"])

    for db, data in sorted_lookup:
        time_ms = data["node_lookup_ms"]
        throughput = 100 / (time_ms / 1000)
        speedup = (
            proximadb_lookup / time_ms if proximadb_lookup != float("inf") else 1.0
        )
        print(f"{db:<15} {time_ms:>12.1f} {throughput:>25,.0f} {speedup:>14.2f}x")

    print()

    # Neighbor Query
    print("NEIGHBOR QUERY (50 queries)")
    print("-" * 80)
    print(
        f"{'Database':<15} {'Time (ms)':>12} {'Throughput (ops/sec)':>25} {'vs ProximaDB':>15}"
    )
    print("-" * 80)

    proximadb_neighbor = results.get("ProximaDB", {}).get(
        "neighbor_query_ms", float("inf")
    )
    sorted_neighbor = sorted(results.items(), key=lambda x: x[1]["neighbor_query_ms"])

    for db, data in sorted_neighbor:
        time_ms = data["neighbor_query_ms"]
        throughput = 50 / (time_ms / 1000)
        speedup = (
            proximadb_neighbor / time_ms if proximadb_neighbor != float("inf") else 1.0
        )
        print(f"{db:<15} {time_ms:>12.1f} {throughput:>25,.0f} {speedup:>14.2f}x")

    print("\n" + "=" * 80)

    # Save results
    with open("all_databases_results.json", "w") as f:
        json.dump(results, f, indent=2)
    print("✓ Results saved to: all_databases_results.json\n")

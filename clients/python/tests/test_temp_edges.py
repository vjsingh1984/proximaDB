import proximadb
import tempfile
import shutil
import time

temp_dir = tempfile.mkdtemp(prefix="proximadb_test_")
try:
    db = proximadb.ProximaDB(data_dirs=temp_dir)
    graph_id = "test"
    db.create_graph(graph_id)

    # Create 100 nodes
    nodes = [proximadb.GraphNode(f"n{i}", labels=["P"], properties={"v": str(i)}) for i in range(100)]
    db.create_nodes(graph_id, nodes)
    print(f"Created {len(nodes)} nodes")

    # Create edges in small batches to see behavior
    for batch_num in range(10):
        start = time.perf_counter()
        edges = []
        for i in range(batch_num * 50, (batch_num + 1) * 50):
            edges.append(proximadb.GraphEdge(f"n{i % 100}", f"n{(i+1) % 100}", "L", weight=1.0))
        
        db.create_edges(graph_id, edges)
        elapsed = (time.perf_counter() - start) * 1000
        print(f"Batch {batch_num+1}: {len(edges)} edges in {elapsed:.1f}ms ({len(edges)*1000/elapsed:.0f} ops/sec)")

finally:
    shutil.rmtree(temp_dir, ignore_errors=True)

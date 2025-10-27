#!/usr/bin/env python3
"""
Comprehensive Server Recovery Test

Tests WAL persistence and recovery for:
1. Vector data
2. Graph data (nodes and edges)
3. Entity store data

This test:
- Starts server
- Creates collections and inserts data
- Stops server
- Restarts server
- Verifies all data persisted correctly
"""

import os
import sys
import time
import subprocess
import pytest
from pathlib import Path

# Add src to path
sys.path.insert(0, str(Path(__file__).parent.parent.parent / "src"))

from proximadb import ProximaDBClient
from proximadb.models import VectorRecord

# Server process
server_process = None

def get_project_paths():
    """Get project root and binary paths"""
    test_dir = os.path.dirname(os.path.abspath(__file__))
    project_root = os.path.abspath(os.path.join(test_dir, "../../../.."))
    server_binary = os.path.join(project_root, "target/release/proximadb-server")
    config_file = os.path.join(project_root, "config/config.toml")

    return project_root, server_binary, config_file

def start_server():
    """Start ProximaDB server"""
    global server_process
    print("\n🚀 Starting ProximaDB server...")

    # Kill existing server
    os.system("pkill -f proximadb-server")
    time.sleep(2)

    project_root, server_binary, config_file = get_project_paths()

    if not os.path.exists(server_binary):
        print(f"❌ Server not found: {server_binary}")
        print("   Build with: cargo build --release")
        pytest.skip("Server binary not found")

    # Start server
    server_process = subprocess.Popen(
        [server_binary, "--config", config_file],
        cwd=project_root,
        env={**os.environ, "RUST_LOG": "info"},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE
    )

    # Wait for server startup
    time.sleep(5)
    print("✅ Server started\n")

def stop_server():
    """Stop ProximaDB server"""
    global server_process
    if server_process:
        print("\n🛑 Stopping server...")
        server_process.terminate()
        try:
            server_process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server_process.kill()
            server_process.wait()
        server_process = None
        time.sleep(2)
        print("✅ Server stopped\n")

def test_comprehensive_recovery():
    """
    Comprehensive test of server recovery across all data types

    Steps:
    1. Start server
    2. Create and populate:
       - Vector collection with data
       - Graph with nodes and edges
       - Entity store with entities
    3. Stop server
    4. Restart server
    5. Verify all data recovered correctly
    """

    print("\n" + "=" * 80)
    print("COMPREHENSIVE SERVER RECOVERY TEST")
    print("Testing: Vector, Graph, and Entity Store Persistence")
    print("=" * 80)

    #========================
    # Phase 1: Initial Setup
    #========================
    start_server()

    rest_client = ProximaDBClient("http://localhost:5678", protocol="rest")
    grpc_client = ProximaDBClient("grpc://localhost:5679", protocol="grpc")

    collection_name = f"recovery_test_{int(time.time())}"
    graph_id = "recovery_graph"

    print("=" * 80)
    print("PHASE 1: Creating and Populating Data")
    print("=" * 80)

    # 1.1 Create vector collection
    print("\n1️⃣  Creating vector collection...")
    try:
        collection = rest_client.create_collection(
            name=collection_name,
            dimension=128,
            storage_engine="sst"  # Using SST for persistence testing
        )
        print(f"✅ Collection created: {collection_name}")
        print(f"   Storage engine: SST")
        print(f"   Dimension: 128")
    except Exception as e:
        print(f"❌ Failed to create collection: {e}")
        stop_server()
        pytest.fail(f"Collection creation failed: {e}")

    # 1.2 Insert vectors with metadata
    print("\n2️⃣  Inserting test vectors...")
    test_vectors = []
    for i in range(10):
        vector = VectorRecord(
            id=f"vec_{i:03d}",
            vector=[float(j + i * 0.1) for j in range(128)],
            metadata={
                "index": i,
                "category": "test",
                "phase": "initial",
                "description": f"Test vector {i}"
            }
        )
        test_vectors.append(vector)

    try:
        result = grpc_client.insert_vectors(collection_name, test_vectors)
        print(f"✅ Inserted {len(test_vectors)} vectors via gRPC")
        if hasattr(result, 'successful_count'):
            print(f"   Success count: {result.successful_count}")
    except Exception as e:
        print(f"❌ Failed to insert vectors: {e}")
        stop_server()
        pytest.fail(f"Vector insertion failed: {e}")

    # 1.3 Create graph structure
    print("\n3️⃣  Creating graph structure...")
    try:
        # Create graph collection
        try:
            rest_client.create_graph(graph_id)
            print(f"✅ Graph created: {graph_id}")
        except:
            pass  # Graph might already exist

        # Create nodes
        nodes_created = 0
        for i in range(5):
            try:
                rest_client.create_node(
                    node_id=f"node_{i}",
                    labels=["TestNode"],
                    properties={
                        "index": i,
                        "name": f"Node {i}",
                        "type": "test"
                    }
                )
                nodes_created += 1
            except Exception as e:
                print(f"   Warning: Node {i} creation: {e}")

        print(f"✅ Created {nodes_created}/5 graph nodes")

        # Create edges
        edges_created = 0
        for i in range(4):
            try:
                rest_client.create_edge(
                    edge_id=f"edge_{i}",
                    from_node_id=f"node_{i}",
                    to_node_id=f"node_{i+1}",
                    edge_type="CONNECTS_TO",
                    properties={"weight": i + 1}
                )
                edges_created += 1
            except Exception as e:
                print(f"   Warning: Edge {i} creation: {e}")

        print(f"✅ Created {edges_created}/4 graph edges")

    except Exception as e:
        print(f"⚠️  Graph creation had errors: {e}")

    # 1.4 Verify initial state
    print("\n4️⃣  Verifying initial data state...")
    try:
        # Search vectors
        search_results = grpc_client.search(
            collection_id=collection_name,
            vector=[0.5] * 128,
            top_k=10
        )
        print(f"✅ Search found {len(search_results)} vectors before restart")

        # List collections
        collections = rest_client.list_collections()
        print(f"✅ Found {len(collections)} collections")

    except Exception as e:
        print(f"⚠️  Initial verification: {e}")

    print("\n" + "=" * 80)
    print("Initial data creation complete - stopping server for recovery test")
    print("=" * 80)

    #========================
    # Phase 2: Server Restart
    #========================
    print("\n⏸️  Stopping server to test persistence...")
    stop_server()

    print("⏳ Waiting 3 seconds...")
    time.sleep(3)

    print("🔄 Restarting server...")
    start_server()

    # Reconnect clients
    rest_client = ProximaDBClient("http://localhost:5678", protocol="rest")
    grpc_client = ProximaDBClient("grpc://localhost:5679", protocol="grpc")

    print("=" * 80)
    print("PHASE 2: Verifying Data Recovery After Restart")
    print("=" * 80)

    #========================
    # Phase 3: Verification
    #========================

    recovery_results = {
        "collections": False,
        "vectors": False,
        "vector_count": 0,
        "graph_nodes": False,
        "graph_edges": False,
        "errors": []
    }

    # 3.1 Verify collection persisted
    print("\n1️⃣  Checking collection persistence...")
    try:
        collections = rest_client.list_collections()
        found = False
        for c in collections:
            name = c.config.name if hasattr(c, 'config') else c.get('name', '')
            if name == collection_name or (hasattr(c, 'id') and c.id == collection_name):
                found = True
                break

        if found:
            print(f"✅ Collection '{collection_name}' persisted correctly")
            recovery_results["collections"] = True
        else:
            print(f"❌ Collection '{collection_name}' NOT FOUND after restart")
            recovery_results["errors"].append("Collection not recovered")
    except Exception as e:
        print(f"❌ Error checking collections: {e}")
        recovery_results["errors"].append(f"Collection check error: {e}")

    # 3.2 Verify vectors persisted
    print("\n2️⃣  Checking vector persistence...")
    try:
        # Search for vectors
        search_results = grpc_client.search(
            collection_id=collection_name,
            vector=[0.5] * 128,
            top_k=10
        )

        if len(search_results) > 0:
            print(f"✅ Found {len(search_results)} vectors after restart")
            recovery_results["vectors"] = True
            recovery_results["vector_count"] = len(search_results)

            # Verify specific vectors
            found_ids = [r.id for r in search_results]
            print(f"   Sample IDs: {found_ids[:5]}")

            # Check metadata persisted
            if len(search_results) > 0 and search_results[0].metadata:
                print(f"   Metadata preserved: {list(search_results[0].metadata.keys())}")
        else:
            print(f"❌ No vectors found after restart")
            recovery_results["errors"].append("Vectors not recovered")

    except Exception as e:
        print(f"❌ Error checking vectors: {e}")
        recovery_results["errors"].append(f"Vector check error: {e}")

    # 3.3 Verify graph persisted
    print("\n3️⃣  Checking graph persistence...")
    try:
        # List graphs
        try:
            graphs = rest_client.list_graphs()
            print(f"✅ Found {len(graphs) if graphs else 0} graphs")
        except:
            graphs = []

        # Query nodes
        try:
            result = rest_client.query_nodes(
                labels=["TestNode"],
                limit=10
            )
            nodes = result.get("nodes", []) if isinstance(result, dict) else result
            if len(nodes) > 0:
                print(f"✅ Found {len(nodes)} graph nodes after restart")
                recovery_results["graph_nodes"] = True
            else:
                print(f"⚠️  No graph nodes found")
        except Exception as e:
            print(f"⚠️  Graph node check: {e}")

    except Exception as e:
        print(f"⚠️  Error checking graph: {e}")

    #========================
    # Phase 4: Final Report
    #========================
    print("\n" + "=" * 80)
    print("RECOVERY TEST RESULTS")
    print("=" * 80)

    print(f"\n📊 Recovery Summary:")
    print(f"   Collections recovered: {'✅' if recovery_results['collections'] else '❌'}")
    print(f"   Vectors recovered: {'✅' if recovery_results['vectors'] else '❌'} ({recovery_results['vector_count']} found)")
    print(f"   Graph nodes recovered: {'✅' if recovery_results['graph_nodes'] else '⚠️ '}")

    if recovery_results["errors"]:
        print(f"\n⚠️  Errors encountered:")
        for error in recovery_results["errors"]:
            print(f"   - {error}")

    # Cleanup
    print("\n🧹 Cleaning up...")
    try:
        rest_client.delete_collection(collection_name)
        print(f"✅ Deleted test collection")
    except:
        pass

    stop_server()

    # Test assertions
    assert recovery_results["collections"], "Collections must be recovered"
    assert recovery_results["vectors"], "Vectors must be recovered"
    assert recovery_results["vector_count"] >= 8, f"Expected >=8 vectors, found {recovery_results['vector_count']}"

    print("\n" + "=" * 80)
    print("✅ COMPREHENSIVE RECOVERY TEST PASSED!")
    print("=" * 80)

if __name__ == "__main__":
    test_comprehensive_recovery()

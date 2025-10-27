#!/usr/bin/env python3
"""
Test gRPC VectorGet functionality after fix
"""

import time
import numpy as np
from proximadb import ProximaDBClient
import subprocess
import os
import signal
import atexit

# Server process
server_process = None

def start_server():
    """Start ProximaDB server for testing"""
    global server_process
    print("🚀 Assuming ProximaDB server is already running...")

    # DISABLED: Don't kill existing server - tests should use already running server
    # os.system("pkill -f proximadb-server")
    # time.sleep(1)

    # DISABLED: Don't start new server - use existing one
    return

    # Start server (DISABLED)
    server_process = subprocess.Popen(
        [
            "/home/vsingh/code/proximaDB/target/release/proximadb-server",
            "--config", "/tmp/proximadb_test/test-config.toml"
        ],
        cwd="/home/vsingh/code/proximaDB",
        env={**os.environ, "RUST_LOG": "debug"},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE
    )
    
    # Wait for server to start
    time.sleep(3)
    print("✅ Server started")

def stop_server():
    """Stop ProximaDB server"""
    global server_process
    # DISABLED: Don't stop server - let it continue running for other tests
    return
    if server_process:
        print("🛑 Stopping server...")
        server_process.terminate()
        server_process.wait()
        server_process = None

# DISABLED: Don't register cleanup - server should keep running
# atexit.register(stop_server)

def test_grpc_vector_get():
    """Test gRPC vector insert and get operations"""
    
    print("\n🔍 Testing gRPC VectorGet Fix")
    print("=" * 60)
    
    # Start server
    start_server()
    
    # Initialize clients
    grpc_client = ProximaDBClient("grpc://localhost:5679", protocol="grpc")
    rest_client = ProximaDBClient("http://localhost:5678", protocol="rest")
    
    # Create collection
    collection_name = f"test_grpc_get_{int(time.time())}"
    print(f"\n📦 Creating collection: {collection_name}")
    
    grpc_client.create_collection(
        name=collection_name,
        dimension=128,
        distance_metric="cosine",
        storage_engine="viper"
    )
    
    # Test vectors
    test_vectors = [
        {
            "id": "vec_001",
            "vector": np.random.rand(128).tolist(),
            "metadata": {"type": "test", "index": 1}
        },
        {
            "id": "vec_002", 
            "vector": np.random.rand(128).tolist(),
            "metadata": {"type": "test", "index": 2}
        }
    ]
    
    # Test 1: gRPC insert -> gRPC get
    print("\n🧪 Test 1: gRPC insert -> gRPC get")
    grpc_client.insert_vectors(collection_name, test_vectors)
    print("✅ Vectors inserted via gRPC")
    
    time.sleep(1)  # Allow time for processing
    
    # Try to get vectors via gRPC
    for vec in test_vectors:
        try:
            result = grpc_client.get_vector(collection_name, vec["id"])
            if result:
                print(f"✅ Found vector {vec['id']} via gRPC")
                print(f"   - Has vector: {result.get('vector') is not None}")
                print(f"   - Has metadata: {result.get('metadata') is not None}")
            else:
                print(f"❌ Vector {vec['id']} NOT FOUND via gRPC")
        except Exception as e:
            print(f"❌ Error getting vector {vec['id']}: {e}")
    
    # Test 2: REST get for comparison
    print("\n🧪 Test 2: Same vectors via REST")
    for vec in test_vectors:
        try:
            result = rest_client.get_vector(collection_name, vec["id"])
            if result:
                print(f"✅ Found vector {vec['id']} via REST")
            else:
                print(f"❌ Vector {vec['id']} NOT FOUND via REST")
        except Exception as e:
            print(f"❌ Error getting vector {vec['id']}: {e}")
    
    # Test 3: Search to verify vectors exist
    print("\n🧪 Test 3: Search to verify vectors exist")
    search_results = grpc_client.search_vectors(
        collection_name,
        query_vector=test_vectors[0]["vector"],
        top_k=5
    )
    print(f"📊 Search found {len(search_results)} results")
    for i, result in enumerate(search_results):
        print(f"   [{i+1}] ID: {result.get('id', 'Unknown')}, Score: {result.get('score', 0):.4f}")
    
    print("\n✅ Test completed")
    

if __name__ == "__main__":
    try:
        test_grpc_vector_get()
    except KeyboardInterrupt:
        print("\n⚠️ Test interrupted")
    except Exception as e:
        print(f"\n❌ Test failed: {e}")
        import traceback
        traceback.print_exc()
    finally:
        stop_server()
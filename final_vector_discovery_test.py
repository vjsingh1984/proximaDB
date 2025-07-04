#!/usr/bin/env python3
"""
Final comprehensive vector discovery test
Traces entire flow from insert to search
"""

import asyncio
import json
import sys
import subprocess
import time
import os
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "clients" / "python" / "src"))

from proximadb.grpc_client import ProximaDBClient

async def final_test():
    print("🔍 FINAL VECTOR DISCOVERY TEST")
    print("="*50)
    
    # Start server with maximum debug logging
    print("🚀 Starting server with comprehensive debug logging...")
    env = os.environ.copy()
    env["RUST_LOG"] = "debug,proximadb::storage::persistence::wal=trace,proximadb::services=trace"
    
    server = subprocess.Popen([
        "cargo", "run", "--release", "--bin", "proximadb-server", "--",
        "--config", "test_config.toml"
    ], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
    
    # Capture server output in background
    server_logs = []
    
    async def capture_logs():
        while True:
            line = server.stdout.readline()
            if line:
                server_logs.append(line.strip())
            else:
                break
    
    # Start log capture
    log_task = asyncio.create_task(capture_logs())
    
    await asyncio.sleep(20)  # Give server time to start
    
    try:
        # Connect
        client = ProximaDBClient("localhost:5679")
        print("✅ Connected to server")
        
        # Create test collection
        coll_name = "final_test"
        dim = 4
        
        # Clean up
        try:
            await client.delete_collection(coll_name)
            await asyncio.sleep(1)
        except:
            pass
        
        print(f"\n📦 Creating collection: {coll_name} (dim={dim})")
        collection = await client.create_collection(
            name=coll_name,
            dimension=dim,
            distance_metric=1,  # COSINE
            storage_engine=1,   # VIPER
        )
        print(f"✅ Collection created: ID={collection.id}")
        
        # Insert test vectors
        print("\n📝 Inserting test vectors...")
        test_vectors = [
            {
                "id": "final_001",
                "vector": [1.0, 0.0, 0.0, 0.0],
                "metadata": {"name": "unit_x", "test": "final"}
            },
            {
                "id": "final_002", 
                "vector": [0.0, 1.0, 0.0, 0.0],
                "metadata": {"name": "unit_y", "test": "final"}
            },
            {
                "id": "final_003",
                "vector": [0.7071, 0.7071, 0.0, 0.0],  # 45 degrees
                "metadata": {"name": "diagonal", "test": "final"}
            }
        ]
        
        insert_result = client.insert_vectors(coll_name, test_vectors)
        print(f"✅ Inserted {insert_result.count} vectors")
        
        # Wait for processing
        print("\n⏳ Waiting 5s for vector processing...")
        await asyncio.sleep(5)
        
        # Check collection metadata
        print("\n📊 Checking collection metadata...")
        collection_info = await client.get_collection(coll_name)
        if collection_info:
            print(f"  • Vector count: {collection_info.vector_count}")
            print(f"  • Status: {collection_info.status}")
        
        # Test different search approaches
        print("\n🔍 Testing vector search...")
        
        # Search 1: Exact match
        print("\n1️⃣ Exact match search for [1,0,0,0]:")
        results1 = client.search_vectors(
            coll_name,
            [[1.0, 0.0, 0.0, 0.0]],
            top_k=3,
            include_metadata=True
        )
        print(f"   Found {len(results1)} results")
        for r in results1:
            print(f"   • {r.id}: score={r.score:.4f}, metadata={r.metadata}")
        
        # Search 2: Similar vector
        print("\n2️⃣ Similar vector search for [0.9,0.1,0,0]:")
        results2 = client.search_vectors(
            coll_name,
            [[0.9, 0.1, 0.0, 0.0]],
            top_k=3,
            include_metadata=True
        )
        print(f"   Found {len(results2)} results")
        for r in results2:
            print(f"   • {r.id}: score={r.score:.4f}")
        
        # Search 3: Different vector
        print("\n3️⃣ Orthogonal vector search for [0,0,1,0]:")
        results3 = client.search_vectors(
            coll_name,
            [[0.0, 0.0, 1.0, 0.0]],
            top_k=3,
            include_metadata=True
        )
        print(f"   Found {len(results3)} results")
        for r in results3:
            print(f"   • {r.id}: score={r.score:.4f}")
        
        # Analyze server logs
        print("\n📋 Analyzing server logs...")
        
        # Filter relevant logs
        insert_logs = [l for l in server_logs if "insert" in l.lower() and "WAL" in l]
        search_logs = [l for l in server_logs if "search" in l.lower() and ("WAL" in l or "UNIFIED" in l)]
        error_logs = [l for l in server_logs if "ERROR" in l or "error[E" in l]
        
        print(f"\nInsert-related logs ({len(insert_logs)}):")
        for log in insert_logs[-5:]:
            print(f"  📝 {log}")
        
        print(f"\nSearch-related logs ({len(search_logs)}):")
        for log in search_logs[-10:]:
            print(f"  🔍 {log}")
        
        if error_logs:
            print(f"\nError logs ({len(error_logs)}):")
            for log in error_logs[-5:]:
                print(f"  ❌ {log}")
        
        # Final verdict
        total_results = len(results1) + len(results2) + len(results3)
        print(f"\n📊 FINAL RESULTS:")
        print(f"  • Total vectors inserted: 3")
        print(f"  • Total search results found: {total_results}")
        print(f"  • Collection vector count: {collection_info.vector_count if collection_info else 'N/A'}")
        
        success = total_results > 0
        
        if success:
            print("\n✅ SUCCESS! Vectors are discoverable")
        else:
            print("\n❌ FAILURE! Vectors not discoverable")
            print("\n💡 Diagnosis:")
            if collection_info and collection_info.vector_count == 0:
                print("  • Collection metadata not updated")
            if len(insert_logs) == 0:
                print("  • No WAL insert logs found")
            if len(search_logs) == 0:
                print("  • No search logs found")
            if len(error_logs) > 0:
                print("  • Errors detected during execution")
        
        return success
        
    finally:
        # Cancel log capture
        log_task.cancel()
        
        # Clean up
        server.terminate()
        await asyncio.sleep(1)
        server.kill()

if __name__ == "__main__":
    success = asyncio.run(final_test())
    sys.exit(0 if success else 1)
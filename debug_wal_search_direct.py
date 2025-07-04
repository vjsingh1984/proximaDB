#!/usr/bin/env python3
"""
Direct WAL search debugging - test if WAL search is actually working
"""

import asyncio
import json
import sys
import subprocess
import time
from pathlib import Path

# Add the Python client to the path
sys.path.insert(0, str(Path(__file__).parent / "clients" / "python" / "src"))

from proximadb.grpc_client import ProximaDBClient

async def debug_wal_search():
    print("🔍 Direct WAL Search Debugging")
    print("="*40)
    
    # Start server with trace logging for WAL
    print("🚀 Starting server with WAL trace logging...")
    import os
    env = os.environ.copy()
    env["RUST_LOG"] = "proximadb::storage::persistence::wal=trace"
    
    server_process = subprocess.Popen([
        "cargo", "run", "--bin", "proximadb-server", "--",
        "--config", "test_config.toml"
    ], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
    
    await asyncio.sleep(10)
    
    try:
        # Connect client
        client = ProximaDBClient("localhost:5679")
        
        # Create a simple collection
        collection_name = "wal_debug_test"
        
        # Delete if exists
        try:
            await client.delete_collection(collection_name)
            await asyncio.sleep(1)
        except:
            pass
        
        # Create collection
        print(f"\n📦 Creating collection: {collection_name}")
        collection = await client.create_collection(
            name=collection_name,
            dimension=4,  # Very small for easy debugging
            distance_metric=1,  # COSINE
            storage_engine=1,   # VIPER
        )
        print(f"✅ Collection created: {collection.id}")
        
        # Insert a single vector
        print("\n📝 Inserting test vector...")
        test_vector = {
            "id": "wal_test_001",
            "vector": [1.0, 0.0, 0.0, 0.0],  # Unit vector
            "metadata": {"test": "wal_direct"}
        }
        
        result = client.insert_vectors(
            collection_id=collection_name,
            vectors=[test_vector],
            upsert=False
        )
        print(f"✅ Inserted {result.count} vector")
        
        # Wait briefly
        await asyncio.sleep(2)
        
        # Search immediately (should find in WAL)
        print("\n🔍 Searching for vector...")
        query_vector = [0.9, 0.1, 0.0, 0.0]  # Similar to inserted
        
        search_start = time.time()
        results = client.search_vectors(
            collection_id=collection_name,
            query_vectors=[query_vector],
            top_k=5,
            include_metadata=True,
            include_vectors=True
        )
        search_time = time.time() - search_start
        
        print(f"📊 Search completed in {search_time*1000:.1f}ms")
        print(f"   • Results found: {len(results)}")
        
        if len(results) > 0:
            print("\n✅ WAL SEARCH WORKING!")
            for i, result in enumerate(results):
                print(f"   {i+1}. ID: {result.id}")
                print(f"      Score: {result.score:.4f}")
                print(f"      Vector: {result.vector}")
                print(f"      Metadata: {result.metadata}")
        else:
            print("\n❌ NO RESULTS FOUND!")
            
            # Try to understand why
            print("\n🔍 Checking server logs for WAL activity...")
            
            # Read server output
            output_lines = []
            while True:
                line = server_process.stdout.readline()
                if not line:
                    break
                output_lines.append(line.strip())
                if len(output_lines) > 1000:
                    break
            
            # Look for WAL-related messages
            wal_lines = [l for l in output_lines if "WAL" in l or "wal" in l or "search" in l]
            print(f"\nFound {len(wal_lines)} WAL-related log lines:")
            for line in wal_lines[-20:]:  # Last 20
                print(f"  • {line}")
        
        return len(results) > 0
        
    finally:
        if server_process:
            server_process.terminate()
            await asyncio.sleep(1)
            server_process.kill()

if __name__ == "__main__":
    success = asyncio.run(debug_wal_search())
    sys.exit(0 if success else 1)
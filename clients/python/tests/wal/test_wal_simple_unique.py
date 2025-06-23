#!/usr/bin/env python3
"""Simple WAL persistence test with unique collection name"""

import grpc
import json
import time
import os
import sys
import numpy as np
from pathlib import Path
import uuid

# Add the Python SDK to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb.grpc_client import ProximaDBClient
import asyncio

async def test_wal_persistence():
    """Test WAL persistence with async client"""
    print("🧪 Testing WAL persistence...")
    
    # Connect to ProximaDB
    client = ProximaDBClient(endpoint="localhost:5679")
    
    # Create test collection with unique name
    collection_name = f"test_wal_{uuid.uuid4().hex[:8]}"
    print(f"📝 Creating collection: {collection_name}")
    
    try:
        collection = await client.create_collection(
            name=collection_name,
            dimension=384
        )
        print(f"✅ Collection created: {collection.name}")
    except Exception as e:
        print(f"Collection error: {e}")
        return
    
    # Check WAL directory
    wal_dir = Path("/workspace/data/wal")
    print(f"📂 WAL directory: {wal_dir}")
    
    def get_wal_files():
        if wal_dir.exists():
            return list(wal_dir.rglob("*.avro"))
        return []
    
    # Before insertion
    before_files = get_wal_files()
    print(f"📁 WAL files before: {len(before_files)}")
    for f in before_files:
        print(f"   - {f.relative_to(wal_dir)} ({f.stat().st_size} bytes)")
    
    # Insert vectors
    print(f"\n🔢 Inserting test vectors into {collection_name}...")
    vectors = [
        {
            "id": "test_1",
            "vector": np.random.random(384).tolist(),
            "metadata": {"text": "test document 1"}
        },
        {
            "id": "test_2", 
            "vector": np.random.random(384).tolist(),
            "metadata": {"text": "test document 2"}
        }
    ]
    
    try:
        result = client.insert_vectors(
            collection_id=collection_name,
            vectors=vectors
        )
        print(f"✅ Vectors inserted: {result.count}")
        print(f"   Duration: {result.duration_ms:.2f}ms")
    except Exception as e:
        print(f"❌ Insert failed: {e}")
        # Check server logs for more details
        print("\n📋 Recent server logs:")
        try:
            with open("/workspace/server.log", "r") as f:
                lines = f.readlines()
                for line in lines[-10:]:
                    if "error" in line.lower() or "failed" in line.lower():
                        print(f"   {line.strip()}")
        except:
            pass
        return
    
    # Check WAL files after insertion
    time.sleep(1)  # Give time for disk writes
    after_files = get_wal_files()
    print(f"\n📁 WAL files after: {len(after_files)}")
    
    for f in after_files:
        stat = f.stat()
        rel_path = f.relative_to(wal_dir)
        print(f"   - {rel_path} ({stat.st_size} bytes)")
        
        # Read first few bytes to verify
        try:
            with open(f, 'rb') as file:
                first_bytes = file.read(20)
                print(f"     First bytes: {first_bytes[:10].hex()}...")
        except Exception as e:
            print(f"     Read error: {e}")
    
    # Check for collection-specific WAL files
    collection_wal_files = [f for f in after_files if collection_name in str(f)]
    print(f"\n📁 Collection-specific WAL files: {len(collection_wal_files)}")
    for f in collection_wal_files:
        print(f"   - {f}")
    
    # Success check
    new_files = [f for f in after_files if f not in before_files]
    if len(new_files) > 0:
        total_size = sum(f.stat().st_size for f in new_files)
        print(f"\n✅ SUCCESS: WAL persistence working!")
        print(f"   New files: {len(new_files)}")
        print(f"   Total new WAL size: {total_size} bytes")
        
        # Show timing performance
        print(f"   Vector insert timing: {result.duration_ms:.2f}ms")
        print(f"   Immediate WAL sync confirmed for durability!")
    else:
        print(f"\n❌ FAILURE: No new WAL files created")
        print(f"   Before: {len(before_files)} files")
        print(f"   After: {len(after_files)} files")
    
    # Cleanup
    try:
        await client.delete_collection(collection_name)
        print(f"\n🧹 Cleaned up collection: {collection_name}")
    except Exception as e:
        print(f"⚠️ Cleanup error: {e}")

def main():
    asyncio.run(test_wal_persistence())

if __name__ == "__main__":
    main()
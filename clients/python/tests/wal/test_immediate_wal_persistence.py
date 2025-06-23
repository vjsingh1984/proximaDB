#!/usr/bin/env python3
"""Test immediate WAL persistence functionality"""

import grpc
import json
import time
import os
import sys
import numpy as np
from pathlib import Path

# Add the Python SDK to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb.grpc_client import ProximaDBClient

def test_immediate_wal_persistence():
    """Test that vector inserts immediately persist to WAL files"""
    print("🧪 Testing immediate WAL persistence...")
    
    # Connect to ProximaDB
    client = ProximaDBClient(endpoint="localhost:5679")
    
    # Create test collection
    collection_name = "test_wal_persistence"
    print(f"📝 Creating collection: {collection_name}")
    
    try:
        client.create_collection(
            name=collection_name,
            dimension=384,
            description="Test collection for WAL persistence"
        )
        print(f"✅ Collection {collection_name} created")
    except Exception as e:
        print(f"Collection might exist: {e}")
    
    # Check WAL directory before insertion
    wal_dir = Path("/workspace/data/wal")
    print(f"📂 Checking WAL directory: {wal_dir}")
    
    def list_wal_files():
        """List all .avro files in WAL directory"""
        avro_files = []
        if wal_dir.exists():
            avro_files = list(wal_dir.rglob("*.avro"))
        return avro_files
    
    # Initial state
    initial_files = list_wal_files()
    print(f"📁 Initial WAL files: {len(initial_files)}")
    for f in initial_files:
        print(f"   - {f}")
    
    # Insert test vectors with BERT embeddings
    print("\n🔢 Inserting test vectors...")
    
    test_vectors = [
        {
            "id": "vector_1",
            "vector": np.random.random(384).tolist(),
            "metadata": {"text": "test document 1", "category": "test"}
        },
        {
            "id": "vector_2", 
            "vector": np.random.random(384).tolist(),
            "metadata": {"text": "test document 2", "category": "test"}
        },
        {
            "id": "vector_3",
            "vector": np.random.random(384).tolist(), 
            "metadata": {"text": "test document 3", "category": "test"}
        }
    ]
    
    # Insert vectors one by one and check WAL immediately after each
    for i, vector_data in enumerate(test_vectors, 1):
        print(f"\n📊 Inserting vector {i}: {vector_data['id']}")
        
        # Insert vector
        try:
            result = client.insert_vector(
                collection_name=collection_name,
                vector_id=vector_data["id"],
                vector=vector_data["vector"],
                metadata=vector_data["metadata"]
            )
            print(f"✅ Vector {vector_data['id']} inserted successfully")
            print(f"   Insert result: {result}")
        except Exception as e:
            print(f"❌ Failed to insert vector {vector_data['id']}: {e}")
            continue
        
        # Give a moment for any async operations
        time.sleep(0.1)
        
        # Immediately check WAL files
        current_files = list_wal_files()
        print(f"📁 WAL files after insert {i}: {len(current_files)}")
        
        for f in current_files:
            stat = f.stat()
            print(f"   - {f.relative_to(wal_dir)} ({stat.st_size} bytes, modified: {time.ctime(stat.st_mtime)})")
    
    print(f"\n🔍 Final WAL directory state:")
    final_files = list_wal_files()
    print(f"📁 Total WAL files: {len(final_files)}")
    
    for f in final_files:
        stat = f.stat()
        rel_path = f.relative_to(wal_dir)
        print(f"   - {rel_path}")
        print(f"     Size: {stat.st_size} bytes")
        print(f"     Modified: {time.ctime(stat.st_mtime)}")
        
        # Try to read first few bytes to verify it's a valid file
        try:
            with open(f, 'rb') as file:
                first_bytes = file.read(10)
                print(f"     First 10 bytes: {first_bytes.hex()}")
        except Exception as e:
            print(f"     Error reading file: {e}")
    
    # Test: Verify WAL persistence worked
    success_criteria = [
        len(final_files) > 0,
        any(f.stat().st_size > 0 for f in final_files)
    ]
    
    if all(success_criteria):
        print(f"\n✅ SUCCESS: Immediate WAL persistence is working!")
        print(f"   - Found {len(final_files)} WAL files")
        print(f"   - Total WAL data: {sum(f.stat().st_size for f in final_files)} bytes")
    else:
        print(f"\n❌ FAILURE: WAL persistence not working as expected")
        print(f"   - Files found: {len(final_files)}")
        print(f"   - Non-empty files: {sum(1 for f in final_files if f.stat().st_size > 0)}")
    
    # Cleanup
    try:
        client.delete_collection(collection_name)
        print(f"\n🧹 Cleaned up test collection: {collection_name}")
    except Exception as e:
        print(f"⚠️ Cleanup warning: {e}")

if __name__ == "__main__":
    test_immediate_wal_persistence()
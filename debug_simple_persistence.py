#!/usr/bin/env python3
"""
Simple persistence test - create collection and immediately check files
"""

import asyncio
import os
import sys
from datetime import datetime

sys.path.insert(0, 'clients/python/src')
from proximadb.grpc_client import ProximaDBClient

async def test_simple_persistence():
    """Test simple persistence"""
    print("🧪 Simple Persistence Test")
    print("=" * 50)
    
    # Connect and create one collection
    client = ProximaDBClient(endpoint='localhost:5679')
    
    timestamp = datetime.now().strftime("%H%M%S")
    collection_name = f"simple_test_{timestamp}"
    
    print(f"1️⃣ Creating collection: {collection_name}")
    result = await client.create_collection(
        name=collection_name,
        dimension=128,
        distance_metric=1,  # COSINE
    )
    print(f"   ✅ Created: {collection_name} (UUID: {result.id})")
    
    # Immediately check what's on disk
    print("\n2️⃣ Checking disk files immediately after creation:")
    
    metadata_dir = "/data/proximadb/1/metadata"
    for subdir in ["incremental", "snapshots", "archive"]:
        path = os.path.join(metadata_dir, subdir)
        if os.path.exists(path):
            files = []
            if subdir == "archive":
                # Check archive subdirectories
                for item in os.listdir(path):
                    item_path = os.path.join(path, item)
                    if os.path.isdir(item_path):
                        for sub_item in ["incremental", "snapshots"]:
                            sub_path = os.path.join(item_path, sub_item)
                            if os.path.exists(sub_path):
                                sub_files = os.listdir(sub_path)
                                for f in sub_files:
                                    files.append(f"{item}/{sub_item}/{f}")
            else:
                files = os.listdir(path)
                
            print(f"   📁 {subdir}: {len(files)} files")
            for f in files[:3]:  # Show first 3
                print(f"      📄 {f}")
            if len(files) > 3:
                print(f"      ... and {len(files) - 3} more")
        else:
            print(f"   📁 {subdir}: directory does not exist")
    
    # List collections to verify it's in memory
    print("\n3️⃣ Listing collections from memory:")
    collections = await client.list_collections()
    print(f"   Found {len(collections)} collections in memory")
    for c in collections:
        print(f"   📋 {c.name} (UUID: {c.id})")

if __name__ == "__main__":
    asyncio.run(test_simple_persistence())
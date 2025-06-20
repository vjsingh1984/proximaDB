#!/usr/bin/env python3
"""
Real-time test to verify filestore backend file creation
"""

import asyncio
import sys
import os
import time
import subprocess

# Add client path
sys.path.insert(0, 'clients/python/src')

from proximadb.grpc_client import ProximaDBClient

async def test_filestore_realtime():
    """Test collection operations with real-time filestore monitoring"""
    
    print("🔍 Real-time Filestore Backend Test")
    print("=" * 50)
    
    # Monitor directory in background
    metadata_path = "./data/metadata"
    
    def check_files():
        files = []
        if os.path.exists(metadata_path):
            for root, dirs, filenames in os.walk(metadata_path):
                for filename in filenames:
                    rel_path = os.path.relpath(os.path.join(root, filename), metadata_path)
                    size = os.path.getsize(os.path.join(root, filename))
                    files.append((rel_path, size))
        return files
    
    print(f"1. Initial filestore state:")
    initial_files = check_files()
    print(f"   📄 Files: {len(initial_files)}")
    for file, size in initial_files:
        print(f"      - {file} ({size} bytes)")
    
    # Connect to server
    print(f"\n2. Connecting to ProximaDB...")
    client = ProximaDBClient("localhost:5679")
    
    # Create collection and monitor files after each operation
    print(f"\n3. Creating collection...")
    collection_name = f"realtime_test_{int(time.time())}"
    
    created = await client.create_collection(
        name=collection_name,
        dimension=128,
        distance_metric=1,  # COSINE
        indexing_algorithm=1  # HNSW
    )
    print(f"   ✅ Collection created: {created.name}")
    print(f"   🆔 UUID: {created.id}")
    
    # Check files immediately after creation
    print(f"\n4. Files after creation:")
    after_create = check_files()
    print(f"   📄 Files: {len(after_create)}")
    for file, size in after_create:
        print(f"      - {file} ({size} bytes)")
    
    # List collections
    print(f"\n5. Listing collections...")
    collections = await client.list_collections()
    print(f"   📊 Total: {len(collections)}")
    
    # Check files after list
    print(f"\n6. Files after list:")
    after_list = check_files()
    print(f"   📄 Files: {len(after_list)}")
    for file, size in after_list:
        print(f"      - {file} ({size} bytes)")
    
    # Get collection
    print(f"\n7. Getting collection...")
    retrieved = await client.get_collection(collection_name)
    if retrieved:
        print(f"   ✅ Retrieved: {retrieved.name}")
    
    # Check files after get
    print(f"\n8. Files after get:")
    after_get = check_files()
    print(f"   📄 Files: {len(after_get)}")
    for file, size in after_get:
        print(f"      - {file} ({size} bytes)")
    
    # Create another collection to see if it generates files
    print(f"\n9. Creating second collection...")
    collection_name2 = f"realtime_test2_{int(time.time())}"
    
    created2 = await client.create_collection(
        name=collection_name2,
        dimension=256,
        distance_metric=2,  # EUCLIDEAN
        indexing_algorithm=2  # IVF
    )
    print(f"   ✅ Second collection created: {created2.name}")
    
    # Check files after second creation
    print(f"\n10. Files after second creation:")
    after_second = check_files()
    print(f"    📄 Files: {len(after_second)}")
    for file, size in after_second:
        print(f"       - {file} ({size} bytes)")
    
    # Delete first collection
    print(f"\n11. Deleting first collection...")
    deleted = await client.delete_collection(collection_name)
    if deleted:
        print(f"    ✅ First collection deleted")
    
    # Check files after deletion
    print(f"\n12. Files after deletion:")
    after_delete = check_files()
    print(f"    📄 Files: {len(after_delete)}")
    for file, size in after_delete:
        print(f"       - {file} ({size} bytes)")
    
    # Clean up - delete second collection
    await client.delete_collection(collection_name2)
    print(f"\n13. Final cleanup completed")
    
    # Final check
    print(f"\n14. Final filestore state:")
    final_files = check_files()
    print(f"    📄 Files: {len(final_files)}")
    for file, size in final_files:
        print(f"       - {file} ({size} bytes)")
    
    print(f"\n" + "=" * 50)
    print(f"✅ Real-time filestore test complete!")
    return True

if __name__ == "__main__":
    success = asyncio.run(test_filestore_realtime())
    sys.exit(0 if success else 1)
#!/usr/bin/env python3
"""
Test to verify filestore backend is generating files correctly
"""

import asyncio
import sys
import os
import time

# Add client path
sys.path.insert(0, 'clients/python/src')

from proximadb.grpc_client import ProximaDBClient

async def test_filestore_files():
    """Test collection operations and verify filestore files"""
    
    print("🔍 Testing Filestore Backend File Generation")
    print("=" * 60)
    
    # Check initial filestore state
    metadata_path = "./data/metadata"
    snapshots_path = f"{metadata_path}/snapshots"
    incremental_path = f"{metadata_path}/incremental" 
    archive_path = f"{metadata_path}/archive"
    
    print(f"1. Checking filestore directories:")
    print(f"   📁 Metadata path: {metadata_path}")
    print(f"   📁 Snapshots path: {snapshots_path}")
    print(f"   📁 Incremental path: {incremental_path}")
    print(f"   📁 Archive path: {archive_path}")
    
    # List initial files
    def list_files_recursive(path):
        files = []
        if os.path.exists(path):
            for root, dirs, filenames in os.walk(path):
                for filename in filenames:
                    rel_path = os.path.relpath(os.path.join(root, filename), path)
                    files.append(rel_path)
        return files
    
    print(f"\n2. Initial filestore state:")
    initial_files = list_files_recursive(metadata_path)
    print(f"   📄 Files before operations: {len(initial_files)}")
    for file in initial_files:
        print(f"      - {file}")
    
    # Connect to server
    print(f"\n3. Connecting to ProximaDB...")
    client = ProximaDBClient("localhost:5679")
    
    # Create a test collection
    print(f"\n4. Creating test collection...")
    collection_name = f"filestore_test_{int(time.time())}"
    
    try:
        created = await client.create_collection(
            name=collection_name,
            dimension=64,
            distance_metric=1,  # COSINE
            indexing_algorithm=1  # HNSW
        )
        print(f"   ✅ Collection created: {created.name}")
        print(f"   🆔 UUID: {created.id}")
        
        # Check files after creation
        print(f"\n5. Checking filestore after creation...")
        after_create_files = list_files_recursive(metadata_path)
        print(f"   📄 Files after creation: {len(after_create_files)}")
        for file in after_create_files:
            print(f"      - {file}")
            
        # Show file sizes and content if small
        for file in after_create_files:
            file_path = os.path.join(metadata_path, file)
            if os.path.exists(file_path):
                size = os.path.getsize(file_path)
                print(f"      📏 {file}: {size} bytes")
                if size < 1024 and file.endswith('.avro'):  # Show small Avro files
                    print(f"         (Binary Avro file)")
        
        # List collections to trigger more operations
        print(f"\n6. Listing collections...")
        collections = await client.list_collections()
        print(f"   📊 Total collections: {len(collections)}")
        
        # Get the collection by name
        print(f"\n7. Getting collection by name...")
        retrieved = await client.get_collection(collection_name)
        if retrieved:
            print(f"   ✅ Retrieved: {retrieved.name}")
        
        # Check files after all operations
        print(f"\n8. Final filestore state...")
        final_files = list_files_recursive(metadata_path)
        print(f"   📄 Files after all operations: {len(final_files)}")
        for file in final_files:
            file_path = os.path.join(metadata_path, file)
            if os.path.exists(file_path):
                size = os.path.getsize(file_path)
                mtime = os.path.getmtime(file_path)
                mtime_str = time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(mtime))
                print(f"      - {file} ({size} bytes, modified: {mtime_str})")
        
        # Delete the test collection
        print(f"\n9. Deleting test collection...")
        deleted = await client.delete_collection(collection_name)
        if deleted:
            print(f"   ✅ Collection deleted successfully")
            
        # Check final filestore state
        print(f"\n10. Filestore state after deletion...")
        delete_files = list_files_recursive(metadata_path)
        print(f"    📄 Files after deletion: {len(delete_files)}")
        for file in delete_files:
            file_path = os.path.join(metadata_path, file)
            if os.path.exists(file_path):
                size = os.path.getsize(file_path)
                mtime = os.path.getmtime(file_path)
                mtime_str = time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(mtime))
                print(f"      - {file} ({size} bytes, modified: {mtime_str})")
        
    except Exception as e:
        print(f"   ❌ Error: {e}")
        return False
    
    print(f"\n" + "=" * 60)
    print(f"✅ Filestore verification complete!")
    print(f"📊 Summary:")
    print(f"   - Initial files: {len(initial_files)}")
    print(f"   - After create: {len(after_create_files)}")
    print(f"   - After operations: {len(final_files)}")
    print(f"   - After deletion: {len(delete_files)}")
    return True

if __name__ == "__main__":
    success = asyncio.run(test_filestore_files())
    sys.exit(0 if success else 1)
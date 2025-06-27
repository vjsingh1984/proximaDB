#!/usr/bin/env python3

"""
Simple test to verify filestore backend file creation using Python gRPC client
"""

import os
import sys
import time
import tempfile
import shutil
import asyncio
from pathlib import Path

# Add Python client to path
sys.path.insert(0, 'clients/python/src')

from proximadb.grpc_client import ProximaDBClient

def count_files_recursive(directory):
    """Count files recursively in a directory"""
    if not os.path.exists(directory):
        return 0
    
    count = 0
    for root, dirs, files in os.walk(directory):
        count += len(files)
    return count

def list_files_recursive(directory):
    """List all files recursively in a directory"""
    files = []
    if not os.path.exists(directory):
        return files
    
    for root, dirs, file_list in os.walk(directory):
        for file in file_list:
            file_path = os.path.join(root, file)
            rel_path = os.path.relpath(file_path, directory)
            size = os.path.getsize(file_path)
            files.append((rel_path, size))
    return files

async def main():
    print("🧪 Testing ProximaDB filestore backend file creation")
    
    # Check if server is running
    try:
        client = ProximaDBClient("localhost:5679")
        await client.health_check()
        print("✅ Server is running and healthy")
    except Exception as e:
        print(f"❌ Server not running or not healthy: {e}")
        print("📝 Please start the server with: cargo run --bin proximadb-server")
        return 1
    
    # Get server's data directory from config
    # Since we can't easily read the server config, we'll use the default location
    data_dir = Path("./data/metadata")
    print(f"📁 Monitoring directory: {data_dir.absolute()}")
    
    if not data_dir.exists():
        print(f"📁 Data directory does not exist: {data_dir}")
        print("📝 This will be created when first collection is added")
    
    # Count initial files
    initial_files = count_files_recursive(str(data_dir))
    print(f"📄 Initial files in {data_dir}: {initial_files}")
    
    try:
        # Test 1: Create a collection
        print("\n1️⃣ Creating test collection...")
        
        result = await client.create_collection(
            name="filestore_test_collection",
            dimension=128,
            distance_metric=1,  # COSINE
            indexing_algorithm=1,  # HNSW
            storage_engine=1  # VIPER
        )
        print(f"   ✅ Collection created: {result}")
        
        # Wait a moment for async operations to complete
        time.sleep(0.1)
        
        # Check files after creation
        after_create_files = count_files_recursive(str(data_dir))
        print(f"📄 Files after creation: {after_create_files} (delta: +{after_create_files - initial_files})")
        
        # List all files
        files = list_files_recursive(str(data_dir))
        if files:
            print("📁 Files created:")
            for file_path, size in files:
                print(f"      - {file_path} ({size} bytes)")
        else:
            print("❌ No files were created!")
        
        # Test 2: List collections to verify in-memory operation
        print("\n2️⃣ Listing collections...")
        collections = await client.list_collections()
        print(f"   📋 Collections in memory: {len(collections)}")
        for col in collections:
            print(f"      - {col.get('name', 'N/A')} (UUID: {col.get('uuid', 'N/A')})")
        
        # Test 3: Create another collection
        print("\n3️⃣ Creating second collection...")
        
        result2 = await client.create_collection(
            name="filestore_test_collection_2",
            dimension=256,
            distance_metric=2,  # EUCLIDEAN
            indexing_algorithm=1,  # HNSW
            storage_engine=1  # VIPER
        )
        print(f"   ✅ Second collection created: {result2}")
        
        # Wait a moment for async operations to complete
        time.sleep(0.1)
        
        # Check files after second creation
        after_second_files = count_files_recursive(str(data_dir))
        print(f"📄 Files after second creation: {after_second_files} (delta: +{after_second_files - after_create_files})")
        
        # List all files again
        files = list_files_recursive(str(data_dir))
        if files:
            print("📁 All files:")
            for file_path, size in files:
                print(f"      - {file_path} ({size} bytes)")
        else:
            print("❌ Still no files were created!")
        
        # Test 4: Delete a collection
        print("\n4️⃣ Deleting first collection...")
        
        delete_result = await client.delete_collection("filestore_test_collection")
        print(f"   ✅ Collection deleted: {delete_result}")
        
        # Wait a moment for async operations to complete
        time.sleep(0.1)
        
        # Check files after deletion
        after_delete_files = count_files_recursive(str(data_dir))
        print(f"📄 Files after deletion: {after_delete_files} (delta: {after_delete_files - after_second_files})")
        
        # List files after deletion
        files = list_files_recursive(str(data_dir))
        if files:
            print("📁 Files after deletion:")
            for file_path, size in files:
                print(f"      - {file_path} ({size} bytes)")
        
        # Verify memory state
        collections = await client.list_collections()
        print(f"   📋 Collections remaining in memory: {len(collections)}")
        
        # Summary
        print("\n🔍 Summary:")
        print(f"   - Memory operations work: {len(collections) >= 0}")
        print(f"   - Files created during test: {after_delete_files > initial_files}")
        print(f"   - Final file count: {after_delete_files}")
        
        if after_delete_files == initial_files:
            print("\n❌ ISSUE CONFIRMED: No files were created on disk!")
            print("   Operations work in memory but files aren't written to filesystem.")
            print("   This confirms the bug in FilestoreMetadataBackend.")
            return 1
        else:
            print("\n✅ Files were created successfully!")
            return 0
            
    except Exception as e:
        print(f"❌ Test failed with error: {e}")
        import traceback
        traceback.print_exc()
        return 1
    
    finally:
        # Clean up - delete any remaining test collections
        try:
            await client.delete_collection("filestore_test_collection")
            await client.delete_collection("filestore_test_collection_2")
        except:
            pass

if __name__ == "__main__":
    exit(asyncio.run(main()))
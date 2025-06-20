#!/usr/bin/env python3
"""
Test filestore persistence across server restarts
"""

import time
import subprocess
import sys
import os

# Add client path
sys.path.insert(0, 'clients/python/src')

from proximadb.grpc_client import ProximaDBClient
import grpc
import asyncio

async def test_collection_persistence():
    """Test that collections persist across server restarts using filestore backend"""
    
    print("🧪 Testing ProximaDB Collection Persistence with Filestore Backend")
    print("=" * 70)
    
    # Start server
    print("1. Starting ProximaDB server...")
    server_process = subprocess.Popen(
        ["cargo", "run", "--bin", "proximadb-server"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=os.getcwd()
    )
    
    # Wait for server to start
    time.sleep(3)
    
    try:
        # Connect to server
        print("2. Connecting to ProximaDB...")
        client = ProximaDBClient("localhost:5679")
        
        # Test 1: Create a collection
        print("3. Creating test collection...")
        import uuid
        collection_name = f"test_filestore_collection_{uuid.uuid4().hex[:8]}"
        
        try:
            # List collections before creation (should be empty)
            collections_before = await client.list_collections()
            print(f"   Collections before creation: {len(collections_before)}")
            
            # Create collection
            try:
                created_collection = await client.create_collection(
                    name=collection_name,
                    dimension=128,
                    distance_metric=1,  # COSINE
                    indexing_algorithm=1  # HNSW
                )
                print(f"   Collection creation successful: {created_collection.name}")
                print(f"   Collection UUID: {created_collection.id}")
                collection_uuid = created_collection.id
            except Exception as e:
                print(f"   Collection creation failed: {e}")
                return False
            
            # List collections after creation
            collections_after = await client.list_collections()
            print(f"   Collections after creation: {len(collections_after)}")
            
            if len(collections_after) != len(collections_before) + 1:
                print("   ❌ Collection count mismatch")
                return False
            
            # Find our collection
            found_collection = None
            for col in collections_after:
                col_name = col.config.name if col.config else col.name
                if col_name == collection_name:
                    found_collection = col
                    break
            
            if not found_collection:
                print("   ❌ Created collection not found in list")
                return False
            
            col_name = found_collection.config.name if found_collection.config else found_collection.name
            print(f"   ✅ Collection created: {col_name}")
            print(f"   ✅ UUID: {found_collection.id}")
            
        except Exception as e:
            print(f"   ❌ Collection creation failed: {e}")
            return False
        
        print("4. Stopping server to test persistence...")
        
    finally:
        # Stop server
        server_process.terminate()
        try:
            server_process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server_process.kill()
            server_process.wait()
    
    # Wait a moment
    time.sleep(1)
    
    # Start server again
    print("5. Restarting ProximaDB server...")
    server_process2 = subprocess.Popen(
        ["cargo", "run", "--bin", "proximadb-server"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=os.getcwd()
    )
    
    # Wait for server to start
    time.sleep(3)
    
    try:
        # Connect again
        print("6. Reconnecting to ProximaDB...")
        client2 = ProximaDBClient("localhost:5679")
        
        # Test 2: Check if collection persisted
        print("7. Checking collection persistence...")
        
        try:
            # List collections after restart
            collections_after_restart = await client2.list_collections()
            print(f"   Collections after restart: {len(collections_after_restart)}")
            
            # Find our collection again
            found_after_restart = None
            for col in collections_after_restart:
                col_name = col.config.name if col.config else col.name
                if col_name == collection_name:
                    found_after_restart = col
                    break
            
            if not found_after_restart:
                print("   ❌ Collection not found after restart - PERSISTENCE FAILED")
                return False
            
            col_name_restart = found_after_restart.config.name if found_after_restart.config else found_after_restart.name
            print(f"   ✅ Collection persisted: {col_name_restart}")
            print(f"   ✅ UUID matches: {found_after_restart.id == collection_uuid}")
            dimension = found_after_restart.config.dimension if found_after_restart.config else found_after_restart.dimension
            print(f"   ✅ Dimension: {dimension}")
            
            # Test 3: Get collection by name
            get_result = await client2.get_collection(collection_name)
            if get_result:
                get_col_name = get_result.config.name if get_result.config else get_result.name
                print(f"   ✅ Get by name successful: {get_col_name}")
            else:
                print("   ❌ Get by name failed")
                return False
            
            # Test 4: Delete collection
            print("8. Testing collection deletion...")
            delete_success = await client2.delete_collection(collection_name)
            if delete_success:
                print("   ✅ Collection deleted successfully")
                
                # Verify deletion
                collections_after_delete = await client2.list_collections()
                print(f"   Collections after deletion: {len(collections_after_delete)}")
                
                # Should not find the collection anymore
                found_after_delete = any(
                    (col.config.name if col.config else col.name) == collection_name 
                    for col in collections_after_delete
                )
                if found_after_delete:
                    print("   ❌ Collection still exists after deletion")
                    return False
                else:
                    print("   ✅ Collection successfully removed")
            else:
                print("   ❌ Collection deletion failed")
                return False
            
        except Exception as e:
            print(f"   ❌ Persistence test failed: {e}")
            return False
        
    finally:
        # Stop server
        server_process2.terminate()
        try:
            server_process2.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server_process2.kill()
            server_process2.wait()
    
    print("\n" + "=" * 70)
    print("🎉 ALL TESTS PASSED - FILESTORE PERSISTENCE WORKING!")
    print("✅ Collections persist across server restarts")
    print("✅ Collection operations work correctly")
    print("✅ gRPC handler properly connected to filestore backend")
    return True

if __name__ == "__main__":
    success = asyncio.run(test_collection_persistence())
    sys.exit(0 if success else 1)
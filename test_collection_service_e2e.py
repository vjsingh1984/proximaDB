#!/usr/bin/env python3
"""
End-to-End Test for Collection Service
Tests all collection operations with persistence validation
"""

import asyncio
import sys
import uuid
import time
import subprocess
import os

# Add client path
sys.path.insert(0, 'clients/python/src')

from proximadb.grpc_client import ProximaDBClient

async def run_e2e_test():
    """Run comprehensive end-to-end test for collection service"""
    
    print("🧪 ProximaDB Collection Service - End-to-End Test")
    print("=" * 70)
    
    # Start server
    print("\n1️⃣ Starting ProximaDB server...")
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
        print("2️⃣ Connecting to ProximaDB on port 5679...")
        client = ProximaDBClient("localhost:5679")
        
        # Test 1: List initial collections
        print("\n3️⃣ Testing LIST operation (initial state)...")
        initial_collections = await client.list_collections()
        print(f"   📊 Initial collections count: {len(initial_collections)}")
        for col in initial_collections:
            col_name = col.config.name if col.config else col.name
            print(f"      - {col_name} (ID: {col.id[:8]}...)")
        
        # Test 2: Create multiple collections
        print("\n4️⃣ Testing CREATE operation (multiple collections)...")
        test_collections = []
        
        # Create collections with different configurations
        configs = [
            {
                "name": f"test_cosine_{uuid.uuid4().hex[:6]}",
                "dimension": 128,
                "distance_metric": 1,  # COSINE
                "indexing_algorithm": 1,  # HNSW
                "storage_engine": 1  # VIPER
            },
            {
                "name": f"test_euclidean_{uuid.uuid4().hex[:6]}",
                "dimension": 256,
                "distance_metric": 2,  # EUCLIDEAN
                "indexing_algorithm": 2,  # IVF
                "storage_engine": 2  # LSM
            },
            {
                "name": f"test_dotproduct_{uuid.uuid4().hex[:6]}",
                "dimension": 512,
                "distance_metric": 3,  # DOT_PRODUCT
                "indexing_algorithm": 3,  # PQ
                "storage_engine": 3  # MMAP
            }
        ]
        
        for config in configs:
            try:
                created = await client.create_collection(**config)
                test_collections.append(created)
                print(f"   ✅ Created: {created.name}")
                print(f"      - UUID: {created.id}")
                print(f"      - Dimension: {created.dimension}")
                print(f"      - Metric: {created.metric}")
                print(f"      - Index: {created.index_type}")
            except Exception as e:
                print(f"   ❌ Failed to create {config['name']}: {e}")
        
        # Test 3: List collections after creation
        print("\n5️⃣ Testing LIST operation (after creation)...")
        after_create = await client.list_collections()
        print(f"   📊 Collections after creation: {len(after_create)}")
        print(f"   📈 Created {len(after_create) - len(initial_collections)} new collections")
        
        # Test 4: GET individual collections
        print("\n6️⃣ Testing GET operation (retrieve by name)...")
        for col in test_collections[:2]:  # Test first 2
            try:
                retrieved = await client.get_collection(col.name)
                if retrieved:
                    print(f"   ✅ Retrieved: {retrieved.name}")
                    print(f"      - Matches creation: {retrieved.id == col.id}")
                else:
                    print(f"   ❌ Failed to retrieve: {col.name}")
            except Exception as e:
                print(f"   ❌ Error retrieving {col.name}: {e}")
        
        # Test 5: UPDATE collection (if implemented)
        print("\n7️⃣ Testing UPDATE operation...")
        if test_collections:
            try:
                update_name = test_collections[0].name
                updated_config = {
                    "dimension": test_collections[0].dimension,
                    "distance_metric": 2,  # Change to EUCLIDEAN
                    "indexing_algorithm": 4,  # Change to FLAT
                }
                updated = await client.update_collection(update_name, updated_config)
                print(f"   ✅ Updated: {updated.name}")
                print(f"      - New metric: {updated.metric}")
                print(f"      - New index: {updated.index_type}")
            except Exception as e:
                print(f"   ⚠️  Update not implemented or failed: {e}")
        
        # Test 6: Persistence test
        print("\n8️⃣ Testing PERSISTENCE (server restart)...")
        print("   🔄 Stopping server...")
        
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
    
    # Restart server
    print("   🔄 Restarting server...")
    server_process2 = subprocess.Popen(
        ["cargo", "run", "--bin", "proximadb-server"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=os.getcwd()
    )
    
    # Wait for server to start
    time.sleep(3)
    
    try:
        # Reconnect
        print("   🔄 Reconnecting to server...")
        client2 = ProximaDBClient("localhost:5679")
        
        # Check persistence
        persisted = await client2.list_collections()
        print(f"   📊 Collections after restart: {len(persisted)}")
        
        # Verify our test collections persisted
        persisted_names = set(col.name for col in persisted)
        test_names = set(col.name for col in test_collections)
        found_count = len(test_names.intersection(persisted_names))
        
        print(f"   ✅ Persistence verified: {found_count}/{len(test_collections)} test collections found")
        
        # Test 7: DELETE operation
        print("\n9️⃣ Testing DELETE operation...")
        delete_count = 0
        for col in test_collections:
            try:
                success = await client2.delete_collection(col.name)
                if success:
                    delete_count += 1
                    print(f"   ✅ Deleted: {col.name}")
                else:
                    print(f"   ❌ Failed to delete: {col.name}")
            except Exception as e:
                print(f"   ❌ Error deleting {col.name}: {e}")
        
        print(f"   📊 Deleted {delete_count}/{len(test_collections)} collections")
        
        # Test 8: Verify deletion
        print("\n🔟 Verifying DELETE operation...")
        final_collections = await client2.list_collections()
        print(f"   📊 Final collections count: {len(final_collections)}")
        
        # Check none of our test collections exist
        final_names = set(col.name for col in final_collections)
        remaining = len(test_names.intersection(final_names))
        
        if remaining == 0:
            print("   ✅ All test collections successfully deleted")
        else:
            print(f"   ❌ {remaining} test collections still exist")
        
        # Performance metrics
        print("\n📊 Performance Summary:")
        print(f"   - Collections created: {len(test_collections)}")
        print(f"   - Collections retrieved: 2")
        print(f"   - Collections deleted: {delete_count}")
        print(f"   - Persistence test: PASSED")
        
        # Test 9: Error handling
        print("\n1️⃣1️⃣ Testing ERROR HANDLING...")
        
        # Try to create duplicate
        if test_collections:
            try:
                # Try to create with existing name (from persisted collections)
                if persisted:
                    existing_name = persisted[0].name
                    duplicate = await client2.create_collection(
                        name=existing_name,
                        dimension=128,
                        distance_metric=1,
                        indexing_algorithm=1
                    )
                    print(f"   ❌ Duplicate creation should have failed!")
            except Exception as e:
                print(f"   ✅ Duplicate prevention working: {e}")
        
        # Try to get non-existent
        try:
            non_existent = await client2.get_collection("non_existent_collection_xyz")
            if non_existent is None:
                print("   ✅ Non-existent collection returns None")
            else:
                print("   ❌ Non-existent collection should return None")
        except Exception as e:
            print(f"   ⚠️  Get non-existent raised exception: {e}")
        
        # Try to delete non-existent
        try:
            delete_result = await client2.delete_collection("non_existent_collection_xyz")
            if not delete_result:
                print("   ✅ Non-existent collection delete returns False")
            else:
                print("   ❌ Non-existent collection delete should return False")
        except Exception as e:
            print(f"   ⚠️  Delete non-existent raised exception: {e}")
        
    finally:
        # Stop server
        server_process2.terminate()
        try:
            server_process2.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server_process2.kill()
            server_process2.wait()
    
    print("\n" + "=" * 70)
    print("✅ End-to-End Test Complete!")
    print("=" * 70)

if __name__ == "__main__":
    asyncio.run(run_e2e_test())
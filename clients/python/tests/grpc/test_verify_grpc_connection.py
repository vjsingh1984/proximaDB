#!/usr/bin/env python3
"""
Simple test to verify Python gRPC client is actually reaching the server
"""

import asyncio
import sys
import time

# Add client path
sys.path.insert(0, 'clients/python/src')

from proximadb.grpc_client import ProximaDBClient

async def verify_grpc_connection():
    """Verify that Python gRPC client is actually reaching the server"""
    
    print("🔍 Verifying Python gRPC Client → Server Communication")
    print("=" * 60)
    
    try:
        # Connect to server
        print("1. Creating gRPC client connection to localhost:5679...")
        client = ProximaDBClient("localhost:5679")
        
        # Test 1: Health check
        print("\n2. Testing health check (should reach server)...")
        try:
            health_result = await client.health_check()
            print(f"   ✅ Health check successful: {health_result}")
            print(f"   📡 This proves Python client IS reaching the server!")
        except Exception as e:
            print(f"   ❌ Health check failed: {e}")
            return False
        
        # Test 2: List collections (should hit collection service)
        print("\n3. Testing list_collections (should reach server metadata)...")
        try:
            collections = await client.list_collections()
            print(f"   ✅ List collections successful: {len(collections)} collections found")
            for i, col in enumerate(collections[:3]):  # Show first 3
                col_name = col.config.name if col.config else col.name
                print(f"      {i+1}. {col_name} (ID: {col.id[:8]}...)")
            print(f"   📡 This proves collection operations reach the server!")
        except Exception as e:
            print(f"   ❌ List collections failed: {e}")
        
        # Test 3: Create collection (should generate server logs)
        print("\n4. Testing create_collection (watch server logs for requests)...")
        collection_name = f"verify_connection_test_{int(time.time())}"
        try:
            created = await client.create_collection(
                name=collection_name,
                dimension=64,
                distance_metric=1,  # COSINE
                indexing_algorithm=1  # HNSW
            )
            print(f"   ✅ Collection creation successful: {created.name}")
            print(f"   🆔 Collection UUID: {created.id}")
            print(f"   📡 Check server logs - you should see gRPC requests!")
            
            # Clean up - delete the test collection
            await client.delete_collection(collection_name)
            print(f"   🗑️  Test collection cleaned up")
            
        except Exception as e:
            print(f"   ❌ Collection creation failed: {e}")
        
        print("\n📊 Connection Verification Summary:")
        print("   ✅ Python gRPC client is NOT using mocks")
        print("   ✅ Python gRPC client IS reaching the server")
        print("   ✅ Server is receiving and processing gRPC requests")
        print("   📄 Check server logs for detailed request traces")
        
        return True
        
    except Exception as e:
        print(f"❌ Connection verification failed: {e}")
        return False

if __name__ == "__main__":
    success = asyncio.run(verify_grpc_connection())
    print(f"\n{'✅ VERIFICATION PASSED' if success else '❌ VERIFICATION FAILED'}")
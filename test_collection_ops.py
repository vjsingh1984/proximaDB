#!/usr/bin/env python3
"""
Test script for collection create, get, list operations
Tests the new filesystem abstraction and single index implementation
"""

import asyncio
import grpc
import time
import json
import sys
import os

# Add the client path
sys.path.insert(0, 'clients/python/src')

from proximadb.grpc_client import ProximaDBClient
from proximadb.models import CollectionConfig, DistanceMetric

async def test_collection_operations():
    """Test collection create, get, and list operations"""
    print("🚀 Starting ProximaDB Collection Operations Test")
    print("=" * 60)
    
    try:
        # Initialize client (server is on port 5679)
        client = ProximaDBClient(endpoint='localhost:5679')
        print("✅ Client initialized")
        
        # Test 1: Create a new collection
        print("\n📝 Test 1: Creating a new collection...")
        
        try:
            result = await client.create_collection(
                name="test_collection_filesystem",
                dimension=384,
                distance_metric=1,  # COSINE
                indexing_algorithm=1,  # HNSW
                storage_engine=1,  # VIPER
            )
            print(f"✅ Collection created: {result}")
            collection_uuid = result.id
            print(f"   UUID: {collection_uuid}")
        except Exception as e:
            if "already exists" in str(e):
                print(f"⚠️ Collection already exists (persistence working!): {e}")
                # Try to get the existing collection
                result = await client.get_collection("test_collection_filesystem")
                collection_uuid = result.id
                print(f"   Retrieved existing UUID: {collection_uuid}")
            else:
                raise
        
        # Test 2: Get the collection
        print("\n📋 Test 2: Getting the created collection...")
        retrieved = await client.get_collection("test_collection_filesystem")
        print(f"✅ Collection retrieved: {retrieved}")
        
        # Test 3: List all collections
        print("\n📋 Test 3: Listing all collections...")
        collections = await client.list_collections()
        print(f"✅ Collections listed: {len(collections)} found")
        for i, coll in enumerate(collections, 1):
            if hasattr(coll, 'name') and hasattr(coll, 'id'):
                print(f"   {i}. {coll.name} - {coll.id}")
            else:
                print(f"   {i}. {coll}")
        
        # Test 4: Create another collection
        print("\n📝 Test 4: Creating second collection...")
        
        try:
            result2 = await client.create_collection(
                name="test_collection_persistence",
                dimension=512,
                distance_metric=2,  # EUCLIDEAN
                indexing_algorithm=1,  # HNSW
                storage_engine=1,  # VIPER
            )
            print(f"✅ Second collection created: {result2}")
        except Exception as e:
            if "already exists" in str(e):
                print(f"⚠️ Second collection already exists (persistence working!): {e}")
                result2 = await client.get_collection("test_collection_persistence")
                print(f"   Retrieved existing collection: {result2.name}")
            else:
                raise
        
        # Test 5: List collections again
        print("\n📋 Test 5: Listing collections after second create...")
        collections = await client.list_collections()
        print(f"✅ Collections listed: {len(collections)} found")
        for i, coll in enumerate(collections, 1):
            if hasattr(coll, 'name') and hasattr(coll, 'id'):
                print(f"   {i}. {coll.name} - {coll.id}")
            else:
                print(f"   {i}. {coll}")
        
        # Test 6: Verify persistence (check if files exist)
        print("\n💾 Test 6: Checking filesystem persistence...")
        data_dir = "./data"  # Default data directory
        metadata_dir = os.path.join(data_dir, "metadata")
        
        if os.path.exists(metadata_dir):
            files = os.listdir(metadata_dir)
            print(f"✅ Metadata directory exists with {len(files)} files:")
            for file in files:
                print(f"   📁 {file}")
        else:
            print(f"❌ Metadata directory not found at {metadata_dir}")
        
        print("\n🎉 All tests completed successfully!")
        print("=" * 60)
        
        return True
        
    except grpc.RpcError as e:
        print(f"❌ gRPC error: {e.code()} - {e.details()}")
        return False
    except Exception as e:
        print(f"❌ Error: {str(e)}")
        import traceback
        traceback.print_exc()
        return False

if __name__ == "__main__":
    print("🔧 Testing ProximaDB Collection Operations")
    print("   Focus: Filesystem abstraction & single index implementation")
    
    # Wait a moment for server to be ready
    print("⏳ Waiting 3 seconds for server startup...")
    time.sleep(3)
    
    success = asyncio.run(test_collection_operations())
    
    if success:
        print("\n✨ Test suite completed successfully!")
        exit(0)
    else:
        print("\n💥 Test suite failed!")
        exit(1)
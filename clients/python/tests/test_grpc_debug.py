#!/usr/bin/env python3
"""
Debug gRPC Collection Creation
"""

import time
from proximadb import ProximaDBClient, Protocol
from proximadb import proximadb_pb2 as pb2


def test_collection_creation():
    """Debug collection creation issue"""
    
    print("🔍 Debugging gRPC Collection Creation")
    print("=" * 60)
    
    # Initialize client
    client = ProximaDBClient(url="grpc://localhost:5679", protocol=Protocol.GRPC)
    print("✅ Client initialized with debug logging")
    
    # Try to create collection with minimal config
    collection_name = f"debug_test_{int(time.time())}"
    print(f"\n📦 Attempting to create collection: {collection_name}")
    
    try:
        # First, let's check what proto fields are required
        print("\n🔍 Creating CollectionConfig proto...")
        config = pb2.CollectionConfig()
        config.name = collection_name
        config.dimension = 128
        config.distance_metric = pb2.DistanceMetric.COSINE
        config.primary_indexing_algorithm = pb2.IndexingAlgorithm.HNSW
        config.storage_engine = pb2.StorageEngine.VIPER
        
        print(f"   Name: {config.name}")
        print(f"   Dimension: {config.dimension}")
        print(f"   Distance metric: {config.distance_metric}")
        print(f"   Storage engine: {config.storage_engine}")
        
        # Create request
        print("\n🔍 Creating CollectionRequest proto...")
        request = pb2.CollectionRequest()
        request.operation = pb2.CollectionOperation.COLLECTION_CREATE
        request.collection_config.CopyFrom(config)
        
        print(f"   Operation: {request.operation}")
        print(f"   Has config: {request.HasField('collection_config')}")
        
        # Make the call directly
        print("\n🔍 Making gRPC call...")
        response = client._call_with_timeout(client.stub.CollectionOperation, request)
        
        print(f"\n📨 Response received:")
        print(f"   Success: {response.success}")
        print(f"   Error message: {response.error_message}")
        print(f"   Error code: {response.error_code}")
        
        if response.collection:
            print(f"   Collection ID: {response.collection.id}")
            print(f"   Collection fields: {[field.name for field in response.collection.DESCRIPTOR.fields]}")
            if hasattr(response.collection, 'name'):
                print(f"   Collection name: {response.collection.name}")
            if hasattr(response.collection, 'config') and response.collection.config:
                print(f"   Collection config name: {response.collection.config.name}")
        
    except Exception as e:
        print(f"\n❌ Exception: {type(e).__name__}: {e}")
        import traceback
        traceback.print_exc()
    
    # Try using the method directly
    print("\n\n🔍 Testing create_collection method...")
    try:
        collection = client.create_collection(
            name=f"method_test_{int(time.time())}",
            dimension=128
        )
        print(f"✅ Success! Collection created: {collection.name}")
    except Exception as e:
        print(f"❌ Method failed: {e}")
        
        # Check if it's a proto field issue
        if "name" in str(e):
            print("\n⚠️  Error mentions 'name' - checking proto definition...")
            
            # Try to see what fields CollectionConfig has
            config = pb2.CollectionConfig()
            print(f"   CollectionConfig fields: {config.DESCRIPTOR.fields_by_name.keys()}")


if __name__ == "__main__":
    test_collection_creation()
#!/usr/bin/env python3
"""
Test script for the v1 ProximaDB client to validate imports and basic functionality.
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

def test_v1_imports():
    """Test that v1 proto and client imports work correctly"""
    print("Testing v1 client imports...")
    
    try:
        # Test proto imports
        from proximadb.proto.proximadb.v1 import (
            vector_pb2, 
            vector_pb2_grpc,
            collection_pb2,
            collection_pb2_grpc,
            vector_types_pb2,
            collection_types_pb2
        )
        print("✅ Proto v1 imports successful")
        
        # Test client import
        from proximadb.client_v1 import ProximaDBClientV1
        print("✅ Client v1 import successful")
        
        # Test client instantiation
        client = ProximaDBClientV1(url="http://localhost:5678")
        print("✅ Client v1 instantiation successful")
        print(f"   Protocol: {client.protocol}")
        print(f"   Base URL: {client.base_url}")
        
        # Test model imports
        from proximadb.models import VectorRecord, SearchResult, Collection, DistanceMetric, StorageEngine
        print("✅ Model imports successful")
        
        # Test VectorRecord creation
        vector = VectorRecord(
            id="test_vector",
            vector=[0.1, 0.2, 0.3, 0.4],
            metadata={"test": "data"}
        )
        print(f"✅ VectorRecord creation successful: {vector.id}")
        
        return True
        
    except ImportError as e:
        print(f"❌ Import error: {e}")
        return False
    except Exception as e:
        print(f"❌ General error: {e}")
        return False

def test_proto_message_creation():
    """Test that we can create proto messages"""
    print("\nTesting proto message creation...")
    
    try:
        from proximadb.proto.proximadb.v1 import vector_types_pb2, collection_types_pb2
        
        # Test VectorRecord proto creation (metadata as empty dict for proto compatibility)
        vector_proto = vector_types_pb2.VectorRecord(
            id="test_id",
            vector=[1.0, 2.0, 3.0]
        )
        print(f"✅ VectorRecord proto created: {vector_proto.id}")
        
        # Test CollectionConfig proto creation
        collection_proto = collection_types_pb2.CollectionConfig(
            name="test_collection",
            dimension=128,
            distance_metric="COSINE",
            storage_engine="SST"
        )
        print(f"✅ CollectionConfig proto created: {collection_proto.name}")
        
        # Test VectorBatchRequest proto creation
        batch_request = vector_types_pb2.VectorBatchRequest(
            collection_id="test_collection",
            vectors=[vector_proto]
        )
        print(f"✅ VectorBatchRequest proto created with {len(batch_request.vectors)} vectors")
        
        return True
        
    except Exception as e:
        print(f"❌ Proto message creation error: {e}")
        return False

def main():
    """Run all tests"""
    print("ProximaDB Python SDK v1 Client Test")
    print("=" * 50)
    
    success = True
    
    # Test imports
    success &= test_v1_imports()
    
    # Test proto message creation
    success &= test_proto_message_creation()
    
    print("\n" + "=" * 50)
    if success:
        print("🎉 All tests passed! v1 client is ready.")
        return 0
    else:
        print("❌ Some tests failed. Check the output above.")
        return 1

if __name__ == "__main__":
    sys.exit(main())
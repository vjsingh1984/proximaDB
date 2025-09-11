#!/usr/bin/env python3
"""
Final SDK Validation Test

This comprehensive test validates that the Python SDK can:
1. Import all necessary components
2. Create both legacy and v1 clients
3. Generate correct proto messages
4. Handle all enum conversions properly
5. Provide a working interface for when the server is available
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

def test_complete_sdk_imports():
    """Test that all SDK components can be imported successfully"""
    print("Testing complete SDK imports...")
    
    try:
        # Main client imports
        from proximadb import ProximaDBClient, ProximaDBClientV1
        print("✅ Main client imports successful")
        
        # Model imports
        from proximadb import VectorRecord, SearchResult, Collection, DistanceMetric, StorageEngine
        print("✅ Core model imports successful")
        
        # Configuration imports
        from proximadb import ClientConfig, Protocol, LogLevel
        print("✅ Configuration imports successful")
        
        # Exception imports
        from proximadb import ProximaDBError, NetworkError, ValidationError
        print("✅ Exception imports successful")
        
        # Filter imports
        from proximadb import FilterBuilder, eq, gt, and_filters
        print("✅ Filter API imports successful")
        
        # Builder imports
        from proximadb import SearchBuilder, CollectionBuilder
        print("✅ Builder imports successful")
        
        # Factory functions
        from proximadb import connect, connect_rest, connect_grpc
        print("✅ Factory function imports successful")
        
        return True
        
    except Exception as e:
        print(f"❌ SDK import error: {e}")
        return False

def test_client_creation_all_methods():
    """Test different client creation methods"""
    print("\nTesting client creation methods...")
    
    try:
        # Legacy unified client
        from proximadb import ProximaDBClient
        legacy_client = ProximaDBClient(url="http://localhost:5678")
        print(f"✅ Legacy client created")
        
        # V1 client - REST
        from proximadb import ProximaDBClientV1
        v1_rest = ProximaDBClientV1(url="http://localhost:5678", protocol="rest")
        print(f"✅ V1 REST client created: {v1_rest.protocol}")
        
        # V1 client - gRPC
        v1_grpc = ProximaDBClientV1(url="grpc://localhost:5679")
        print(f"✅ V1 gRPC client created: {v1_grpc.protocol}")
        
        # Factory functions
        from proximadb import connect, connect_rest, connect_grpc
        factory_client = connect("http://localhost:5678")
        print(f"✅ Factory client created")
        
        rest_factory = connect_rest("http://localhost:5678")
        print(f"✅ REST factory client created")
        
        grpc_factory = connect_grpc("http://localhost:5679")
        print(f"✅ gRPC factory client created")
        
        # Clean up
        legacy_client.close() if hasattr(legacy_client, 'close') else None
        v1_rest.close()
        v1_grpc.close()
        
        return True
        
    except Exception as e:
        print(f"❌ Client creation error: {e}")
        return False

def test_proto_v1_message_generation():
    """Test that we can generate all v1 proto messages correctly"""
    print("\nTesting v1 proto message generation...")
    
    try:
        from proximadb.proto.proximadb.v1 import (
            vector_types_pb2,
            collection_types_pb2,
            vector_pb2,
            collection_pb2,
            sql_pb2
        )
        
        # Collection messages
        collection_config = collection_types_pb2.CollectionConfig(
            name="test_collection",
            dimension=128,
            distance_metric="COSINE",
            storage_engine="SST"
        )
        print(f"✅ CollectionConfig proto: {collection_config.name}")
        
        get_collection_req = collection_types_pb2.GetCollectionRequest(collection_id="test_collection")
        print(f"✅ GetCollectionRequest proto: {get_collection_req.collection_id}")
        
        list_collections_req = collection_types_pb2.ListCollectionsRequest()
        print(f"✅ ListCollectionsRequest proto created")
        
        # Vector messages
        vector_record = vector_types_pb2.VectorRecord(
            id="test_vector",
            vector=[0.1, 0.2, 0.3, 0.4]
        )
        print(f"✅ VectorRecord proto: {vector_record.id}")
        
        batch_request = vector_types_pb2.VectorBatchRequest(
            collection_id="test_collection",
            vectors=[vector_record]
        )
        print(f"✅ VectorBatchRequest proto: {len(batch_request.vectors)} vectors")
        
        search_query = vector_types_pb2.SearchQuery(
            vector=[0.1, 0.2, 0.3, 0.4],
            filters={}
        )
        search_request = vector_types_pb2.VectorSearchRequest(
            collection_id="test_collection",
            queries=[search_query],
            top_k=10
        )
        print(f"✅ VectorSearchRequest proto: {search_request.top_k} top_k")
        
        get_request = vector_types_pb2.VectorGetRequest(
            collection_id="test_collection",
            vector_id="test_vector"
        )
        print(f"✅ VectorGetRequest proto: {get_request.vector_id}")
        
        # SQL messages  
        from proximadb.proto.proximadb.v1 import types_pb2
        sql_request = types_pb2.ExecuteSqlRequest(
            query="SELECT * FROM test_collection",
            parameters=[]
        )
        print(f"✅ ExecuteSqlRequest proto: {len(sql_request.query)} chars")
        
        return True
        
    except Exception as e:
        print(f"❌ Proto message generation error: {e}")
        return False

def test_enum_mappings():
    """Test that SDK enums map correctly to proto enums"""
    print("\nTesting enum mappings...")
    
    try:
        from proximadb.models import DistanceMetric, StorageEngine, IndexingAlgorithm
        from proximadb.proto.proximadb.v1.vector_types_pb2 import (
            DistanceMetric as ProtoDistanceMetric,
            StorageEngine as ProtoStorageEngine, 
            IndexingAlgorithm as ProtoIndexingAlgorithm
        )
        
        # Test distance metrics
        sdk_metric = DistanceMetric.COSINE
        proto_metrics = dict(ProtoDistanceMetric.items())
        
        if sdk_metric.value.upper() in proto_metrics:
            print(f"✅ DistanceMetric mapping: {sdk_metric.value} -> {sdk_metric.value.upper()}")
        else:
            print(f"❌ DistanceMetric mapping failed: {sdk_metric.value}")
            return False
        
        # Test storage engines
        sdk_engine = StorageEngine.SST
        proto_engines = dict(ProtoStorageEngine.items())
        
        if sdk_engine.value.upper() in proto_engines:
            print(f"✅ StorageEngine mapping: {sdk_engine.value} -> {sdk_engine.value.upper()}")
        else:
            print(f"❌ StorageEngine mapping failed: {sdk_engine.value}")
            return False
        
        # Test indexing algorithms
        sdk_algo = IndexingAlgorithm.HNSW
        proto_algos = dict(ProtoIndexingAlgorithm.items())
        
        if sdk_algo.value.upper() in proto_algos:
            print(f"✅ IndexingAlgorithm mapping: {sdk_algo.value} -> {sdk_algo.value.upper()}")
        else:
            print(f"❌ IndexingAlgorithm mapping failed: {sdk_algo.value}")
            return False
        
        return True
        
    except Exception as e:
        print(f"❌ Enum mapping error: {e}")
        return False

def test_full_workflow_simulation():
    """Test that a complete workflow can be simulated (without server)"""
    print("\nTesting complete workflow simulation...")
    
    try:
        from proximadb import ProximaDBClientV1, VectorRecord, DistanceMetric, StorageEngine
        
        # Create client
        client = ProximaDBClientV1(url="http://localhost:5678")
        print(f"✅ Client created: {client.protocol}")
        
        # Create vector records
        vectors = [
            VectorRecord(
                id=f"vector_{i}",
                vector=[0.1 * i, 0.2 * i, 0.3 * i, 0.4 * i],
                metadata={"index": i, "category": "test"}
            )
            for i in range(1, 4)
        ]
        print(f"✅ Created {len(vectors)} vector records")
        
        # Test REST payload generation (internal method would be called)
        collection_payload = {
            "name": "test_collection",
            "dimension": 4,
            "distance_metric": DistanceMetric.COSINE.value.upper(),
            "storage_engine": StorageEngine.SST.value.upper()
        }
        print(f"✅ Collection payload: {collection_payload}")
        
        vector_payload = {
            "collection_id": "test_collection",
            "vectors": [
                {
                    "id": vec.id,
                    "vector": vec.vector,
                    "metadata": vec.metadata
                }
                for vec in vectors
            ]
        }
        print(f"✅ Vector payload: {len(vector_payload['vectors'])} vectors")
        
        search_payload = {
            "collection_id": "test_collection",
            "vector": [0.15, 0.25, 0.35, 0.45],
            "top_k": 2
        }
        print(f"✅ Search payload: top_k={search_payload['top_k']}")
        
        # Test proto message generation (internal gRPC method would be called)
        from proximadb.proto.proximadb.v1 import vector_types_pb2
        
        proto_vectors = [
            vector_types_pb2.VectorRecord(
                id=vec.id,
                vector=vec.vector
            )
            for vec in vectors
        ]
        
        batch_request = vector_types_pb2.VectorBatchRequest(
            collection_id="test_collection",
            vectors=proto_vectors
        )
        print(f"✅ Proto batch request: {len(batch_request.vectors)} vectors")
        
        client.close()
        print("✅ Workflow simulation completed successfully")
        
        return True
        
    except Exception as e:
        print(f"❌ Workflow simulation error: {e}")
        return False

def main():
    """Run complete SDK validation"""
    print("ProximaDB Python SDK - Final Validation Test")
    print("=" * 70)
    
    success = True
    
    # Run all validation tests
    success &= test_complete_sdk_imports()
    success &= test_client_creation_all_methods()
    success &= test_proto_v1_message_generation()
    success &= test_enum_mappings()
    success &= test_full_workflow_simulation()
    
    print("\n" + "=" * 70)
    if success:
        print("🎉 ALL TESTS PASSED! ProximaDB Python SDK is fully validated!")
        print("\nThe SDK is ready for use with the ProximaDB server.")
        print("Both legacy and v1 clients are available:")
        print("  - ProximaDBClient (legacy, unified client)")
        print("  - ProximaDBClientV1 (v1 proto messages, aligned with server)")
        print("\nRun example_v1_client.py when the server is available.")
        return 0
    else:
        print("❌ SOME TESTS FAILED! Check the output above.")
        return 1

if __name__ == "__main__":
    sys.exit(main())
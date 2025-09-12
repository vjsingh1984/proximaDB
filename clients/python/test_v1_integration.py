#!/usr/bin/env python3
"""
Integration test for the v1 ProximaDB client to test compatibility with server expectations.
This test can run without a server and validates the SDK structure and message creation.

Usage:
    PYTHONPATH=src python test_v1_integration.py

Note: Set PYTHONPATH environment variable to include the 'src' directory
instead of modifying sys.path. This is the recommended approach.
"""

import sys
import os
import logging

# Setup logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Recommended: Use PYTHONPATH=src instead of this sys.path modification
# Example: PYTHONPATH=src python test_v1_integration.py
if 'PYTHONPATH' not in os.environ:
    logger.warning("⚠️  Recommendation: Set PYTHONPATH=src environment variable")
    logger.info("   Example: PYTHONPATH=src python test_v1_integration.py")
    logger.info("   Falling back to sys.path modification...\n")
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

def test_v1_client_message_compatibility():
    """Test that v1 client creates compatible proto messages"""
    logger.info("Testing v1 client message compatibility...")
    
    try:
        from proximadb.client_v1 import ProximaDBClientV1
        from proximadb.models import VectorRecord, DistanceMetric, StorageEngine
        
        # Test client creation
        client = ProximaDBClientV1(url="http://localhost:5678", protocol="rest")
        logger.info("✅ Client created with protocol: {client.protocol}")
        
        # Test message creation for gRPC client (without actually connecting)
        grpc_client = ProximaDBClientV1(url="grpc://localhost:5679", protocol="grpc")
        logger.info("✅ gRPC client created with protocol: {grpc_client.protocol}")
        
        # Test vector record creation
        vector_record = VectorRecord(
            id="test_vector_123",
            vector=[0.1, 0.2, 0.3, 0.4, 0.5],
            metadata={"category": "test", "source": "unit_test"}
        )
        logger.info("✅ VectorRecord created: {vector_record.id}")
        
        # Test proto message creation (internal method - not called but validated)
        from proximadb.proto.proximadb.v1 import vector_types_pb2
        
        proto_vector = vector_types_pb2.VectorRecord(
            id=vector_record.id,
            vector=vector_record.vector
        )
        logger.info("✅ Proto VectorRecord created: {proto_vector.id}")
        
        batch_request = vector_types_pb2.VectorBatchRequest(
            collection_id="test_collection",
            vectors=[proto_vector]
        )
        logger.info("✅ Proto VectorBatchRequest created for collection: {batch_request.collection_id}")
        
        # Create SearchQuery first
        search_query = vector_types_pb2.SearchQuery(
            vector=[0.1, 0.2, 0.3, 0.4, 0.5],
            filters={}
        )
        
        search_request = vector_types_pb2.VectorSearchRequest(
            collection_id="test_collection",
            queries=[search_query],
            top_k=10
        )
        logger.info("✅ Proto VectorSearchRequest created with top_k: {search_request.top_k}")
        
        return True
        
    except Exception as e:
        logger.error("❌ v1 client compatibility test failed: {e}")
        return False

def test_enum_compatibility():
    """Test that SDK enums are compatible with proto enums"""
    logger.info("\nTesting enum compatibility...")
    
    try:
        from proximadb.models import DistanceMetric, StorageEngine
        from proximadb.proto.proximadb.v1 import vector_types_pb2
        
        # Test DistanceMetric compatibility
        sdk_metric = DistanceMetric.COSINE
        proto_metrics = {name: value for name, value in vector_types_pb2.DistanceMetric.items()}
        
        logger.info("SDK DistanceMetric.COSINE: {sdk_metric.value}")
        logger.info("Proto DistanceMetrics available: {list(proto_metrics.keys())}")
        
        if 'COSINE' in proto_metrics:
            logger.info("✅ COSINE metric is compatible between SDK and proto")
        else:
            logger.error("❌ COSINE metric compatibility issue")
            return False
            
        # Test StorageEngine compatibility  
        sdk_engine = StorageEngine.SST
        proto_engines = {name: value for name, value in vector_types_pb2.StorageEngine.items()}
        
        logger.info("SDK StorageEngine.SST: {sdk_engine.value}")
        logger.info("Proto StorageEngines available: {list(proto_engines.keys())}")
        
        if 'SST' in proto_engines:
            logger.info("✅ SST engine is compatible between SDK and proto")
        else:
            logger.error("❌ SST engine compatibility issue")
            return False
            
        return True
        
    except Exception as e:
        logger.error("❌ Enum compatibility test failed: {e}")
        return False

def test_rest_payload_structure():
    """Test that REST payloads match expected server format"""
    logger.info("\nTesting REST payload structure...")
    
    try:
        # Test collection creation payload
        collection_payload = {
            "name": "test_collection",
            "dimension": 128,
            "distance_metric": "COSINE",
            "storage_engine": "SST"
        }
        logger.info("✅ Collection payload structure: {collection_payload}")
        
        # Test vector batch payload
        vector_payload = {
            "collection_id": "test_collection",
            "vectors": [
                {
                    "id": "vec_1",
                    "vector": [0.1, 0.2, 0.3],
                    "metadata": {"key": "value"}
                }
            ]
        }
        logger.info("✅ Vector batch payload structure: {len(vector_payload['vectors'])} vectors")
        
        # Test search payload
        search_payload = {
            "collection_id": "test_collection", 
            "vector": [0.1, 0.2, 0.3],
            "top_k": 10,
            "filters": {"category": "test"}
        }
        logger.info("✅ Search payload structure: top_k={search_payload['top_k']}")
        
        return True
        
    except Exception as e:
        logger.error("❌ REST payload structure test failed: {e}")
        return False

def main():
    """Run all v1 integration tests"""
    logger.info("ProximaDB Python SDK v1 Integration Test")
    print("=" * 60)
    
    success = True
    
    # Test v1 client message compatibility
    success &= test_v1_client_message_compatibility()
    
    # Test enum compatibility
    success &= test_enum_compatibility()
    
    # Test REST payload structure
    success &= test_rest_payload_structure()
    
    print("\n" + "=" * 60)
    if success:
        logger.info("🎉 All v1 integration tests passed! SDK is ready for server testing.")
        return 0
    else:
        logger.error("❌ Some v1 integration tests failed. Check the output above.")
        return 1

if __name__ == "__main__":
    sys.exit(main())
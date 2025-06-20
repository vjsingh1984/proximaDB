#!/usr/bin/env python3
"""
Test the new gRPC client with correct protobuf files

Copyright 2025 ProximaDB
"""

import sys
import logging

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# Add Python SDK path
sys.path.insert(0, '/home/vsingh/code/proximadb/clients/python/src')

try:
    from proximadb.grpc_client import ProximaDBGrpcClient
    logger.info("✅ Successfully imported new ProximaDBGrpcClient")
except ImportError as e:
    logger.error(f"❌ Import failed: {e}")
    exit(1)


def test_new_grpc_client():
    """Test the new gRPC client"""
    logger.info("🎯 Testing new gRPC client with correct protobuf files")
    
    try:
        # Create client
        client = ProximaDBGrpcClient(
            endpoint="localhost:5679",
            timeout=10.0,
            enable_debug_logging=False
        )
        logger.info("✅ Client created successfully")
        
        # Test health check
        health = client.health_check()
        logger.info(f"🩺 Health check: status={health.status}, version={health.version}")
        
        # Test list collections
        collections = client.list_collections()
        logger.info(f"📋 Collections: {len(collections)} found")
        
        # Test create collection
        collection = client.create_collection(
            name="test_new_client_collection",
            dimension=128,
            distance_metric="cosine"
        )
        logger.info(f"📁 Created collection: {collection.name}")
        
        # Test insert vector
        insert_result = client.insert_vector(
            collection_name="test_new_client_collection",
            vector_id="test_vector_1",
            vector=[0.5] * 128,
            metadata={"source": "new_client_test"}
        )
        logger.info(f"📌 Insert result: success={insert_result.success}")
        
        # Test search vectors
        search_results = client.search_vectors(
            collection_name="test_new_client_collection",
            query_vector=[0.5] * 128,
            k=5
        )
        logger.info(f"🔍 Search results: {len(search_results)} found")
        
        # Cleanup
        delete_result = client.delete_collection("test_new_client_collection")
        logger.info(f"🗑️ Delete collection: success={delete_result.success}")
        
        client.close()
        logger.info("✅ All tests completed successfully!")
        return True
        
    except Exception as e:
        logger.error(f"❌ Test failed: {e}")
        return False


if __name__ == "__main__":
    success = test_new_grpc_client()
    exit(0 if success else 1)
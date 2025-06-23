#!/usr/bin/env python3
"""
Simple gRPC Vector Insert Test

Tests vector insertion via gRPC service to verify WAL persistence.
"""

import asyncio
import json
import logging
import sys
from typing import List, Dict, Any

# Add the client to path
sys.path.insert(0, '/workspace/clients/python/src')

from proximadb.grpc_client import ProximaDBClient
from proximadb.models import Vector

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

async def test_grpc_vector_insertion():
    """Test gRPC vector insertion with WAL verification"""
    
    # Connect to gRPC service (default port 5679)
    client = ProximaDBClient("localhost:5679")
    
    try:
        # 1. Health check
        logger.info("🔍 Testing gRPC health check...")
        health = await client.health_check()
        logger.info(f"✅ Health status: {health.status}")
        
        # 2. Create test collection
        collection_name = "grpc_test_collection"
        logger.info(f"🔍 Creating collection: {collection_name}")
        
        # Try to delete existing collection first
        try:
            await client.delete_collection(collection_name)
            logger.info(f"🧹 Deleted existing collection: {collection_name}")
        except:
            pass  # Collection might not exist
        
        collection = await client.create_collection(
            name=collection_name,
            dimension=384,
            distance_metric=1,  # COSINE
            storage_engine=1,   # VIPER
            indexing_algorithm=1  # HNSW
        )
        logger.info(f"✅ Created collection: {collection.name} (dimension: {collection.dimension})")
        
        # 3. Prepare test vectors 
        test_vectors = [
            {
                "id": "grpc_vec_1",
                "vector": [0.1] * 384,  # 384-dimensional BERT-like vector
                "metadata": {
                    "source": "grpc_test",
                    "type": "test_vector_1"
                }
            },
            {
                "id": "grpc_vec_2", 
                "vector": [0.2] * 384,
                "metadata": {
                    "source": "grpc_test",
                    "type": "test_vector_2"
                }
            }
        ]
        
        # 4. Insert vectors via gRPC
        logger.info(f"🔍 Inserting {len(test_vectors)} vectors via gRPC...")
        result = client.insert_vectors(
            collection_id=collection_name,
            vectors=test_vectors,
            upsert=False
        )
        
        logger.info(f"✅ Vector insertion successful!")
        logger.info(f"   Inserted count: {result.count}")
        logger.info(f"   Failed count: {result.failed_count}")
        logger.info(f"   Duration: {result.duration_ms:.2f}ms")
        
        # 5. Search for inserted vectors
        logger.info("🔍 Testing vector search...")
        query_vector = [0.15] * 384  # Similar to our test vectors
        
        search_results = client.search_vectors(
            collection_id=collection_name,
            query_vectors=[query_vector],
            top_k=5,
            include_metadata=True,
            include_vectors=False
        )
        
        logger.info(f"✅ Search completed, found {len(search_results)} results")
        for i, result in enumerate(search_results[:3]):
            logger.info(f"   Result {i+1}: ID={result.id}, Score={result.score:.4f}")
        
        # 6. Clean up
        logger.info("🔍 Cleaning up test collection...")
        deleted = await client.delete_collection(collection_name)
        if deleted:
            logger.info("✅ Test collection deleted successfully")
        
        logger.info("🎉 gRPC vector insertion test completed successfully!")
        return True
        
    except Exception as e:
        logger.error(f"❌ Test failed with error: {e}")
        import traceback
        traceback.print_exc()
        return False
        
    finally:
        await client.close()

if __name__ == "__main__":
    success = asyncio.run(test_grpc_vector_insertion())
    if success:
        logger.info("✅ All gRPC tests passed!")
        sys.exit(0)
    else:
        logger.error("❌ Some gRPC tests failed!")
        sys.exit(1)
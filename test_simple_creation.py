#!/usr/bin/env python3

import grpc
import logging
import time
import sys

# Add the parent directory to the Python path so we can import the generated protobuf files
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

from proximadb.proximadb_pb2 import (
    CollectionRequest, CollectionConfig,
    CollectionOperation, DistanceMetric, StorageEngine, IndexingAlgorithm
)
from proximadb.proximadb_pb2_grpc import ProximaDBStub

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(levelname)s:%(name)s:%(message)s')
logger = logging.getLogger(__name__)

def test_collection_creation():
    """Simple test for collection creation debugging"""
    logger.info("🧪 Simple Collection Creation Test")
    logger.info("=" * 40)
    
    try:
        # Connect to server
        channel = grpc.insecure_channel('localhost:5679')
        stub = ProximaDBStub(channel)
        logger.info("✅ Connected to ProximaDB gRPC server")
        
        # Create collection config
        config = CollectionConfig(
            name="test_simple_collection",
            dimension=128,
            distance_metric=DistanceMetric.COSINE,
            storage_engine=StorageEngine.VIPER,
            indexing_algorithm=IndexingAlgorithm.HNSW,
            filterable_metadata_fields=[],
            indexing_config={}
        )
        
        request = CollectionRequest(
            operation=CollectionOperation.COLLECTION_CREATE,
            collection_config=config
        )
        
        logger.info("📝 Creating test collection...")
        logger.info(f"   Name: {config.name}")
        logger.info(f"   Dimension: {config.dimension}")
        logger.info(f"   Distance metric: {config.distance_metric}")
        logger.info(f"   Storage engine: {config.storage_engine}")
        logger.info(f"   Indexing algorithm: {config.indexing_algorithm}")
        
        response = stub.CollectionOperation(request)
        
        if response.success:
            logger.info("🎉 ✅ Collection created successfully!")
            logger.info(f"   Processing time: {response.processing_time_us}μs")
            logger.info(f"   Affected count: {response.affected_count}")
            return True
        else:
            logger.error("❌ Collection creation failed!")
            logger.error(f"   Error: {response.error_message}")
            logger.error(f"   Error code: {response.error_code}")
            return False
            
    except Exception as e:
        logger.error(f"❌ Exception during test: {e}")
        return False
    
    finally:
        if 'channel' in locals():
            channel.close()

if __name__ == "__main__":
    success = test_collection_creation()
    sys.exit(0 if success else 1)
#!/usr/bin/env python3
"""
Simple test to create and list collections to debug persistence issues.
"""

import asyncio
import subprocess
import time
import os
import logging
import grpc
import sys

# Add the client directory to Python path
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

from proximadb import proximadb_pb2, proximadb_pb2_grpc

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

async def test_basic_collection():
    """Test basic collection creation and listing"""
    logger.info("🧪 Starting Simple Collection Test")
    
    # Cleanup
    data_dirs = ["/home/vsingh/code/proximadb/data", "/home/vsingh/code/proximadb/certs/data"]
    for data_dir in data_dirs:
        if os.path.exists(data_dir):
            subprocess.run(["rm", "-rf", data_dir], check=False)
            logger.info(f"🧹 Cleaned up: {data_dir}")
    
    # Start server
    os.chdir("/home/vsingh/code/proximadb")
    cmd = ["cargo", "run", "--bin", "proximadb-server"]
    
    server_process = subprocess.Popen(cmd)
    
    try:
        # Wait for server startup
        logger.info("⏳ Waiting for server to start...")
        await asyncio.sleep(12)
        
        # Connect client
        channel = grpc.aio.insecure_channel("localhost:5679")
        stub = proximadb_pb2_grpc.ProximaDBStub(channel)
        
        # Test health check first
        try:
            health_request = proximadb_pb2.HealthRequest()
            health_response = await stub.Health(health_request)
            logger.info(f"✅ Health check successful: {health_response.status}")
        except Exception as e:
            logger.error(f"❌ Health check failed: {e}")
            return False
        
        # Create a test collection
        logger.info("📦 Creating test collection...")
        collection_config = proximadb_pb2.CollectionConfig(
            name="simple_test_collection",
            dimension=128,
            distance_metric=1,  # Cosine
            storage_engine=1,   # Viper
            indexing_algorithm=1,  # HNSW
            filterable_metadata_fields=["category"],
            indexing_config={}
        )
        
        create_request = proximadb_pb2.CollectionRequest(
            operation=1,  # CREATE
            collection_config=collection_config
        )
        
        create_response = await stub.CollectionOperation(create_request)
        
        if create_response.success:
            logger.info("✅ Collection created successfully")
        else:
            logger.error(f"❌ Failed to create collection: {create_response.error_message}")
            return False
        
        # List collections
        logger.info("📋 Listing collections...")
        list_request = proximadb_pb2.CollectionRequest(
            operation=4  # LIST
        )
        
        list_response = await stub.CollectionOperation(list_request)
        
        if list_response.success:
            logger.info(f"✅ List operation successful, found {len(list_response.collections)} collections")
            for i, collection in enumerate(list_response.collections):
                logger.info(f"  Collection {i+1}: {collection.config.name} (dim={collection.config.dimension})")
        else:
            logger.error(f"❌ Failed to list collections: {list_response.error_message}")
            return False
        
        await channel.close()
        
        # Success if we found the collection we created
        return len(list_response.collections) == 1
        
    except Exception as e:
        logger.error(f"❌ Test failed: {e}")
        return False
    finally:
        server_process.terminate()
        server_process.wait()
        logger.info("✅ Server stopped")

async def main():
    success = await test_basic_collection()
    if success:
        print("✅ Simple collection test PASSED!")
    else:
        print("❌ Simple collection test FAILED!")

if __name__ == "__main__":
    asyncio.run(main())
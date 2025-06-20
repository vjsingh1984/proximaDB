#!/usr/bin/env python3
"""
Simple collection test with longer server startup time
"""

import asyncio
import subprocess
import sys
import os
import grpc
import logging

# Add the client directory to Python path
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

import proximadb.proximadb_pb2 as proximadb_pb2
import proximadb.proximadb_pb2_grpc as proximadb_pb2_grpc

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

async def test_simple_collections():
    logger.info("🚀 Starting ProximaDB server...")
    
    # Cleanup old data
    data_dirs = ["/home/vsingh/code/proximadb/data", "/home/vsingh/code/proximadb/certs/data"]
    for data_dir in data_dirs:
        if os.path.exists(data_dir):
            subprocess.run(["rm", "-rf", data_dir], check=False)
            logger.info(f"🧹 Cleaned up: {data_dir}")
    
    # Start server
    os.chdir("/home/vsingh/code/proximadb")
    cmd = ["cargo", "run", "--bin", "proximadb-server"]
    server_process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    
    try:
        # Wait longer for server startup
        logger.info("⏳ Waiting 25 seconds for server to start...")
        await asyncio.sleep(25)
        
        # Connect client
        channel = grpc.aio.insecure_channel("localhost:5679")
        stub = proximadb_pb2_grpc.ProximaDBStub(channel)
        
        # Health check
        health_request = proximadb_pb2.HealthRequest()
        health_response = await stub.Health(health_request)
        logger.info(f"✅ Health check: {health_response.status}")
        
        # Test 1: Create collection with Viper storage
        logger.info("📦 Creating collection with Viper storage...")
        collection_config = proximadb_pb2.CollectionConfig(
            name="test_viper_collection",
            dimension=128,
            distance_metric=1,  # Cosine
            storage_engine=1,   # Viper
            indexing_algorithm=4,  # Flat
            filterable_metadata_fields=["category", "source"],
            indexing_config={}
        )
        
        create_request = proximadb_pb2.CollectionRequest(
            operation=1,  # CREATE
            collection_config=collection_config
        )
        
        response = await stub.CollectionOperation(create_request)
        
        if response.success:
            logger.info("✅ Viper collection created successfully")
        else:
            logger.error(f"❌ Failed to create Viper collection: {response.error_message}")
            await channel.close()
            return False
        
        # Test 2: Create collection with Standard storage
        logger.info("📦 Creating collection with Standard storage...")
        collection_config = proximadb_pb2.CollectionConfig(
            name="test_standard_collection",
            dimension=128,
            distance_metric=1,  # Cosine
            storage_engine=2,   # Standard
            indexing_algorithm=4,  # Flat
            filterable_metadata_fields=["category", "source"],
            indexing_config={}
        )
        
        create_request = proximadb_pb2.CollectionRequest(
            operation=1,  # CREATE
            collection_config=collection_config
        )
        
        response = await stub.CollectionOperation(create_request)
        
        if response.success:
            logger.info("✅ Standard collection created successfully")
        else:
            logger.error(f"❌ Failed to create Standard collection: {response.error_message}")
            await channel.close()
            return False
        
        # Test 3: List collections
        logger.info("📋 Listing collections...")
        list_request = proximadb_pb2.CollectionRequest(operation=2)  # LIST
        response = await stub.CollectionOperation(list_request)
        
        if response.success:
            collection_names = [col.name for col in response.collections]
            logger.info(f"✅ Listed {len(collection_names)} collections: {collection_names}")
        else:
            logger.error(f"❌ Failed to list collections: {response.error_message}")
            await channel.close()
            return False
        
        await channel.close()
        
        logger.info("✅ All simple collection tests passed!")
        return True
        
    except Exception as e:
        logger.error(f"❌ Test failed: {e}")
        return False
    finally:
        server_process.terminate()
        server_process.wait()
        logger.info("✅ Server stopped")

async def main():
    success = await test_simple_collections()
    if success:
        print("✅ Simple collection tests completed successfully!")
    else:
        print("❌ Simple collection tests failed!")
    return success

if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)
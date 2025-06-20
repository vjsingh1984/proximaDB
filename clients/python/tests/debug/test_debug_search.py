#!/usr/bin/env python3
"""
Debug test to understand the vector insertion and search flow.
"""

import asyncio
import json
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

async def debug_search_flow():
    """Debug the complete search flow"""
    logger.info("🧪 Starting Debug Search Flow Test")
    
    # Cleanup
    data_dirs = ["/home/vsingh/code/proximadb/data", "/home/vsingh/code/proximadb/certs/data"]
    for data_dir in data_dirs:
        if os.path.exists(data_dir):
            subprocess.run(["rm", "-rf", data_dir], check=False)
            logger.info(f"🧹 Cleaned up: {data_dir}")
    
    collection_name = "debug_search_test"
    
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
        
        # Health check
        health_request = proximadb_pb2.HealthRequest()
        health_response = await stub.Health(health_request)
        logger.info(f"✅ Health check: {health_response.status}")
        
        # Create collection
        logger.info(f"📦 Creating collection: {collection_name}")
        collection_config = proximadb_pb2.CollectionConfig(
            name=collection_name,
            dimension=3,  # Use small dimension for easy debugging
            distance_metric=1,  # Cosine
            storage_engine=1,   # Viper
            indexing_algorithm=4,  # Flat (no indexing for simplicity)
            filterable_metadata_fields=["type"],
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
        
        # Insert a single test vector with simple values
        logger.info("📝 Inserting single test vector...")
        vector_data = {
            "id": "debug_vector_001",
            "vector": [1.0, 0.0, 0.0],  # Simple unit vector
            "metadata": {
                "type": "debug",
                "test": True
            },
            "timestamp": int(time.time()),
            "version": 1
        }
        
        # Create insert request
        insert_data = {
            "collection_id": collection_name,
            "vectors": [vector_data],
            "upsert_mode": False
        }
        
        json_payload = json.dumps(insert_data).encode('utf-8')
        schema_version = (1).to_bytes(4, 'little')
        operation = b"vector_insert"
        op_len = len(operation).to_bytes(4, 'little')
        avro_payload = schema_version + op_len + operation + json_payload
        
        request = proximadb_pb2.VectorInsertRequest(
            collection_id=collection_name,
            avro_payload=avro_payload
        )
        
        response = await stub.VectorInsert(request)
        if response.success:
            logger.info(f"✅ Inserted vector successfully")
        else:
            logger.error(f"❌ Failed to insert vector: {response.error_message}")
            return False
        
        # Wait longer for background processing to complete
        await asyncio.sleep(5)
        
        # Test search with exact same vector
        logger.info("🔍 Testing search with exact same vector...")
        query_vector = [1.0, 0.0, 0.0]  # Same as inserted vector
        search_query = proximadb_pb2.SearchQuery(vector=query_vector)
        search_request = proximadb_pb2.VectorSearchRequest(
            collection_id=collection_name,
            queries=[search_query],
            top_k=5,
            distance_metric_override=1,  # Cosine
            include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
        )
        
        search_response = await stub.VectorSearch(search_request)
        if search_response.success:
            if hasattr(search_response.result_payload, 'compact_results') and search_response.result_payload.compact_results:
                results_count = len(search_response.result_payload.compact_results.results)
                logger.info(f"✅ Search successful. Found {results_count} results")
                
                if results_count > 0:
                    for i, result in enumerate(search_response.result_payload.compact_results.results):
                        logger.info(f"  Result {i+1}: ID={result.id}, Score={result.score:.6f}")
                        logger.info(f"    Vector: {result.vector}")
                        logger.info(f"    Metadata: {result.metadata}")
                else:
                    logger.warning("⚠️ Search found 0 results - this indicates a problem")
            else:
                logger.warning("⚠️ Search response has no compact_results")
        else:
            logger.error(f"❌ Search failed: {search_response.error_message}")
            return False
        
        # Test search with different vector
        logger.info("🔍 Testing search with different vector...")
        query_vector2 = [0.0, 1.0, 0.0]  # Different vector
        search_query2 = proximadb_pb2.SearchQuery(vector=query_vector2)
        search_request2 = proximadb_pb2.VectorSearchRequest(
            collection_id=collection_name,
            queries=[search_query2],
            top_k=5,
            distance_metric_override=1,  # Cosine
            include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
        )
        
        search_response2 = await stub.VectorSearch(search_request2)
        if search_response2.success:
            if hasattr(search_response2.result_payload, 'compact_results') and search_response2.result_payload.compact_results:
                results_count2 = len(search_response2.result_payload.compact_results.results)
                logger.info(f"✅ Second search successful. Found {results_count2} results")
                
                if results_count2 > 0:
                    for i, result in enumerate(search_response2.result_payload.compact_results.results):
                        logger.info(f"  Result {i+1}: ID={result.id}, Score={result.score:.6f}")
                else:
                    logger.warning("⚠️ Second search also found 0 results")
            else:
                logger.warning("⚠️ Second search response has no compact_results")
        else:
            logger.error(f"❌ Second search failed: {search_response2.error_message}")
        
        await channel.close()
        
        # Analysis
        logger.info("📊 Debug Analysis:")
        logger.info(f"  Collection created: ✅")
        logger.info(f"  Vector inserted: ✅")
        logger.info(f"  Search 1 results: {results_count if 'results_count' in locals() else 'ERROR'}")
        logger.info(f"  Search 2 results: {results_count2 if 'results_count2' in locals() else 'ERROR'}")
        
        return True
        
    except Exception as e:
        logger.error(f"❌ Test failed: {e}")
        return False
    finally:
        server_process.terminate()
        server_process.wait()
        logger.info("✅ Server stopped")

async def main():
    success = await debug_search_flow()
    if success:
        print("✅ Debug search flow test completed!")
    else:
        print("❌ Debug search flow test failed!")

if __name__ == "__main__":
    asyncio.run(main())
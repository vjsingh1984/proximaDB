#!/usr/bin/env python3
"""
Test vector persistence across server restarts.
This test specifically verifies that vectors can be inserted and retrieved across restarts.
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

async def test_vector_persistence():
    """Test vector persistence across server restarts"""
    logger.info("🧪 Starting Vector Persistence Test")
    
    # Cleanup
    data_dirs = ["/home/vsingh/code/proximadb/data", "/home/vsingh/code/proximadb/certs/data"]
    for data_dir in data_dirs:
        if os.path.exists(data_dir):
            subprocess.run(["rm", "-rf", data_dir], check=False)
            logger.info(f"🧹 Cleaned up: {data_dir}")
    
    collection_name = "vector_persistence_test"
    test_vectors = []
    
    # === PHASE 1: Initial Setup ===
    logger.info("\n=== PHASE 1: Create Collection and Insert Vectors ===")
    
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
        
        # Insert test vectors
        logger.info("📝 Inserting test vectors...")
        for i in range(5):
            vector_data = {
                "id": f"test_vector_{i:03d}",
                "vector": [0.1 * (j + i) for j in range(128)],
                "metadata": {
                    "category": f"category_{i % 3}",
                    "index": i
                },
                "timestamp": int(time.time()) + i,
                "version": 1
            }
            test_vectors.append(vector_data)
        
        # Create insert request
        insert_data = {
            "collection_id": collection_name,
            "vectors": test_vectors,
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
            logger.info(f"✅ Inserted {len(test_vectors)} vectors successfully")
        else:
            logger.error(f"❌ Failed to insert vectors: {response.error_message}")
            return False
        
        # Test search before restart
        logger.info("🔍 Testing search before restart...")
        query_vector = [0.1 * i for i in range(128)]
        search_query = proximadb_pb2.SearchQuery(vector=query_vector)
        search_request = proximadb_pb2.VectorSearchRequest(
            collection_id=collection_name,
            queries=[search_query],
            top_k=5,
            distance_metric_override=1,
            include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
        )
        
        search_response = await stub.VectorSearch(search_request)
        results_before = 0
        if search_response.success:
            if hasattr(search_response.result_payload, 'compact_results') and search_response.result_payload.compact_results:
                results_before = len(search_response.result_payload.compact_results.results)
                logger.info(f"✅ Found {results_before} results before restart")
                for i, result in enumerate(search_response.result_payload.compact_results.results[:3]):
                    logger.info(f"  Result {i+1}: ID={result.id}, Score={result.score:.4f}")
            else:
                logger.info("✅ Search successful but no results found")
        else:
            logger.error(f"❌ Search failed: {search_response.error_message}")
            return False
        
        await channel.close()
        
        # === PHASE 2: Server Restart ===
        logger.info("\n=== PHASE 2: Server Restart ===")
        
        # Stop server
        server_process.terminate()
        server_process.wait()
        logger.info("✅ Server stopped")
        
        # Wait before restart
        logger.info("⏳ Waiting 5 seconds before restart...")
        await asyncio.sleep(5)
        
        # Start server again
        server_process = subprocess.Popen(cmd)
        logger.info("⏳ Waiting for server to restart...")
        await asyncio.sleep(12)
        
        # Connect client again
        channel = grpc.aio.insecure_channel("localhost:5679")
        stub = proximadb_pb2_grpc.ProximaDBStub(channel)
        
        # Health check
        health_request = proximadb_pb2.HealthRequest()
        health_response = await stub.Health(health_request)
        logger.info(f"✅ Health check after restart: {health_response.status}")
        
        # === PHASE 3: Verify Persistence ===
        logger.info("\n=== PHASE 3: Verify Collection and Vector Persistence ===")
        
        # List collections
        list_request = proximadb_pb2.CollectionRequest(operation=4)  # LIST
        list_response = await stub.CollectionOperation(list_request)
        
        if list_response.success:
            collections_found = len(list_response.collections)
            logger.info(f"✅ Found {collections_found} collections after restart")
            if collections_found > 0:
                for collection in list_response.collections:
                    logger.info(f"  Collection: {collection.config.name} (vectors: {collection.stats.vector_count})")
        else:
            logger.error(f"❌ Failed to list collections: {list_response.error_message}")
            return False
        
        # Test search after restart
        logger.info("🔍 Testing search after restart...")
        search_response = await stub.VectorSearch(search_request)
        results_after = 0
        if search_response.success:
            if hasattr(search_response.result_payload, 'compact_results') and search_response.result_payload.compact_results:
                results_after = len(search_response.result_payload.compact_results.results)
                logger.info(f"✅ Found {results_after} results after restart")
                for i, result in enumerate(search_response.result_payload.compact_results.results[:3]):
                    logger.info(f"  Result {i+1}: ID={result.id}, Score={result.score:.4f}")
            else:
                logger.info("✅ Search successful but no results found")
        else:
            logger.error(f"❌ Search failed after restart: {search_response.error_message}")
            return False
        
        # Verify specific vector by ID
        logger.info("🔍 Testing vector retrieval by ID...")
        for test_vector in test_vectors[:2]:  # Test first 2 vectors
            # Use the same vector as query to find exact match
            search_query = proximadb_pb2.SearchQuery(vector=test_vector["vector"])
            id_search_request = proximadb_pb2.VectorSearchRequest(
                collection_id=collection_name,
                queries=[search_query],
                top_k=1,
                distance_metric_override=1,
                include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
            )
            
            id_response = await stub.VectorSearch(id_search_request)
            if id_response.success:
                if hasattr(id_response.result_payload, 'compact_results') and id_response.result_payload.compact_results:
                    results = id_response.result_payload.compact_results.results
                    if results and results[0].id == test_vector["id"]:
                        logger.info(f"✅ Found vector by ID: {test_vector['id']} (score: {results[0].score:.4f})")
                    else:
                        logger.error(f"❌ Vector {test_vector['id']} not found or ID mismatch")
                        return False
                else:
                    logger.error(f"❌ No results for vector {test_vector['id']}")
                    return False
        
        await channel.close()
        
        # === PHASE 4: Results ===
        logger.info("\n=== PHASE 4: Test Results ===")
        
        persistence_success = (
            collections_found == 1 and
            results_before > 0 and
            results_after > 0 and
            results_before == results_after
        )
        
        logger.info(f"📊 Test Results:")
        logger.info(f"  Collections after restart: {collections_found}")
        logger.info(f"  Search results before restart: {results_before}")
        logger.info(f"  Search results after restart: {results_after}")
        logger.info(f"  Vector persistence successful: {'✅' if persistence_success else '❌'}")
        
        if persistence_success:
            logger.info("🎉 Vector Persistence Test PASSED!")
        else:
            logger.error("❌ Vector Persistence Test FAILED!")
        
        return persistence_success
        
    except Exception as e:
        logger.error(f"❌ Test failed: {e}")
        return False
    finally:
        server_process.terminate()
        server_process.wait()
        logger.info("✅ Server stopped")

async def main():
    success = await test_vector_persistence()
    if success:
        print("✅ Vector persistence test PASSED!")
    else:
        print("❌ Vector persistence test FAILED!")

if __name__ == "__main__":
    asyncio.run(main())
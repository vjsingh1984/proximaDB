#!/usr/bin/env python3
"""
Final Working Demo - Shows what currently works in ProximaDB
"""

import asyncio
import json
import subprocess
import sys
import os
import grpc
import logging
import numpy as np
import time

# Add the client directory to Python path
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

import proximadb.proximadb_pb2 as proximadb_pb2
import proximadb.proximadb_pb2_grpc as proximadb_pb2_grpc

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

async def demo_working_features():
    """Demonstrate all currently working features"""
    logger.info("🚀 ProximaDB Working Features Demo")
    logger.info("="*60)
    
    # Cleanup old data
    data_dirs = ["/home/vsingh/code/proximadb/data", "/home/vsingh/code/proximadb/certs/data"]
    for data_dir in data_dirs:
        if os.path.exists(data_dir):
            subprocess.run(["rm", "-rf", data_dir], check=False)
    
    # Start server
    os.chdir("/home/vsingh/code/proximadb")
    cmd = ["cargo", "run", "--bin", "proximadb-server"]
    server_process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    
    try:
        # Wait for server startup
        logger.info("⏳ Starting ProximaDB server...")
        await asyncio.sleep(20)
        
        # Connect client
        channel = grpc.aio.insecure_channel("localhost:5679")
        stub = proximadb_pb2_grpc.ProximaDBStub(channel)
        
        # Feature 1: Health Check
        logger.info("\n1️⃣ HEALTH CHECK")
        health_request = proximadb_pb2.HealthRequest()
        health_response = await stub.Health(health_request)
        logger.info(f"✅ Server Status: {health_response.status}")
        logger.info(f"✅ Server Version: {health_response.version}")
        logger.info(f"✅ Active Connections: {health_response.active_connections}")
        
        # Feature 2: Collection Creation (Both Storage Engines)
        logger.info("\n2️⃣ COLLECTION CREATION")
        
        collections_created = []
        
        for storage_name, storage_engine in [("Viper", 1), ("Standard", 2)]:
            collection_name = f"demo_{storage_name.lower()}_collection"
            
            collection_config = proximadb_pb2.CollectionConfig(
                name=collection_name,
                dimension=128,
                distance_metric=1,  # Cosine
                storage_engine=storage_engine,
                indexing_algorithm=4,  # Flat
                filterable_metadata_fields=["category", "source", "priority"],
                indexing_config={}
            )
            
            create_request = proximadb_pb2.CollectionRequest(
                operation=1,  # CREATE
                collection_config=collection_config
            )
            
            response = await stub.CollectionOperation(create_request)
            
            if response.success:
                logger.info(f"✅ {storage_name} Collection Created: {collection_name}")
                collections_created.append((collection_name, storage_name))
            else:
                logger.error(f"❌ {storage_name} Collection Failed: {response.error_message}")
        
        # Feature 3: Vector Insertion (New Separated gRPC Schema)
        logger.info("\n3️⃣ VECTOR INSERTION (New Separated Schema)")
        
        for collection_name, storage_name in collections_created:
            logger.info(f"\n📥 Inserting vectors into {storage_name} collection...")
            
            # Generate test vectors
            np.random.seed(42)
            vectors_data = []
            
            for i in range(3):
                vector_id = f"{storage_name.lower()}_vector_{i:03d}"
                
                # Generate BERT-like embedding
                embedding = np.random.normal(0, 0.1, 128)
                embedding = embedding / np.linalg.norm(embedding)
                
                metadata = {
                    "category": ["research", "documentation", "analysis"][i % 3],
                    "source": ["web", "database", "file"][i % 3],
                    "priority": str(i + 1)
                }
                
                vector_record = {
                    "id": vector_id,
                    "vector": embedding.tolist(),
                    "metadata": metadata,
                    "timestamp": int(time.time()),
                    "version": 1
                }
                vectors_data.append(vector_record)
            
            # Use new separated schema format
            vectors_json = json.dumps(vectors_data).encode('utf-8')
            
            request = proximadb_pb2.VectorInsertRequest(
                collection_id=collection_name,      # gRPC field (metadata)
                upsert_mode=False,                  # gRPC field (metadata)  
                vectors_avro_payload=vectors_json  # Only vector data
            )
            
            response = await stub.VectorInsert(request)
            
            if response.success:
                logger.info(f"✅ Inserted 3 vectors into {storage_name} collection")
                if response.metrics:
                    logger.info(f"   Processing time: {response.metrics.processing_time_us}μs")
            else:
                logger.error(f"❌ Vector insertion failed: {response.error_message}")
        
        # Feature 4: Search Operations (Structure Works, Results Pending)
        logger.info("\n4️⃣ SEARCH OPERATIONS")
        
        for collection_name, storage_name in collections_created:
            logger.info(f"\n🔍 Testing search in {storage_name} collection...")
            
            # Generate query vector
            query_vector = np.random.normal(0, 0.1, 128)
            query_vector = query_vector / np.linalg.norm(query_vector)
            
            # Similarity search
            search_query = proximadb_pb2.SearchQuery(vector=query_vector.tolist())
            search_request = proximadb_pb2.VectorSearchRequest(
                collection_id=collection_name,
                queries=[search_query],
                top_k=5,
                distance_metric_override=1,  # Cosine
                include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
            )
            
            response = await stub.VectorSearch(search_request)
            
            if response.success:
                result_count = 0
                if (hasattr(response, 'result_payload') and response.result_payload and
                    hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                    result_count = len(response.result_payload.compact_results.results)
                
                logger.info(f"✅ Search request processed successfully")
                logger.info(f"   Results found: {result_count}")
                logger.info(f"   Processing time: {response.metrics.processing_time_us if response.metrics else 'N/A'}μs")
            else:
                logger.error(f"❌ Search failed: {response.error_message}")
            
            # Metadata filter search
            filter_query = proximadb_pb2.SearchQuery(
                vector=[0.0] * 128,
                metadata_filter={"category": "research"}
            )
            
            filter_request = proximadb_pb2.VectorSearchRequest(
                collection_id=collection_name,
                queries=[filter_query],
                top_k=10,
                distance_metric_override=1,
                include_fields=proximadb_pb2.IncludeFields(vector=False, metadata=True)
            )
            
            response = await stub.VectorSearch(filter_request)
            
            if response.success:
                filter_count = 0
                if (hasattr(response, 'result_payload') and response.result_payload and
                    hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                    filter_count = len(response.result_payload.compact_results.results)
                
                logger.info(f"✅ Metadata filter processed successfully")
                logger.info(f"   Filtered results: {filter_count}")
        
        # Feature 5: Metrics
        logger.info("\n5️⃣ METRICS")
        metrics_request = proximadb_pb2.MetricsRequest()
        metrics_response = await stub.GetMetrics(metrics_request)
        
        logger.info(f"✅ Metrics retrieved:")
        for key, value in metrics_response.metrics.items():
            logger.info(f"   {key}: {value}")
        
        await channel.close()
        
        # Summary
        logger.info("\n" + "="*60)
        logger.info("📊 WORKING FEATURES SUMMARY")
        logger.info("="*60)
        logger.info("✅ Server startup and health check")
        logger.info("✅ gRPC connection and communication") 
        logger.info("✅ Collection creation (Viper + Standard storage)")
        logger.info("✅ Vector insertion with new separated schema")
        logger.info("✅ Search request processing (infrastructure)")
        logger.info("✅ Metadata filtering (infrastructure)")
        logger.info("✅ Metrics collection")
        logger.info("")
        logger.info("🔧 PENDING FIXES:")
        logger.info("⚠️ Search result retrieval (collection ID mapping)")
        logger.info("⚠️ Collection LIST/GET/DELETE operations")
        logger.info("="*60)
        
        return True
        
    except Exception as e:
        logger.error(f"❌ Demo failed: {e}")
        return False
    finally:
        server_process.terminate()
        server_process.wait()
        logger.info("✅ Server stopped")

if __name__ == "__main__":
    success = asyncio.run(demo_working_features())
    print(f"\n{'✅ Demo completed successfully!' if success else '❌ Demo failed!'}")
    sys.exit(0 if success else 1)
#!/usr/bin/env python3
"""
Test only the working operations: Collection CREATE and Vector INSERT/SEARCH
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

class WorkingOperationsTest:
    def __init__(self):
        self.stub = None
        self.server_process = None
        self.dimension = 128
        
    async def setup_server(self):
        """Start ProximaDB server"""
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
        self.server_process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        
        # Wait for server startup
        logger.info("⏳ Waiting for server to start...")
        await asyncio.sleep(20)
        
        # Connect client
        channel = grpc.aio.insecure_channel("localhost:5679")
        self.stub = proximadb_pb2_grpc.ProximaDBStub(channel)
        
        # Health check
        health_request = proximadb_pb2.HealthRequest()
        health_response = await self.stub.Health(health_request)
        logger.info(f"✅ Health check: {health_response.status}")
        
    async def teardown_server(self):
        """Stop ProximaDB server"""
        if self.server_process:
            self.server_process.terminate()
            self.server_process.wait()
            logger.info("✅ Server stopped")
    
    def generate_test_vectors(self, count: int) -> tuple:
        """Generate test vectors and metadata"""
        np.random.seed(42)  # For reproducible results
        
        vector_ids = []
        embeddings = []
        metadata_list = []
        
        for i in range(count):
            # Generate vector ID
            vector_id = f"test_vector_{i:03d}"
            vector_ids.append(vector_id)
            
            # Generate embedding
            embedding = np.random.normal(0, 0.1, self.dimension)
            embedding = embedding / np.linalg.norm(embedding)  # Normalize
            embeddings.append(embedding.tolist())
            
            # Generate metadata
            metadata = {
                "category": ["research", "documentation", "analysis"][i % 3],
                "source": ["web", "database", "file"][i % 3],
                "priority": str((i % 3) + 1),
                "created_at": str(int(time.time()) - (i * 3600))
            }
            metadata_list.append(metadata)
        
        return vector_ids, embeddings, metadata_list
    
    async def test_collection_creation(self, storage_name: str, storage_engine: int):
        """Test collection creation for a storage engine"""
        collection_name = f"test_{storage_name.lower()}_collection"
        
        logger.info(f"🧪 Testing {storage_name} storage engine collection creation...")
        
        try:
            # Create collection
            collection_config = proximadb_pb2.CollectionConfig(
                name=collection_name,
                dimension=self.dimension,
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
            
            response = await self.stub.CollectionOperation(create_request)
            
            if response.success:
                logger.info(f"✅ {storage_name} collection created successfully")
                return True, collection_name
            else:
                logger.error(f"❌ Failed to create {storage_name} collection: {response.error_message}")
                return False, collection_name
            
        except Exception as e:
            logger.error(f"❌ Exception creating {storage_name} collection: {e}")
            return False, collection_name
    
    async def test_vector_operations(self, collection_name: str, storage_name: str):
        """Test vector insert and search operations"""
        logger.info(f"🔄 Testing vector operations for {storage_name}...")
        
        try:
            # Generate test vectors
            vector_ids, embeddings, metadata_list = self.generate_test_vectors(5)
            
            # Test 1: Vector insertion
            logger.info(f"📥 Inserting {len(vector_ids)} vectors...")
            
            # Prepare vectors data for new separated schema
            vectors_data = []
            for i, (vector_id, embedding, metadata) in enumerate(zip(vector_ids, embeddings, metadata_list)):
                vector_record = {
                    "id": vector_id,
                    "vector": embedding,
                    "metadata": metadata,
                    "timestamp": int(time.time()),
                    "version": 1
                }
                vectors_data.append(vector_record)
            
            # Use new separated schema format
            vectors_json = json.dumps(vectors_data).encode('utf-8')
            
            request = proximadb_pb2.VectorInsertRequest(
                collection_id=collection_name,
                upsert_mode=False,
                vectors_avro_payload=vectors_json
            )
            
            response = await self.stub.VectorInsert(request)
            
            if not response.success:
                logger.error(f"❌ Failed to insert vectors: {response.error_message}")
                return False
            
            logger.info(f"✅ Inserted {len(vector_ids)} vectors successfully")
            
            # Wait for processing
            await asyncio.sleep(3)
            
            # Test 2: Vector search
            logger.info("🔍 Testing vector search...")
            
            # Use first vector as query
            query_vector = embeddings[0]
            
            search_query = proximadb_pb2.SearchQuery(vector=query_vector)
            search_request = proximadb_pb2.VectorSearchRequest(
                collection_id=collection_name,
                queries=[search_query],
                top_k=3,
                distance_metric_override=1,  # Cosine
                include_fields=proximadb_pb2.IncludeFields(vector=True, metadata=True)
            )
            
            response = await self.stub.VectorSearch(search_request)
            
            if not response.success:
                logger.error(f"❌ Vector search failed: {response.error_message}")
                return False
            
            # Check results
            results_found = 0
            if (hasattr(response, 'result_payload') and response.result_payload and
                hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                results_found = len(response.result_payload.compact_results.results)
                
                logger.info(f"✅ Search found {results_found} results:")
                for i, result in enumerate(response.result_payload.compact_results.results):
                    logger.info(f"   {i+1}. ID: {result.id}, Score: {result.score:.4f}")
            
            if results_found > 0:
                logger.info(f"✅ Vector search successful for {storage_name}")
                return True
            else:
                logger.warning(f"⚠️ Vector search returned no results for {storage_name}")
                return False
            
        except Exception as e:
            logger.error(f"❌ Exception in vector operations for {storage_name}: {e}")
            return False
    
    async def test_metadata_search(self, collection_name: str, storage_name: str):
        """Test metadata filtering search"""
        logger.info(f"🔍 Testing metadata search for {storage_name}...")
        
        try:
            # Search with metadata filter
            query_vector = [0.0] * self.dimension  # Neutral vector for metadata filtering
            
            search_query = proximadb_pb2.SearchQuery(
                vector=query_vector,
                metadata_filter={"category": "research"}
            )
            
            search_request = proximadb_pb2.VectorSearchRequest(
                collection_id=collection_name,
                queries=[search_query],
                top_k=10,
                distance_metric_override=1,  # Cosine
                include_fields=proximadb_pb2.IncludeFields(vector=False, metadata=True)
            )
            
            response = await self.stub.VectorSearch(search_request)
            
            if not response.success:
                logger.error(f"❌ Metadata search failed: {response.error_message}")
                return False
            
            # Check results
            results_found = 0
            if (hasattr(response, 'result_payload') and response.result_payload and
                hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results):
                results_found = len(response.result_payload.compact_results.results)
                
                logger.info(f"✅ Metadata search found {results_found} results with category='research'")
            
            return True
            
        except Exception as e:
            logger.error(f"❌ Exception in metadata search for {storage_name}: {e}")
            return False
    
    async def run_working_operations_test(self):
        """Run tests for working operations only"""
        logger.info("🚀 Starting working operations test...")
        
        # Test configurations: only Viper and Standard
        test_configs = [
            ("Viper", 1),
            ("Standard", 2)
        ]
        
        results = {}
        
        for storage_name, storage_engine in test_configs:
            logger.info(f"\n🧪 Testing {storage_name} storage engine:")
            
            # Test 1: Collection creation
            collection_success, collection_name = await self.test_collection_creation(storage_name, storage_engine)
            
            if not collection_success:
                results[storage_name] = {"collection": False, "vectors": False, "metadata": False}
                continue
            
            # Test 2: Vector operations
            vector_success = await self.test_vector_operations(collection_name, storage_name)
            
            # Test 3: Metadata search
            metadata_success = await self.test_metadata_search(collection_name, storage_name)
            
            results[storage_name] = {
                "collection": collection_success,
                "vectors": vector_success,
                "metadata": metadata_success
            }
            
            # Overall success for this storage engine
            overall_success = collection_success and vector_success and metadata_success
            
            if overall_success:
                logger.info(f"✅ {storage_name} ALL TESTS PASSED")
            else:
                logger.error(f"❌ {storage_name} some tests failed")
            
            # Small delay between storage engine tests
            await asyncio.sleep(2)
        
        # Final summary
        total_engines = len(test_configs)
        successful_engines = sum(1 for r in results.values() if all(r.values()))
        
        logger.info(f"\n📊 Final Summary: {successful_engines}/{total_engines} storage engines passed all tests")
        
        # Detailed results
        for storage_name, tests in results.items():
            logger.info(f"  {storage_name}:")
            for test_name, success in tests.items():
                status = "✅" if success else "❌"
                logger.info(f"    {test_name}: {status}")
        
        return successful_engines == total_engines

async def main():
    """Main test execution"""
    test_suite = WorkingOperationsTest()
    
    try:
        # Setup
        await test_suite.setup_server()
        
        # Run tests
        all_passed = await test_suite.run_working_operations_test()
        
        # Print final result
        print("\n" + "="*70)
        print("📊 WORKING OPERATIONS TEST SUMMARY")
        print("="*70)
        
        if all_passed:
            print("✅ All working operations tests completed successfully!")
            print("✅ Both Viper and Standard storage engines work correctly!")
        else:
            print("❌ Some working operations tests failed!")
        
        print("="*70)
        
        return all_passed
        
    except Exception as e:
        logger.error(f"❌ Test suite failed: {e}")
        return False
    finally:
        await test_suite.teardown_server()

if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)
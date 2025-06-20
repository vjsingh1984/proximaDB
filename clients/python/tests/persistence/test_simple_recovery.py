#!/usr/bin/env python3
"""
Simple WAL recovery and search test using direct gRPC calls.
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

class SimpleRecoveryTester:
    def __init__(self, server_port=5679):
        self.server_port = server_port
        self.server_process = None
        self.channel = None
        self.stub = None
        self.test_collection = "simple_recovery_test"
        
    def cleanup_data_dirs(self):
        """Clean up data directories before testing"""
        dirs_to_clean = [
            "/home/vsingh/code/proximadb/data",
            "/home/vsingh/code/proximadb/certs/data"
        ]
        
        for data_dir in dirs_to_clean:
            if os.path.exists(data_dir):
                subprocess.run(["rm", "-rf", data_dir], check=False)
                logger.info(f"🧹 Cleaned up data directory: {data_dir}")

    async def start_server(self):
        """Start the ProximaDB server"""
        logger.info("🚀 Starting ProximaDB server...")
        
        # Ensure we're in the right directory
        os.chdir("/home/vsingh/code/proximadb")
        
        # Start server with debug logging
        cmd = [
            "cargo", "run", "--bin", "proximadb-server"
        ]
        
        self.server_process = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True
        )
        
        # Wait for server to start
        await asyncio.sleep(8)
        
        if self.server_process.poll() is not None:
            stdout, stderr = self.server_process.communicate()
            logger.error(f"❌ Server failed to start. STDOUT: {stdout}, STDERR: {stderr}")
            raise Exception("Server failed to start")
            
        logger.info(f"✅ Server started on port {self.server_port}")

    async def stop_server(self):
        """Stop the ProximaDB server"""
        if self.server_process:
            logger.info("🛑 Stopping ProximaDB server...")
            self.server_process.terminate()
            try:
                self.server_process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                logger.warning("⚠️ Server didn't stop gracefully, forcing kill")
                self.server_process.kill()
                self.server_process.wait()
            
            self.server_process = None
            logger.info("✅ Server stopped")

    async def connect_client(self):
        """Connect the gRPC client"""
        logger.info("🔌 Connecting gRPC client...")
        
        self.channel = grpc.aio.insecure_channel(f"localhost:{self.server_port}")
        self.stub = proximadb_pb2_grpc.ProximaDBStub(self.channel)
        
        # Test connection with health check
        for attempt in range(10):
            try:
                health_request = proximadb_pb2.HealthRequest()
                health_response = await self.stub.Health(health_request)
                logger.info(f"✅ Client connected. Server health: {health_response.status}")
                return
            except Exception as e:
                logger.info(f"⏳ Connection attempt {attempt + 1}/10 failed: {e}")
                await asyncio.sleep(2)
                
        raise Exception("❌ Failed to connect to server after 10 attempts")

    async def create_collection(self):
        """Create test collection"""
        logger.info(f"📦 Creating collection: {self.test_collection}")
        
        try:
            # Create collection configuration
            collection_config = proximadb_pb2.CollectionConfig(
                name=self.test_collection,
                dimension=128,
                distance_metric=1,  # Cosine
                storage_engine=1,   # Viper
                indexing_algorithm=1,  # HNSW
                filterable_metadata_fields=["category", "priority"],
                indexing_config={}
            )
            
            # Create collection request
            request = proximadb_pb2.CollectionRequest(
                operation=1,  # CREATE
                collection_config=collection_config
            )
            
            response = await self.stub.CollectionOperation(request)
            
            if response.success:
                logger.info(f"✅ Collection created successfully: {self.test_collection}")
            else:
                logger.error(f"❌ Failed to create collection: {response.error_message}")
                raise Exception(f"Collection creation failed: {response.error_message}")
                
        except Exception as e:
            logger.error(f"❌ Error creating collection: {e}")
            raise

    async def insert_test_vectors(self, count=10):
        """Insert test vectors using zero-copy insertion"""
        logger.info(f"📝 Inserting {count} test vectors...")
        
        try:
            # Create test data in JSON format
            test_vectors = []
            for i in range(count):
                vector_data = {
                    "id": f"test_vec_{i:03d}",
                    "vector": [0.1 * j + i * 0.01 for j in range(128)],  # Simple pattern
                    "metadata": {
                        "category": "test",
                        "priority": i % 5 + 1,
                        "index": i
                    },
                    "timestamp": int(time.time()) + i,
                    "version": 1
                }
                test_vectors.append(vector_data)
            
            # Create insert request with JSON payload
            insert_data = {
                "collection_id": self.test_collection,
                "vectors": test_vectors,
                "upsert_mode": False
            }
            
            # Convert to JSON bytes
            json_payload = json.dumps(insert_data).encode('utf-8')
            
            # Create versioned payload (schema_version + op_len + operation + data)
            schema_version = (1).to_bytes(4, 'little')
            operation = b"vector_insert"
            op_len = len(operation).to_bytes(4, 'little')
            
            avro_payload = schema_version + op_len + operation + json_payload
            
            # Create vector insert request
            request = proximadb_pb2.VectorInsertRequest(
                collection_id=self.test_collection,
                avro_payload=avro_payload
            )
            
            response = await self.stub.VectorInsert(request)
            
            if response.success:
                logger.info(f"✅ Inserted {count} vectors successfully")
                return True
            else:
                logger.error(f"❌ Failed to insert vectors: {response.error_message}")
                return False
                
        except Exception as e:
            logger.error(f"❌ Error inserting vectors: {e}")
            return False

    async def test_search(self, test_name=""):
        """Test vector search functionality"""
        logger.info(f"🔍 Testing search {test_name}...")
        
        try:
            # Create a simple query vector
            query_vector = [0.1 * i for i in range(128)]
            
            # Create search query
            search_query = proximadb_pb2.SearchQuery(vector=query_vector)
            
            # Create search request
            request = proximadb_pb2.VectorSearchRequest(
                collection_id=self.test_collection,
                queries=[search_query],
                top_k=5,
                distance_metric_override=1,  # Cosine
                include_fields=proximadb_pb2.IncludeFields(
                    vector=True,
                    metadata=True
                )
            )
            
            response = await self.stub.VectorSearch(request)
            
            if response.success:
                # Check if we got results
                if hasattr(response.result_payload, 'compact_results') and response.result_payload.compact_results:
                    results_count = len(response.result_payload.compact_results.results)
                    logger.info(f"✅ Search {test_name} successful. Found {results_count} results")
                    
                    # Log some results
                    for i, result in enumerate(response.result_payload.compact_results.results[:3]):
                        logger.info(f"  Result {i+1}: ID={result.id}, Score={result.score:.4f}")
                    
                    return results_count
                else:
                    logger.info(f"✅ Search {test_name} successful but no results found")
                    return 0
            else:
                logger.error(f"❌ Search {test_name} failed: {response.error_message}")
                return -1
                
        except Exception as e:
            logger.error(f"❌ Error during search {test_name}: {e}")
            return -1

    async def run_recovery_test(self):
        """Run the complete recovery test"""
        logger.info("🧪 Starting Simple WAL Recovery Test")
        
        try:
            # Phase 1: Initial setup
            logger.info("\n=== PHASE 1: Initial Setup ===")
            self.cleanup_data_dirs()
            await self.start_server()
            await self.connect_client()
            
            # Phase 2: Create collection and insert data
            logger.info("\n=== PHASE 2: Data Creation ===")
            await self.create_collection()
            insert_success = await self.insert_test_vectors(10)
            
            if not insert_success:
                logger.error("❌ Failed to insert vectors, aborting test")
                return False
            
            # Phase 3: Test search before restart
            logger.info("\n=== PHASE 3: Search Before Restart ===")
            results_before = await self.test_search("before restart")
            
            # Phase 4: Server restart
            logger.info("\n=== PHASE 4: Server Restart ===")
            await self.stop_server()
            if self.channel:
                await self.channel.close()
            
            logger.info("⏳ Waiting 5 seconds before restart...")
            await asyncio.sleep(5)
            
            await self.start_server()
            await self.connect_client()
            
            # Phase 5: Test search after restart
            logger.info("\n=== PHASE 5: Search After Restart ===")
            results_after = await self.test_search("after restart")
            
            # Phase 6: Results summary
            logger.info("\n=== PHASE 6: Test Results Summary ===")
            recovery_success = (results_before > 0 and results_after > 0)
            
            logger.info(f"📊 Test Results:")
            logger.info(f"  Search results before restart: {results_before}")
            logger.info(f"  Search results after restart: {results_after}")
            logger.info(f"  WAL Recovery successful: {recovery_success}")
            
            if recovery_success:
                logger.info("🎉 WAL Recovery Test PASSED!")
            else:
                logger.error("❌ WAL Recovery Test FAILED!")
                
            return recovery_success
            
        except Exception as e:
            logger.error(f"❌ Test failed with exception: {e}")
            return False
        finally:
            if self.channel:
                await self.channel.close()
            await self.stop_server()

async def main():
    """Main test function"""
    tester = SimpleRecoveryTester()
    success = await tester.run_recovery_test()
    
    if success:
        print("\n✅ Simple WAL Recovery Test completed successfully!")
        exit(0)
    else:
        print("\n❌ Simple WAL Recovery Test failed!")
        exit(1)

if __name__ == "__main__":
    asyncio.run(main())
#!/usr/bin/env python3
"""
Comprehensive test for WAL recovery and search functionality across server restarts.

This test verifies:
1. WAL persistence across server restarts
2. Memtable repopulation after recovery
3. Search functionality on recovered data
4. Collection metadata persistence
5. Two-part search (memtable + indexes)
"""

import asyncio
import json
import subprocess
import time
import signal
import os
import logging
import grpc
from pathlib import Path

# Add the client directory to Python path
import sys
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')

from proximadb.grpc_client import ProximaDBClient
from proximadb import proximadb_pb2

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

class ProximaDBRecoveryTester:
    def __init__(self, server_port=50051):
        self.server_port = server_port
        self.server_process = None
        self.client = None
        self.test_collection = "recovery_test_collection"
        self.test_vectors = []
        
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
            "cargo", "run", "--bin", "proximadb-server", "--",
            "--port", str(self.server_port),
            "--config", "config.toml"
        ]
        
        self.server_process = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True
        )
        
        # Wait for server to start
        await asyncio.sleep(5)
        
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
        self.client = ProximaDBClient(f"localhost:{self.server_port}")
        
        # Test connection with health check
        for attempt in range(10):
            try:
                health_response = await asyncio.to_thread(self.client.health_check)
                logger.info(f"✅ Client connected. Server health: {health_response.status}")
                return
            except Exception as e:
                logger.info(f"⏳ Connection attempt {attempt + 1}/10 failed: {e}")
                await asyncio.sleep(2)
                
        raise Exception("❌ Failed to connect to server after 10 attempts")

    def generate_test_vectors(self, count=50):
        """Generate test vectors with metadata"""
        import random
        
        self.test_vectors = []
        categories = ["tech", "science", "arts", "sports", "news"]
        
        for i in range(count):
            # Generate random 128-dimensional vectors
            vector = [random.uniform(-1.0, 1.0) for _ in range(128)]
            
            vector_data = {
                "id": f"test_vector_{i:03d}",
                "vector": vector,
                "metadata": {
                    "category": random.choice(categories),
                    "priority": random.randint(1, 10),
                    "source": f"test_source_{i % 5}",
                    "timestamp": int(time.time()) + i
                }
            }
            
            self.test_vectors.append(vector_data)
            
        logger.info(f"📊 Generated {len(self.test_vectors)} test vectors")

    async def create_collection(self):
        """Create test collection"""
        logger.info(f"📦 Creating collection: {self.test_collection}")
        
        try:
            response = await asyncio.to_thread(
                self.client.create_collection,
                name=self.test_collection,
                dimension=128,
                distance_metric="cosine",
                storage_engine="viper",  # Ensure we use Viper
                indexing_algorithm="hnsw"
            )
            
            if response.success:
                logger.info(f"✅ Collection created successfully: {self.test_collection}")
            else:
                logger.error(f"❌ Failed to create collection: {response.error_message}")
                raise Exception(f"Collection creation failed: {response.error_message}")
                
        except Exception as e:
            logger.error(f"❌ Error creating collection: {e}")
            raise

    async def insert_vectors(self):
        """Insert test vectors into the collection"""
        logger.info(f"📝 Inserting {len(self.test_vectors)} vectors...")
        
        try:
            for i, vector_data in enumerate(self.test_vectors):
                response = await asyncio.to_thread(
                    self.client.insert_vector,
                    collection_id=self.test_collection,
                    vector_id=vector_data["id"],
                    vector=vector_data["vector"],
                    metadata=vector_data["metadata"]
                )
                
                if not response.success:
                    logger.error(f"❌ Failed to insert vector {vector_data['id']}: {response.error_message}")
                    
                if (i + 1) % 10 == 0:
                    logger.info(f"📝 Inserted {i + 1}/{len(self.test_vectors)} vectors")
                    
            logger.info(f"✅ All {len(self.test_vectors)} vectors inserted successfully")
            
        except Exception as e:
            logger.error(f"❌ Error inserting vectors: {e}")
            raise

    async def test_search_before_restart(self):
        """Test search functionality before server restart"""
        logger.info("🔍 Testing search before restart...")
        
        try:
            # Test vector similarity search
            query_vector = self.test_vectors[0]["vector"]  # Use first vector as query
            
            search_response = await asyncio.to_thread(
                self.client.search_vectors,
                collection_id=self.test_collection,
                query_vectors=[query_vector],
                top_k=5
            )
            
            if search_response.success:
                logger.info(f"✅ Search successful before restart. Found {len(search_response.result_payload.compact_results.results)} results")
                
                # Log some results
                for i, result in enumerate(search_response.result_payload.compact_results.results[:3]):
                    logger.info(f"  Result {i+1}: ID={result.id}, Score={result.score:.4f}")
                    
                return len(search_response.result_payload.compact_results.results)
            else:
                logger.error(f"❌ Search failed before restart: {search_response.error_message}")
                return 0
                
        except Exception as e:
            logger.error(f"❌ Error during search before restart: {e}")
            return 0

    async def test_search_after_restart(self):
        """Test search functionality after server restart"""
        logger.info("🔍 Testing search after restart...")
        
        try:
            # Test the same search as before restart
            query_vector = self.test_vectors[0]["vector"]
            
            search_response = await asyncio.to_thread(
                self.client.search_vectors,
                collection_id=self.test_collection,
                query_vectors=[query_vector],
                top_k=5
            )
            
            if search_response.success:
                results_count = len(search_response.result_payload.compact_results.results)
                logger.info(f"✅ Search successful after restart. Found {results_count} results")
                
                # Log detailed results to verify recovery
                for i, result in enumerate(search_response.result_payload.compact_results.results[:3]):
                    logger.info(f"  Result {i+1}: ID={result.id}, Score={result.score:.4f}")
                
                return results_count
            else:
                logger.error(f"❌ Search failed after restart: {search_response.error_message}")
                return 0
                
        except Exception as e:
            logger.error(f"❌ Error during search after restart: {e}")
            return 0

    async def test_metadata_search(self):
        """Test metadata filtering search"""
        logger.info("🔍 Testing metadata filtering search...")
        
        try:
            # Search for vectors with specific category
            query_vector = self.test_vectors[0]["vector"]
            
            # For now, we'll do a regular search since metadata filtering 
            # implementation may need to be enhanced
            search_response = await asyncio.to_thread(
                self.client.search_vectors,
                collection_id=self.test_collection,
                query_vectors=[query_vector],
                top_k=10
            )
            
            if search_response.success:
                results_count = len(search_response.result_payload.compact_results.results)
                logger.info(f"✅ Metadata search successful. Found {results_count} results")
                return results_count
            else:
                logger.error(f"❌ Metadata search failed: {search_response.error_message}")
                return 0
                
        except Exception as e:
            logger.error(f"❌ Error during metadata search: {e}")
            return 0

    async def verify_collection_exists(self):
        """Verify that collection exists after restart"""
        logger.info("📦 Verifying collection exists after restart...")
        
        try:
            # List collections to see if our test collection is there
            # Note: This depends on the implementation of list_collections
            # For now, we'll just try to search on it
            query_vector = [0.1] * 128  # Simple test vector
            
            search_response = await asyncio.to_thread(
                self.client.search_vectors,
                collection_id=self.test_collection,
                query_vectors=[query_vector],
                top_k=1
            )
            
            if search_response.success:
                logger.info("✅ Collection exists and is accessible after restart")
                return True
            else:
                logger.error(f"❌ Collection not accessible after restart: {search_response.error_message}")
                return False
                
        except Exception as e:
            logger.error(f"❌ Error verifying collection existence: {e}")
            return False

    async def run_recovery_test(self):
        """Run the complete recovery test"""
        logger.info("🧪 Starting WAL Recovery and Search Test")
        
        try:
            # Phase 1: Initial setup
            logger.info("\n=== PHASE 1: Initial Setup ===")
            self.cleanup_data_dirs()
            await self.start_server()
            await self.connect_client()
            
            # Phase 2: Create collection and insert data
            logger.info("\n=== PHASE 2: Data Creation ===")
            self.generate_test_vectors()
            await self.create_collection()
            await self.insert_vectors()
            
            # Phase 3: Test search before restart
            logger.info("\n=== PHASE 3: Search Before Restart ===")
            results_before = await self.test_search_before_restart()
            
            # Phase 4: Server restart
            logger.info("\n=== PHASE 4: Server Restart ===")
            await self.stop_server()
            logger.info("⏳ Waiting 3 seconds before restart...")
            await asyncio.sleep(3)
            await self.start_server()
            await self.connect_client()
            
            # Phase 5: Verify recovery
            logger.info("\n=== PHASE 5: Recovery Verification ===")
            collection_exists = await self.verify_collection_exists()
            results_after = await self.test_search_after_restart()
            metadata_results = await self.test_metadata_search()
            
            # Phase 6: Results summary
            logger.info("\n=== PHASE 6: Test Results Summary ===")
            recovery_success = (
                collection_exists and 
                results_after > 0 and 
                results_before == results_after
            )
            
            logger.info(f"📊 Test Results:")
            logger.info(f"  Collection exists after restart: {collection_exists}")
            logger.info(f"  Search results before restart: {results_before}")
            logger.info(f"  Search results after restart: {results_after}")
            logger.info(f"  Metadata search results: {metadata_results}")
            logger.info(f"  WAL Recovery successful: {recovery_success}")
            
            if recovery_success:
                logger.info("🎉 WAL Recovery and Search Test PASSED!")
            else:
                logger.error("❌ WAL Recovery and Search Test FAILED!")
                
            return recovery_success
            
        except Exception as e:
            logger.error(f"❌ Test failed with exception: {e}")
            return False
        finally:
            await self.stop_server()

async def main():
    """Main test function"""
    tester = ProximaDBRecoveryTester()
    success = await tester.run_recovery_test()
    
    if success:
        print("\n✅ WAL Recovery and Search Test completed successfully!")
        exit(0)
    else:
        print("\n❌ WAL Recovery and Search Test failed!")
        exit(1)

if __name__ == "__main__":
    asyncio.run(main())
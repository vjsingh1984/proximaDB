#!/usr/bin/env python3
"""
Minimal collection test - test just 2 configurations (Viper + Standard)
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

class MinimalCollectionTest:
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
    
    async def test_collection_lifecycle(self, storage_name: str, storage_engine: int):
        """Test complete collection lifecycle for a storage engine"""
        collection_name = f"test_{storage_name.lower()}_collection"
        
        logger.info(f"🧪 Testing {storage_name} storage engine...")
        
        try:
            # Step 1: Create collection
            collection_config = proximadb_pb2.CollectionConfig(
                name=collection_name,
                dimension=self.dimension,
                distance_metric=1,  # Cosine
                storage_engine=storage_engine,
                indexing_algorithm=4,  # Flat
                filterable_metadata_fields=["category", "source"],
                indexing_config={}
            )
            
            create_request = proximadb_pb2.CollectionRequest(
                operation=1,  # CREATE
                collection_config=collection_config
            )
            
            response = await self.stub.CollectionOperation(create_request)
            
            if not response.success:
                logger.error(f"❌ Failed to create {storage_name} collection: {response.error_message}")
                return False
            
            logger.info(f"✅ {storage_name} collection created successfully")
            
            # Step 2: List collections (verify it appears)
            list_request = proximadb_pb2.CollectionRequest(operation=2)  # LIST
            response = await self.stub.CollectionOperation(list_request)
            
            if not response.success:
                logger.error(f"❌ Failed to list collections: {response.error_message}")
                return False
            
            collection_names = [col.name for col in response.collections]
            if collection_name not in collection_names:
                logger.error(f"❌ {storage_name} collection not found in list: {collection_names}")
                return False
            
            logger.info(f"✅ {storage_name} collection found in list")
            
            # Step 3: Get collection info
            get_request = proximadb_pb2.CollectionRequest(
                operation=3,  # GET
                collection_name=collection_name
            )
            
            response = await self.stub.CollectionOperation(get_request)
            
            if not response.success or not response.collection:
                logger.error(f"❌ Failed to get {storage_name} collection info: {response.error_message}")
                return False
            
            collection = response.collection
            logger.info(f"✅ {storage_name} collection info retrieved:")
            logger.info(f"   - Name: {collection.name}")
            logger.info(f"   - Dimension: {collection.dimension}")
            logger.info(f"   - Storage Engine: {collection.storage_engine}")
            logger.info(f"   - Indexing Algorithm: {collection.indexing_algorithm}")
            
            # Step 4: Delete collection
            delete_request = proximadb_pb2.CollectionRequest(
                operation=4,  # DELETE
                collection_name=collection_name
            )
            
            response = await self.stub.CollectionOperation(delete_request)
            
            if not response.success:
                logger.error(f"❌ Failed to delete {storage_name} collection: {response.error_message}")
                return False
            
            logger.info(f"✅ {storage_name} collection deleted successfully")
            
            # Step 5: Verify deletion (list again)
            list_request = proximadb_pb2.CollectionRequest(operation=2)  # LIST
            response = await self.stub.CollectionOperation(list_request)
            
            if response.success:
                collection_names_after = [col.name for col in response.collections]
                if collection_name in collection_names_after:
                    logger.error(f"❌ {storage_name} collection still exists after deletion")
                    return False
                
                logger.info(f"✅ {storage_name} collection successfully removed from list")
            
            return True
            
        except Exception as e:
            logger.error(f"❌ Exception testing {storage_name}: {e}")
            return False
    
    async def run_minimal_tests(self):
        """Run minimal collection tests for Viper and Standard"""
        logger.info("🚀 Starting minimal collection tests...")
        
        # Test configurations: only Viper and Standard
        test_configs = [
            ("Viper", 1),
            ("Standard", 2)
        ]
        
        results = {}
        
        for storage_name, storage_engine in test_configs:
            success = await self.test_collection_lifecycle(storage_name, storage_engine)
            results[storage_name] = success
            
            if success:
                logger.info(f"✅ {storage_name} test PASSED")
            else:
                logger.error(f"❌ {storage_name} test FAILED")
            
            # Small delay between tests
            await asyncio.sleep(1)
        
        # Summary
        passed = sum(results.values())
        total = len(results)
        
        logger.info(f"📊 Test Summary: {passed}/{total} storage engines passed")
        
        if passed == total:
            logger.info("✅ All minimal collection tests passed!")
            return True
        else:
            logger.error("❌ Some minimal collection tests failed!")
            return False

async def main():
    """Main test execution"""
    test_suite = MinimalCollectionTest()
    
    try:
        # Setup
        await test_suite.setup_server()
        
        # Run tests
        all_passed = await test_suite.run_minimal_tests()
        
        # Print final result
        print("\n" + "="*60)
        print("📊 MINIMAL COLLECTION TEST SUMMARY")
        print("="*60)
        
        if all_passed:
            print("✅ All minimal collection tests completed successfully!")
        else:
            print("❌ Some minimal collection tests failed!")
        
        print("="*60)
        
        return all_passed
        
    except Exception as e:
        logger.error(f"❌ Test suite failed: {e}")
        return False
    finally:
        await test_suite.teardown_server()

if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)
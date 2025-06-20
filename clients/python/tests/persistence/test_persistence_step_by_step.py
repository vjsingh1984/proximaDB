#!/usr/bin/env python3

import grpc
import logging
import time
import subprocess
import signal
import os
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

class ProximaDBTester:
    def __init__(self):
        self.channel = None
        self.stub = None
        self.server_process = None
        
    def connect(self):
        """Connect to ProximaDB gRPC server"""
        try:
            self.channel = grpc.insecure_channel('localhost:5679')
            self.stub = ProximaDBStub(self.channel)
            logger.info("✅ Connected to ProximaDB gRPC server")
            return True
        except Exception as e:
            logger.error(f"❌ Failed to connect: {e}")
            return False
    
    def disconnect(self):
        """Disconnect from server"""
        if self.channel:
            self.channel.close()
            self.channel = None
            self.stub = None
            logger.info("🔌 Disconnected from server")
    
    def start_server(self):
        """Start ProximaDB server"""
        logger.info("🚀 Starting ProximaDB server...")
        try:
            # Kill any existing server
            subprocess.run(["pkill", "-f", "proximadb-server"], capture_output=True)
            time.sleep(2)
            
            # Start new server
            self.server_process = subprocess.Popen(
                ["cargo", "run", "--bin", "proximadb-server"],
                cwd="/home/vsingh/code/proximadb",
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True
            )
            
            # Wait for server to start
            time.sleep(5)
            logger.info("✅ Server started")
            return True
        except Exception as e:
            logger.error(f"❌ Failed to start server: {e}")
            return False
    
    def stop_server(self):
        """Stop ProximaDB server"""
        logger.info("🛑 Stopping ProximaDB server...")
        try:
            if self.server_process:
                self.server_process.terminate()
                self.server_process.wait(timeout=10)
            
            # Force kill if needed
            subprocess.run(["pkill", "-f", "proximadb-server"], capture_output=True)
            time.sleep(2)
            logger.info("✅ Server stopped")
        except Exception as e:
            logger.error(f"❌ Error stopping server: {e}")
    
    def list_collections(self):
        """List all collections"""
        try:
            request = CollectionRequest(
                operation=CollectionOperation.COLLECTION_LIST
            )
            
            logger.info("📋 Listing all collections...")
            response = self.stub.CollectionOperation(request)
            
            if response.success:
                collection_count = len(response.collections)
                logger.info(f"✅ Found {collection_count} collections:")
                for i, collection in enumerate(response.collections, 1):
                    logger.info(f"  {i}. {collection.config.name} (dim: {collection.config.dimension})")
                return response.collections
            else:
                logger.error(f"❌ Failed to list collections: {response.error_message}")
                return []
        except Exception as e:
            logger.error(f"❌ Exception listing collections: {e}")
            return []
    
    def create_collection(self, name, dimension=128):
        """Create a new collection"""
        try:
            config = CollectionConfig(
                name=name,
                dimension=dimension,
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
            
            logger.info(f"📝 Creating collection: {name}")
            response = self.stub.CollectionOperation(request)
            
            if response.success:
                logger.info(f"✅ Collection '{name}' created successfully")
                return True
            else:
                logger.error(f"❌ Failed to create collection '{name}': {response.error_message}")
                return False
        except Exception as e:
            logger.error(f"❌ Exception creating collection '{name}': {e}")
            return False
    
    def check_data_files(self):
        """Check what files exist on disk"""
        logger.info("📂 Checking data files on disk...")
        
        data_dirs = [
            "/home/vsingh/code/proximadb/data",
            "/home/vsingh/code/proximadb/data/metadata",
            "/home/vsingh/code/proximadb/data/metadata/wal"
        ]
        
        total_files = 0
        for data_dir in data_dirs:
            if os.path.exists(data_dir):
                try:
                    files = []
                    for root, dirs, filenames in os.walk(data_dir):
                        for filename in filenames:
                            files.append(os.path.join(root, filename))
                    
                    total_files += len(files)
                    logger.info(f"  📁 {data_dir}: {len(files)} files")
                    
                    # Show first few files as examples
                    for file in files[:3]:
                        size = os.path.getsize(file)
                        logger.info(f"    📄 {file} ({size} bytes)")
                    
                    if len(files) > 3:
                        logger.info(f"    ... and {len(files) - 3} more files")
                        
                except Exception as e:
                    logger.error(f"❌ Error reading {data_dir}: {e}")
            else:
                logger.info(f"  📁 {data_dir}: not found")
        
        logger.info(f"📊 Total files found: {total_files}")
        return total_files

def main():
    logger.info("🧪 Step-by-Step Collection Persistence Test")
    logger.info("=" * 60)
    
    tester = ProximaDBTester()
    
    try:
        # Step 1: Start server and list initial collections
        logger.info("\n📋 STEP 1: Start server and list initial collections")
        logger.info("-" * 50)
        
        if not tester.start_server():
            return False
        
        if not tester.connect():
            return False
        
        initial_collections = tester.list_collections()
        initial_count = len(initial_collections)
        
        # Step 2: Check initial data files
        logger.info("\n📋 STEP 2: Check initial data files")
        logger.info("-" * 50)
        initial_files = tester.check_data_files()
        
        # Step 3: Add a new collection
        logger.info("\n📋 STEP 3: Add a new collection")
        logger.info("-" * 50)
        
        test_collection_name = f"persistence_test_{int(time.time())}"
        if not tester.create_collection(test_collection_name, dimension=256):
            return False
        
        # Step 4: List collections after creation
        logger.info("\n📋 STEP 4: List collections after creation")
        logger.info("-" * 50)
        after_create_collections = tester.list_collections()
        after_create_count = len(after_create_collections)
        
        # Step 5: Check data files after creation
        logger.info("\n📋 STEP 5: Check data files after creation")
        logger.info("-" * 50)
        after_create_files = tester.check_data_files()
        
        # Step 6: Restart server
        logger.info("\n📋 STEP 6: Restart server")
        logger.info("-" * 50)
        
        tester.disconnect()
        tester.stop_server()
        
        logger.info("⏱️ Waiting 3 seconds...")
        time.sleep(3)
        
        if not tester.start_server():
            return False
        
        if not tester.connect():
            return False
        
        # Step 7: List collections after restart
        logger.info("\n📋 STEP 7: List collections after restart")
        logger.info("-" * 50)
        after_restart_collections = tester.list_collections()
        after_restart_count = len(after_restart_collections)
        
        # Step 8: Check data files after restart
        logger.info("\n📋 STEP 8: Check data files after restart")
        logger.info("-" * 50)
        after_restart_files = tester.check_data_files()
        
        # Step 9: Analyze results
        logger.info("\n📋 STEP 9: Test Results Analysis")
        logger.info("-" * 50)
        
        logger.info(f"📊 Collection counts:")
        logger.info(f"  Initial: {initial_count}")
        logger.info(f"  After creation: {after_create_count}")
        logger.info(f"  After restart: {after_restart_count}")
        
        logger.info(f"📊 File counts:")
        logger.info(f"  Initial: {initial_files}")
        logger.info(f"  After creation: {after_create_files}")
        logger.info(f"  After restart: {after_restart_files}")
        
        # Check if our test collection persisted
        test_collection_found = any(
            col.config.name == test_collection_name 
            for col in after_restart_collections
        )
        
        logger.info(f"📊 Test collection '{test_collection_name}' found after restart: {test_collection_found}")
        
        # Determine test result
        success = (
            after_create_count == initial_count + 1 and  # Collection was created
            after_restart_count == after_create_count and  # Collection count maintained
            test_collection_found and  # Our specific collection found
            after_restart_files > initial_files  # Files were written to disk
        )
        
        if success:
            logger.info("🎉 ✅ PERSISTENCE TEST PASSED!")
            logger.info("   Collections successfully persist across server restarts")
        else:
            logger.error("❌ PERSISTENCE TEST FAILED!")
            if after_create_count != initial_count + 1:
                logger.error("   Collection creation failed")
            if after_restart_count != after_create_count:
                logger.error("   Collection count changed after restart")
            if not test_collection_found:
                logger.error("   Test collection not found after restart")
            if after_restart_files <= initial_files:
                logger.error("   No new files written to disk")
        
        return success
        
    except Exception as e:
        logger.error(f"❌ Test failed with exception: {e}")
        return False
    
    finally:
        # Cleanup
        logger.info("\n🧹 Cleanup")
        logger.info("-" * 50)
        tester.disconnect()
        tester.stop_server()

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)
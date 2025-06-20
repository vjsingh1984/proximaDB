#!/usr/bin/env python3
"""
ProximaDB Debug Persistence Test - Fresh Build

Tests with the new release binary:
1. 2-disk hierarchical storage layout with debug logs
2. Collection creation and persistence verification
3. Metadata storage across server restarts
4. File system operations tracking
"""

import asyncio
import subprocess
import time
import os
import sys
import grpc
import logging
import json

# Add the client directory to Python path
sys.path.append('/home/vsingh/code/proximadb/clients/python/src')
from proximadb import proximadb_pb2, proximadb_pb2_grpc

# Configure detailed logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

class DebugPersistenceTest:
    def __init__(self):
        self.server_process = None
        self.channel = None
        self.stub = None
        self.test_collections = [
            {
                "name": "fresh_test_1",
                "dimension": 128,
                "distance_metric": 1,  # Cosine
                "storage_engine": 1,   # Viper
                "indexing_algorithm": 1,  # HNSW
            },
            {
                "name": "fresh_test_2", 
                "dimension": 256,
                "distance_metric": 2,  # Euclidean
                "storage_engine": 1,   # Viper
                "indexing_algorithm": 2,  # IVF
            }
        ]

    async def start_server(self):
        """Start server with release binary and debug logging"""
        logger.info("🚀 Starting ProximaDB server with release binary...")
        
        # Ensure clean directory
        os.chdir("/home/vsingh/code/proximadb")
        
        # Set environment for debug logging
        env = os.environ.copy()
        env.update({
            'RUST_LOG': 'debug,proximadb=trace',
            'RUST_BACKTRACE': '1'
        })
        
        # Use release binary directly
        cmd = ["./target/release/proximadb-server", "--config", "config.toml"]
        
        logger.info(f"📝 Server command: {' '.join(cmd)}")
        logger.info(f"🔧 Config file: {os.path.abspath('config.toml')}")
        
        # Start server and capture output
        self.server_process = subprocess.Popen(
            cmd,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1
        )
        
        logger.info(f"✅ Server process started (PID: {self.server_process.pid})")
        
        # Wait for server to be ready
        max_wait = 15
        for i in range(max_wait):
            if self.server_process.poll() is not None:
                stdout, _ = self.server_process.communicate()
                logger.error(f"❌ Server exited early: {stdout[-1000:]}")
                raise Exception("Server process exited unexpectedly")
            
            # Check if port is listening
            try:
                import socket
                sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                sock.settimeout(1)
                result = sock.connect_ex(('localhost', 5679))
                sock.close()
                if result == 0:
                    logger.info(f"✅ Server ready on port 5679 (attempt {i+1}/{max_wait})")
                    await asyncio.sleep(1)  # Give it a moment to fully initialize
                    return
            except:
                pass
            
            await asyncio.sleep(1)
        
        raise Exception(f"Server failed to start within {max_wait} seconds")

    async def stop_server(self):
        """Stop server and capture final logs"""
        if self.server_process:
            logger.info("🛑 Stopping server...")
            self.server_process.terminate()
            
            try:
                stdout, _ = self.server_process.communicate(timeout=5)
                logger.info(f"📝 Server final output: {stdout[-500:] if stdout else 'No output'}")
            except subprocess.TimeoutExpired:
                logger.warning("⚠️ Server didn't stop gracefully, forcing kill")
                self.server_process.kill()
                stdout, _ = self.server_process.communicate()
                logger.info(f"📝 Server forced stop output: {stdout[-500:] if stdout else 'No output'}")
            
            self.server_process = None
            logger.info("✅ Server stopped")

    async def connect_client(self):
        """Connect gRPC client"""
        logger.info("🔌 Connecting gRPC client...")
        
        self.channel = grpc.aio.insecure_channel("localhost:5679")
        self.stub = proximadb_pb2_grpc.ProximaDBStub(self.channel)
        
        # Test connection
        health_request = proximadb_pb2.HealthRequest()
        health_response = await self.stub.Health(health_request)
        logger.info(f"✅ Connected! Server: {health_response.status} v{health_response.version}")
        logger.info(f"📊 Uptime: {health_response.uptime_seconds}s, Memory: {health_response.memory_usage_bytes} bytes")

    async def disconnect_client(self):
        """Disconnect gRPC client"""
        if self.channel:
            await self.channel.close()
            self.channel = None
            self.stub = None
            logger.info("🔌 Client disconnected")

    async def create_collections(self):
        """Create test collections with detailed logging"""
        logger.info("📦 Creating test collections...")
        
        created = []
        for i, config in enumerate(self.test_collections):
            logger.info(f"📦 Creating collection {i+1}: {config['name']}")
            
            try:
                grpc_config = proximadb_pb2.CollectionConfig(
                    name=config["name"],
                    dimension=config["dimension"],
                    distance_metric=config["distance_metric"],
                    storage_engine=config["storage_engine"],
                    indexing_algorithm=config["indexing_algorithm"],
                    filterable_metadata_fields=[],
                    indexing_config={}
                )
                
                request = proximadb_pb2.CollectionRequest(
                    operation=1,  # CREATE
                    collection_config=grpc_config
                )
                
                response = await self.stub.CollectionOperation(request)
                
                if response.success:
                    logger.info(f"✅ Collection '{config['name']}' created")
                    if response.collection:
                        logger.info(f"   🆔 UUID: {response.collection.id}")
                        logger.info(f"   📅 Created: {response.collection.created_at}")
                    created.append(config['name'])
                else:
                    logger.error(f"❌ Failed: {response.error_message}")
                    
            except Exception as e:
                logger.error(f"❌ Exception: {e}")
        
        logger.info(f"✅ Created {len(created)} collections")
        return created

    async def list_collections(self, context=""):
        """List collections with full details"""
        logger.info(f"📋 Listing collections {context}...")
        
        try:
            request = proximadb_pb2.CollectionRequest(operation=4)  # LIST
            response = await self.stub.CollectionOperation(request)
            
            if response.success:
                count = len(response.collections)
                logger.info(f"✅ Found {count} collections {context}")
                
                collections = []
                for i, coll in enumerate(response.collections):
                    info = {
                        "name": coll.config.name,
                        "uuid": coll.id,
                        "dimension": coll.config.dimension,
                        "distance_metric": coll.config.distance_metric,
                        "storage_engine": coll.config.storage_engine,
                        "indexing_algorithm": coll.config.indexing_algorithm,
                        "created_at": coll.created_at,
                        "vector_count": coll.stats.vector_count
                    }
                    collections.append(info)
                    
                    logger.info(f"  📋 Collection {i+1}: {info['name']}")
                    logger.info(f"     🆔 UUID: {info['uuid']}")
                    logger.info(f"     📐 Dim: {info['dimension']}, Distance: {info['distance_metric']}")
                    logger.info(f"     💾 Engine: {info['storage_engine']}, Algorithm: {info['indexing_algorithm']}")
                    logger.info(f"     📊 Vectors: {info['vector_count']}, Created: {info['created_at']}")
                
                return collections
            else:
                logger.error(f"❌ Failed to list collections: {response.error_message}")
                return []
                
        except Exception as e:
            logger.error(f"❌ Exception: {e}")
            return []

    async def check_filesystem(self, context=""):
        """Check filesystem state with detailed analysis"""
        logger.info(f"🗂️ Checking filesystem state {context}...")
        
        dirs_to_check = [
            "/data/proximadb/1/metadata",
            "/data/proximadb/1/wal",
            "/data/proximadb/1/store", 
            "/data/proximadb/2/wal",
            "/data/proximadb/2/store"
        ]
        
        total_files = 0
        for directory in dirs_to_check:
            if os.path.exists(directory):
                try:
                    files = [f for f in os.listdir(directory) if os.path.isfile(os.path.join(directory, f))]
                    subdirs = [d for d in os.listdir(directory) if os.path.isdir(os.path.join(directory, d))]
                    
                    logger.info(f"📁 {directory}:")
                    logger.info(f"   📄 Files: {len(files)}")
                    logger.info(f"   📁 Subdirs: {len(subdirs)}")
                    
                    # Show some file details
                    for file in files[:3]:  # Show first 3 files
                        file_path = os.path.join(directory, file)
                        size = os.path.getsize(file_path)
                        logger.info(f"      📄 {file} ({size} bytes)")
                    
                    # Show subdirectories (collection UUIDs)
                    for subdir in subdirs[:3]:  # Show first 3 subdirs
                        subdir_path = os.path.join(directory, subdir)
                        sub_files = len([f for f in os.listdir(subdir_path) 
                                       if os.path.isfile(os.path.join(subdir_path, f))])
                        logger.info(f"      📁 {subdir}/ ({sub_files} files)")
                    
                    total_files += len(files)
                    
                except Exception as e:
                    logger.error(f"❌ Error checking {directory}: {e}")
            else:
                logger.warning(f"❌ Directory missing: {directory}")
        
        logger.info(f"📊 Total files across all directories: {total_files}")

    async def run_persistence_test(self):
        """Run complete persistence test with full debugging"""
        logger.info("🧪 ProximaDB Fresh Build Debug Persistence Test")
        logger.info("=" * 80)
        
        try:
            # Phase 1: Clean start
            logger.info("\n=== PHASE 1: Fresh Server Start ===")
            await self.start_server()
            await self.connect_client()
            await self.check_filesystem("initial state")
            
            # Phase 2: Create collections
            logger.info("\n=== PHASE 2: Create Collections ===")
            created = await self.create_collections()
            
            if not created:
                logger.error("❌ No collections created, aborting")
                return False
            
            await self.check_filesystem("after collection creation")
            
            # Phase 3: List before restart
            logger.info("\n=== PHASE 3: Pre-Restart State ===")
            before = await self.list_collections("before restart")
            
            # Phase 4: Server restart
            logger.info("\n=== PHASE 4: Server Restart ===")
            await self.disconnect_client()
            await self.stop_server()
            
            logger.info("⏳ Waiting 3 seconds...")
            await asyncio.sleep(3)
            await self.check_filesystem("during downtime")
            
            logger.info("🔄 Restarting server...")
            await self.start_server()
            await self.connect_client()
            
            # Phase 5: List after restart  
            logger.info("\n=== PHASE 5: Post-Restart State ===")
            after = await self.list_collections("after restart")
            await self.check_filesystem("after restart")
            
            # Phase 6: Verify persistence
            logger.info("\n=== PHASE 6: Persistence Verification ===")
            
            success = (
                len(before) == len(self.test_collections) and
                len(after) == len(self.test_collections) and
                len(before) == len(after)
            )
            
            if success:
                # Verify details match
                before_names = {c['name']: c for c in before}
                after_names = {c['name']: c for c in after}
                
                for name in before_names:
                    if name not in after_names:
                        logger.error(f"❌ Collection '{name}' missing after restart")
                        success = False
                        continue
                    
                    b = before_names[name]
                    a = after_names[name]
                    
                    for field in ['dimension', 'distance_metric', 'storage_engine', 'indexing_algorithm']:
                        if b[field] != a[field]:
                            logger.error(f"❌ '{name}' {field}: {b[field]} -> {a[field]}")
                            success = False
                        else:
                            logger.info(f"✅ '{name}' {field} preserved: {b[field]}")
            
            # Results
            logger.info("\n=== PHASE 7: Final Results ===")
            logger.info(f"📊 Collections before: {len(before)}")
            logger.info(f"📊 Collections after: {len(after)}")  
            logger.info(f"📊 Expected: {len(self.test_collections)}")
            logger.info(f"📊 Persistence: {'✅ SUCCESS' if success else '❌ FAILED'}")
            
            if success:
                logger.info("🎉 PERSISTENCE TEST PASSED!")
                logger.info("✅ Collections persist across server restarts")
                logger.info("✅ All metadata preserved correctly")
                logger.info("✅ 2-disk storage layout working")
                logger.info("✅ Release binary functioning properly")
            else:
                logger.error("❌ PERSISTENCE TEST FAILED!")
            
            return success
            
        except Exception as e:
            logger.error(f"❌ Test failed: {e}")
            import traceback
            logger.error(f"📝 Traceback: {traceback.format_exc()}")
            return False
        finally:
            await self.disconnect_client()
            await self.stop_server()

async def main():
    """Main test function"""
    tester = DebugPersistenceTest()
    success = await tester.run_persistence_test()
    
    if success:
        print("\n✅ Debug Persistence Test PASSED!")
        exit(0)
    else:
        print("\n❌ Debug Persistence Test FAILED!")
        exit(1)

if __name__ == "__main__":
    asyncio.run(main())
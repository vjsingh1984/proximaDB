#!/usr/bin/env python3
"""
Test just collection creation and persistence (no search)
"""

import grpc
import subprocess
import time
import logging
import shutil
from pathlib import Path
import sys

# Add the client path
sys.path.append('./clients/python/src')
import proximadb.proximadb_pb2 as pb2
import proximadb.proximadb_pb2_grpc as pb2_grpc

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

def force_kill_servers():
    """Force kill any running ProximaDB servers"""
    try:
        subprocess.run(["pkill", "-f", "proximadb-server"], check=False)
        time.sleep(2)
    except:
        pass

def test_collection_persistence():
    """Test collection persistence without search operations"""
    
    # Clean environment
    force_kill_servers()
    
    data_dir = Path("./test_collection_persistence")
    if data_dir.exists():
        shutil.rmtree(data_dir)
    data_dir.mkdir(parents=True, exist_ok=True)
    
    # Clean default data dir too
    default_data_dir = Path("./data")
    if default_data_dir.exists():
        shutil.rmtree(default_data_dir)
    default_data_dir.mkdir(parents=True, exist_ok=True)
    
    logger.info("🧪 Collection Persistence Test (WAL Backend)")
    logger.info("=" * 60)
    
    server_proc = None
    
    try:
        # === Phase 1: Start server ===
        logger.info("\n📋 PHASE 1: Server Start")
        logger.info("-" * 50)
        
        logger.info("🚀 Starting ProximaDB server...")
        server_proc = subprocess.Popen([
            "cargo", "run", "--bin", "proximadb-server", "--",
            "--data-dir", str(data_dir)
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        
        # Wait for server startup
        time.sleep(15)
        
        # === Phase 2: Create collections via gRPC ===
        logger.info("\n📋 PHASE 2: Collection Creation")
        logger.info("-" * 50)
        
        # Connect to server
        channel = grpc.insecure_channel('localhost:5679')
        stub = pb2_grpc.ProximaDBStub(channel)
        
        # Test health
        logger.info("🔍 Testing server health...")
        health_response = stub.Health(pb2.HealthRequest(), timeout=10)
        logger.info(f"✅ Server healthy: {health_response.status}")
        
        # Create collection configurations
        collections_to_create = [
            {
                "name": "test_persistence_collection_1", 
                "dimension": 768,
                "distance_metric": pb2.COSINE,
                "storage_engine": pb2.VIPER,
                "indexing_algorithm": pb2.HNSW
            },
            {
                "name": "test_persistence_collection_2",
                "dimension": 512, 
                "distance_metric": pb2.EUCLIDEAN,
                "storage_engine": pb2.VIPER,
                "indexing_algorithm": pb2.FLAT
            }
        ]
        
        # Create collections
        for config in collections_to_create:
            logger.info(f"📝 Creating collection: {config['name']}")
            
            # Create collection config
            collection_config = pb2.CollectionConfig(
                name=config["name"],
                dimension=config["dimension"],
                distance_metric=config["distance_metric"],
                storage_engine=config["storage_engine"],
                indexing_algorithm=config["indexing_algorithm"]
            )
            
            # Send create request
            request = pb2.CollectionRequest(
                operation=pb2.COLLECTION_CREATE,
                collection_config=collection_config
            )
            
            response = stub.CollectionOperation(request, timeout=30)
            
            if response.success:
                logger.info(f"✅ Created collection: {config['name']}")
            else:
                logger.error(f"❌ Failed to create {config['name']}: {response.error_message}")
        
        # === Phase 3: Skip List (not implemented) ===
        logger.info("\n📋 PHASE 3: Skip List Operation (not implemented)")
        logger.info("-" * 50)
        logger.info("⚠️ Skipping LIST operation - not yet implemented")
        
        # Give time for WAL operations to complete
        logger.info("⏳ Waiting for WAL flush...")
        time.sleep(5)
        
        # === Phase 4: Check persistence files ===
        logger.info("\n📋 PHASE 4: File System Check")
        logger.info("-" * 50)
        
        # Check all possible data directories
        directories_to_check = [
            data_dir,
            default_data_dir,
            Path("./data"),
            Path("./metadata")
        ]
        
        total_files = 0
        for directory in directories_to_check:
            if directory.exists():
                files = list(directory.rglob("*"))
                file_count = len([f for f in files if f.is_file()])
                logger.info(f"📁 {directory}: {file_count} files")
                
                for f in files:
                    if f.is_file():
                        logger.info(f"  📄 {f.relative_to(directory)} ({f.stat().st_size} bytes)")
                        total_files += 1
        
        # === Phase 5: Server restart test ===
        logger.info("\n📋 PHASE 5: Server Restart Test")
        logger.info("-" * 50)
        
        logger.info("🛑 Stopping server...")
        if server_proc:
            server_proc.terminate()
            try:
                server_proc.wait(timeout=10)
                logger.info("✅ Server stopped gracefully")
            except subprocess.TimeoutExpired:
                server_proc.kill()
                server_proc.wait()
                logger.info("✅ Server force stopped")
        
        time.sleep(3)
        
        logger.info("🔄 Restarting server...")
        server_proc = subprocess.Popen([
            "cargo", "run", "--bin", "proximadb-server", "--",
            "--data-dir", str(data_dir)
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        
        time.sleep(15)
        
        # Test persistence by trying to create same collections again
        logger.info("📋 Testing collection persistence...")
        channel = grpc.insecure_channel('localhost:5679')
        stub = pb2_grpc.ProximaDBStub(channel)
        
        # Try to recreate the same collections - should fail if they exist
        collections_after_restart = 0
        for config in collections_to_create:
            collection_config = pb2.CollectionConfig(
                name=config["name"],
                dimension=config["dimension"],
                distance_metric=config["distance_metric"],
                storage_engine=config["storage_engine"],
                indexing_algorithm=config["indexing_algorithm"]
            )
            
            request = pb2.CollectionRequest(
                operation=pb2.COLLECTION_CREATE,
                collection_config=collection_config
            )
            
            try:
                response = stub.CollectionOperation(request, timeout=10)
                if not response.success and "already exists" in (response.error_message or "").lower():
                    logger.info(f"✅ Collection {config['name']} already exists (persisted)")
                    collections_after_restart += 1
                elif response.success:
                    logger.warning(f"⚠️ Collection {config['name']} recreated (not persisted)")
                else:
                    logger.error(f"❌ Unexpected error for {config['name']}: {response.error_message}")
            except Exception as e:
                logger.error(f"❌ Error testing {config['name']}: {e}")
        
        # === Phase 6: Results ===
        logger.info("\n📋 FINAL RESULTS")
        logger.info("=" * 50)
        
        logger.info(f"📊 Summary:")
        logger.info(f"   Collections created: {len(collections_to_create)}")
        logger.info(f"   Collections after restart: {collections_after_restart}")
        logger.info(f"   Persistence files: {total_files}")
        
        # Determine success
        collections_persisted = collections_after_restart >= len(collections_to_create)
        files_created = total_files > 0
        
        if collections_persisted and files_created:
            logger.info("🎉 COLLECTION PERSISTENCE TEST PASSED!")
            logger.info("✅ Collections persisted across restart")
            logger.info("✅ WAL files created on disk")
            logger.info("✅ WAL metadata backend working correctly")
            return True
        elif collections_persisted and not files_created:
            logger.warning("⚠️ PARTIAL SUCCESS:")
            logger.warning("✅ Collections persisted across restart")
            logger.warning("❌ No WAL files found (may be using memory backend)")
            return False
        else:
            logger.error("❌ COLLECTION PERSISTENCE TEST FAILED!")
            logger.error("❌ Collections not persisted across restart")
            return False
            
    except Exception as e:
        logger.error(f"❌ Test failed with exception: {e}")
        return False
        
    finally:
        # Cleanup
        force_kill_servers()
        if server_proc:
            try:
                server_proc.terminate()
                server_proc.wait(timeout=5)
            except:
                try:
                    server_proc.kill()
                    server_proc.wait(timeout=5)
                except:
                    pass
        logger.info("🧹 Test cleanup completed")

if __name__ == "__main__":
    success = test_collection_persistence()
    exit(0 if success else 1)
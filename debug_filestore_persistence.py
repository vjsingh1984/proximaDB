#!/usr/bin/env python3
"""
Debug filestore persistence - minimal test to identify the exact failure point
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

def debug_filestore_persistence():
    """Debug exactly where filestore persistence is failing"""
    
    # Clean environment
    subprocess.run(["pkill", "-f", "proximadb-server"], check=False)
    time.sleep(2)
    
    data_dir = Path("./debug_data")
    if data_dir.exists():
        shutil.rmtree(data_dir)
    data_dir.mkdir(parents=True, exist_ok=True)
    
    logger.info("🔍 DEBUG: Filestore Persistence Issue")
    logger.info("=" * 60)
    
    server_proc = None
    
    try:
        # Start server with debug data dir
        logger.info("🚀 Starting server with data dir: ./debug_data")
        server_proc = subprocess.Popen([
            "cargo", "run", "--bin", "proximadb-server", "--",
            "--data-dir", str(data_dir)
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        
        time.sleep(10)
        
        # Check initial file structure
        logger.info("\n📁 BEFORE: File structure")
        for f in data_dir.rglob("*"):
            if f.is_file():
                logger.info(f"  📄 {f.relative_to(data_dir)} ({f.stat().st_size} bytes)")
        
        # Connect and create ONE collection
        channel = grpc.insecure_channel('localhost:5679')
        stub = pb2_grpc.ProximaDBStub(channel)
        
        logger.info("\n📝 Creating single test collection...")
        collection_config = pb2.CollectionConfig(
            name="debug_collection",
            dimension=128,
            distance_metric=pb2.COSINE,
            storage_engine=pb2.VIPER,
            indexing_algorithm=pb2.HNSW
        )
        
        request = pb2.CollectionRequest(
            operation=pb2.COLLECTION_CREATE,
            collection_config=collection_config
        )
        
        response = stub.CollectionOperation(request, timeout=30)
        
        if response.success:
            logger.info("✅ Collection created successfully")
        else:
            logger.error(f"❌ Collection creation failed: {response.error_message}")
            return False
        
        # Wait for potential flush
        logger.info("⏳ Waiting 10 seconds for filestore operations...")
        time.sleep(10)
        
        # Check file structure after creation
        logger.info("\n📁 AFTER: File structure")
        found_metadata = False
        found_wal = False
        
        for f in data_dir.rglob("*"):
            if f.is_file():
                logger.info(f"  📄 {f.relative_to(data_dir)} ({f.stat().st_size} bytes)")
                if "metadata" in str(f).lower():
                    found_metadata = True
                if "wal" in str(f).lower() or ".avro" in str(f).lower():
                    found_wal = True
        
        # Check specific paths that should exist
        expected_paths = [
            data_dir / "metadata",
            data_dir / "metadata" / "snapshots",
            data_dir / "metadata" / "incremental", 
            data_dir / "wal"
        ]
        
        logger.info("\n📋 Expected path check:")
        for path in expected_paths:
            exists = path.exists()
            logger.info(f"  {'✅' if exists else '❌'} {path}")
            if exists and path.is_dir():
                files = list(path.iterdir())
                logger.info(f"    📁 Contains {len(files)} items")
                for item in files[:5]:  # Show first 5 items
                    logger.info(f"      📄 {item.name}")
        
        # Results
        logger.info("\n🔍 DIAGNOSIS:")
        logger.info(f"  Metadata files found: {'✅' if found_metadata else '❌'}")
        logger.info(f"  WAL files found: {'✅' if found_wal else '❌'}")
        
        if not found_metadata and not found_wal:
            logger.error("❌ ISSUE: No persistence files created at all")
            logger.error("   This suggests FilestoreMetadataBackend is not writing to disk")
        elif found_wal and not found_metadata:
            logger.error("❌ ISSUE: WAL files exist but no metadata files")
            logger.error("   This suggests FilestoreMetadataBackend is not functioning")
        elif found_metadata and not found_wal:
            logger.warning("⚠️ ISSUE: Metadata files exist but no WAL files")
            logger.warning("   This suggests WAL is using memory backend")
        else:
            logger.info("✅ Both metadata and WAL files found")
        
        return found_metadata or found_wal
        
    except Exception as e:
        logger.error(f"❌ Debug failed: {e}")
        return False
        
    finally:
        if server_proc:
            server_proc.terminate()
            try:
                server_proc.wait(timeout=5)
            except:
                server_proc.kill()

if __name__ == "__main__":
    debug_filestore_persistence()
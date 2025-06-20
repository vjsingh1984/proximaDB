#!/usr/bin/env python3

import subprocess
import time
import logging
import os
import shutil
import asyncio
from clients.python.src.proximadb.grpc_client import ProximaDBClient

# Set up logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

def cleanup_data():
    """Clean up test data"""
    paths_to_clean = [
        "/data/proximadb/1/metadata",
        "/data/proximadb/1/wal", 
        "/data/proximadb/1/store",
        "/data/proximadb/2",
        "./metadata",
        "./data"
    ]
    
    for path in paths_to_clean:
        if os.path.exists(path):
            if os.path.isdir(path):
                shutil.rmtree(path)
                logger.info(f"🧹 Cleaned up directory: {path}")
            else:
                os.remove(path)
                logger.info(f"🧹 Cleaned up file: {path}")

async def test_unified_metadata():
    """Test that metadata is stored in the configured location"""
    
    logger.info("🧹 Cleaning up previous test data...")
    cleanup_data()
    
    # Ensure directory exists
    os.makedirs("/data/proximadb/1/metadata", exist_ok=True)
    
    logger.info("🚀 Starting server with unified metadata configuration...")
    server_process = subprocess.Popen(
        ["./target/release/proximadb-server", "--config", "config.toml"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT
    )
    
    try:
        # Wait for server to start
        time.sleep(8)
        
        logger.info("📡 Connecting to server...")
        client = ProximaDBClient("localhost:5679")
        
        logger.info("📋 Creating test collection...")
        collection = await client.create_collection(
            name="unified_test",
            dimension=128,
            distance_metric=1,  # COSINE
            indexing_algorithm=1,  # HNSW
            storage_engine=1  # VIPER
        )
        if collection:
            logger.info(f"✅ Collection created successfully: {collection.name}")
        else:
            logger.error("❌ Collection creation failed")
            return False
        
        # Give the server a moment to write metadata
        time.sleep(2)
        
        # Check metadata files
        metadata_dir = "/data/proximadb/1/metadata"
        metadata_files = []
        if os.path.exists(metadata_dir):
            metadata_files = os.listdir(metadata_dir)
            logger.info(f"📁 Metadata files in {metadata_dir}: {metadata_files}")
        else:
            logger.error(f"❌ Metadata directory does not exist: {metadata_dir}")
        
        # Check if wrong location has files (should be empty)
        wrong_locations = ["./metadata", "./data/metadata", "./data"]
        for wrong_loc in wrong_locations:
            if os.path.exists(wrong_loc):
                try:
                    wrong_files = os.listdir(wrong_loc)
                    if wrong_files:
                        logger.error(f"❌ Found metadata files in wrong location {wrong_loc}: {wrong_files}")
                        # List subdirectories too
                        for item in wrong_files:
                            item_path = os.path.join(wrong_loc, item)
                            if os.path.isdir(item_path):
                                subitems = os.listdir(item_path)
                                logger.error(f"  📁 {item}/ contains: {subitems}")
                        return False
                    else:
                        logger.info(f"✅ No metadata files in wrong location: {wrong_loc}")
                except PermissionError:
                    logger.info(f"⚠️ Cannot access directory: {wrong_loc}")
            else:
                logger.info(f"✅ Wrong location doesn't exist (good): {wrong_loc}")
        
        # If we reach here, metadata is in the correct location
        if metadata_files:
            logger.info("✅ Unified metadata configuration test PASSED!")
            return True
        else:
            logger.error("❌ No metadata files found in correct location but collection was created")
            return False
        
    except Exception as e:
        logger.error(f"❌ Test failed with exception: {e}")
        return False
    
    finally:
        logger.info("🛑 Stopping server...")
        server_process.terminate()
        server_process.wait()
        logger.info("✅ Server stopped")

if __name__ == "__main__":
    success = asyncio.run(test_unified_metadata())
    if success:
        logger.info("🎉 All unified metadata tests passed!")
    else:
        logger.error("❌ Unified metadata tests failed!")
        exit(1)
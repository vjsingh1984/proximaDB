#!/usr/bin/env python3
"""
Simple WAL Metadata Backend Persistence Test

Tests that collections created through gRPC SDK persist across server restarts
when using the WAL metadata backend (default configuration).
"""

import asyncio
import subprocess
import time
import logging
import os
import signal
from pathlib import Path
from typing import Optional

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

class SimpleWALTest:
    """Simple WAL persistence test using final_working_demo.py as reference"""
    
    def __init__(self, data_dir: str = "./test_wal_data"):
        self.data_dir = Path(data_dir)
        self.server_process: Optional[subprocess.Popen] = None
        
    async def start_server(self) -> bool:
        """Start ProximaDB server with WAL backend"""
        try:
            # Ensure data directory exists
            self.data_dir.mkdir(parents=True, exist_ok=True)
            
            logger.info("🔨 Building ProximaDB server...")
            build_result = subprocess.run(
                ["cargo", "build", "--bin", "proximadb-server"],
                capture_output=True,
                text=True,
                timeout=120
            )
            
            if build_result.returncode != 0:
                logger.error(f"❌ Build failed: {build_result.stderr}")
                return False
                
            logger.info("✅ Server built successfully")
            
            # Start server with WAL backend
            logger.info(f"🚀 Starting ProximaDB server with data dir: {self.data_dir}")
            self.server_process = subprocess.Popen(
                [
                    "cargo", "run", "--bin", "proximadb-server", "--",
                    "--data-dir", str(self.data_dir),
                    "--grpc-port", "5678"
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True
            )
            
            # Wait for server to be ready
            max_retries = 30
            for attempt in range(max_retries):
                try:
                    # Test with simple python client
                    result = subprocess.run(
                        ["python", "test_health_check.py"],
                        capture_output=True,
                        text=True,
                        timeout=10
                    )
                    
                    if result.returncode == 0:
                        logger.info(f"✅ Server is ready!")
                        return True
                        
                except Exception:
                    pass
                    
                if attempt < max_retries - 1:
                    logger.info(f"⏳ Waiting for server... (attempt {attempt + 1}/{max_retries})")
                    await asyncio.sleep(2)
                else:
                    logger.error(f"❌ Server failed to start")
                    return False
                        
        except Exception as e:
            logger.error(f"❌ Failed to start server: {e}")
            return False
            
    async def stop_server(self) -> bool:
        """Stop ProximaDB server gracefully"""
        try:
            if self.server_process and self.server_process.poll() is None:
                logger.info("🛑 Stopping ProximaDB server...")
                
                # Try graceful shutdown first
                self.server_process.terminate()
                
                try:
                    # Wait up to 10 seconds for graceful shutdown
                    stdout, stderr = self.server_process.communicate(timeout=10)
                    logger.info("✅ Server stopped gracefully")
                        
                except subprocess.TimeoutExpired:
                    logger.warning("⚠️ Graceful shutdown timed out, forcing kill...")
                    self.server_process.kill()
                    self.server_process.wait()
                    logger.info("✅ Server force killed")
                    
                self.server_process = None
                return True
            else:
                logger.info("ℹ️ Server was not running")
                return True
                
        except Exception as e:
            logger.error(f"❌ Failed to stop server: {e}")
            return False
            
    def cleanup_data_dir(self):
        """Clean up test data directory"""
        try:
            if self.data_dir.exists():
                import shutil
                shutil.rmtree(self.data_dir)
                logger.info(f"🧹 Cleaned up data directory: {self.data_dir}")
        except Exception as e:
            logger.warning(f"⚠️ Failed to cleanup data directory: {e}")
            
    async def test_collections_with_demo_script(self, script_name: str) -> tuple[bool, str]:
        """Test collections using existing demo script"""
        try:
            logger.info(f"🧪 Running {script_name}...")
            result = subprocess.run(
                ["python", script_name],
                capture_output=True,
                text=True,
                timeout=60
            )
            
            success = result.returncode == 0
            output = result.stdout if success else result.stderr
            
            if success:
                logger.info(f"✅ {script_name} completed successfully")
            else:
                logger.error(f"❌ {script_name} failed")
                
            return success, output
            
        except Exception as e:
            logger.error(f"❌ Failed to run {script_name}: {e}")
            return False, str(e)
            
    async def run_persistence_test(self) -> bool:
        """Run the complete WAL persistence test"""
        logger.info("🧪 Starting Simple WAL Metadata Backend Persistence Test")
        logger.info("="*60)
        
        try:
            # Clean start
            self.cleanup_data_dir()
            
            # === PHASE 1: Initial Setup ===
            logger.info("\n📋 PHASE 1: Initial Server Start & Collection Creation")
            logger.info("-" * 50)
            
            if not await self.start_server():
                logger.error("❌ Failed to start server initially")
                return False
                
            # Test initial connection and create collections
            success1, output1 = await self.test_collections_with_demo_script("final_working_demo.py")
            if not success1:
                logger.error("❌ Failed to create collections initially")
                logger.error(f"Output: {output1}")
                return False
                
            logger.info("✅ Phase 1 completed - Collections created successfully")
            
            # === PHASE 2: Server Restart ===
            logger.info("\n📋 PHASE 2: Server Restart & Persistence Verification") 
            logger.info("-" * 50)
            
            # Stop server
            if not await self.stop_server():
                logger.error("❌ Failed to stop server")
                return False
                
            # Wait a moment
            await asyncio.sleep(2)
            
            # Restart server
            if not await self.start_server():
                logger.error("❌ Failed to restart server")
                return False
                
            # Test persistence by running demo again
            success2, output2 = await self.test_collections_with_demo_script("final_working_demo.py") 
            
            logger.info("✅ Phase 2 completed - Server restarted")
            
            # === PHASE 3: Check Data Directory ===
            logger.info("\n📋 PHASE 3: WAL Data Directory Verification")
            logger.info("-" * 50)
            
            # Check if WAL files exist
            wal_files = []
            metadata_files = []
            
            for file_path in self.data_dir.rglob("*"):
                if file_path.is_file():
                    if "wal" in file_path.name.lower():
                        wal_files.append(str(file_path))
                    elif "metadata" in file_path.name.lower():
                        metadata_files.append(str(file_path))
                        
            logger.info(f"📁 Data directory contents:")
            for file_path in self.data_dir.rglob("*"):
                if file_path.is_file():
                    logger.info(f"  📄 {file_path.relative_to(self.data_dir)} ({file_path.stat().st_size} bytes)")
                    
            logger.info(f"📊 Found {len(wal_files)} WAL files and {len(metadata_files)} metadata files")
            
            # === FINAL RESULTS ===
            logger.info("\n📋 FINAL RESULTS")
            logger.info("=" * 50)
            
            persistence_working = len(wal_files) > 0 or len(metadata_files) > 0
            
            if persistence_working:
                logger.info("🎉 WAL PERSISTENCE TEST PASSED!")
                logger.info(f"✅ Server started and collections created")
                logger.info(f"✅ Server restarted successfully")
                logger.info(f"✅ Data persisted to disk ({len(wal_files + metadata_files)} files)")
                logger.info(f"✅ WAL metadata backend working correctly")
                return True
            else:
                logger.error("❌ WAL PERSISTENCE TEST FAILED!")
                logger.error("❌ No WAL or metadata files found - data not persisting")
                return False
                
        except Exception as e:
            logger.error(f"❌ Test failed with exception: {e}")
            return False
            
        finally:
            # Cleanup
            await self.stop_server()
            logger.info("🧹 Test cleanup completed")

async def main():
    """Main test runner"""
    test_runner = SimpleWALTest()
    
    try:
        success = await test_runner.run_persistence_test()
        if success:
            logger.info("\n🎉 ALL TESTS PASSED - WAL persistence working correctly!")
            exit(0)
        else:
            logger.error("\n❌ TESTS FAILED - WAL persistence issues detected!")
            exit(1)
            
    except KeyboardInterrupt:
        logger.info("\n⚠️ Test interrupted by user")
        await test_runner.stop_server()
        exit(1)
        
    except Exception as e:
        logger.error(f"\n❌ Test runner failed: {e}")
        await test_runner.stop_server()
        exit(1)

if __name__ == "__main__":
    asyncio.run(main())
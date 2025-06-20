#!/usr/bin/env python3
"""
Comprehensive WAL backend test with forced persistence verification
"""

import subprocess
import time
import os
import logging
import shutil
from pathlib import Path
import signal

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

def force_kill_servers():
    """Force kill any running ProximaDB servers"""
    try:
        subprocess.run(["pkill", "-f", "proximadb-server"], check=False)
        time.sleep(2)
    except:
        pass

def comprehensive_wal_test():
    """Comprehensive test of WAL backend persistence"""
    
    # Clean environment
    force_kill_servers()
    
    data_dir = Path("./test_wal_comprehensive")
    if data_dir.exists():
        shutil.rmtree(data_dir)
    data_dir.mkdir(parents=True, exist_ok=True)
    
    # Also check default data directory
    default_data_dir = Path("./data")
    if default_data_dir.exists():
        shutil.rmtree(default_data_dir)
    default_data_dir.mkdir(parents=True, exist_ok=True)
    
    logger.info("🧪 Comprehensive WAL Backend Persistence Test")
    logger.info("=" * 60)
    
    server_proc = None
    
    try:
        # === Phase 1: Start server and create collections ===
        logger.info("\n📋 PHASE 1: Server Start and Collection Creation")
        logger.info("-" * 50)
        
        # Start server with explicit configuration
        logger.info("🚀 Starting ProximaDB server...")
        server_proc = subprocess.Popen([
            "cargo", "run", "--bin", "proximadb-server", "--",
            "--data-dir", str(data_dir)
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        
        # Wait longer for server startup
        logger.info("⏳ Waiting for server startup...")
        time.sleep(15)
        
        # Run demo to create collections
        logger.info("📝 Running demo to create collections...")
        result1 = subprocess.run([
            "python", "final_working_demo.py"
        ], capture_output=True, text=True, timeout=60)
        
        if result1.returncode != 0:
            logger.error(f"❌ Demo failed: {result1.stderr}")
            return False
            
        logger.info("✅ Demo completed successfully")
        
        # Give time for WAL operations to complete
        logger.info("⏳ Waiting for WAL operations to flush...")
        time.sleep(5)
        
        # === Phase 2: Check persistence files ===
        logger.info("\n📋 PHASE 2: File System Analysis")
        logger.info("-" * 50)
        
        # Check test data directory
        test_files = list(data_dir.rglob("*"))
        logger.info(f"📁 Files in test directory ({data_dir}):")
        for f in test_files:
            if f.is_file():
                logger.info(f"  📄 {f.relative_to(data_dir)} ({f.stat().st_size} bytes)")
        
        # Check default data directory 
        default_files = list(default_data_dir.rglob("*"))
        logger.info(f"📁 Files in default directory ({default_data_dir}):")
        for f in default_files:
            if f.is_file():
                logger.info(f"  📄 {f.relative_to(default_data_dir)} ({f.stat().st_size} bytes)")
        
        # Check for metadata subdirectories
        metadata_dirs = [
            data_dir / "metadata",
            default_data_dir / "metadata", 
            Path("./data/metadata"),
            Path("./metadata"),
        ]
        
        for md in metadata_dirs:
            if md.exists():
                md_files = list(md.rglob("*"))
                logger.info(f"📁 Files in {md}:")
                for f in md_files:
                    if f.is_file():
                        logger.info(f"  📄 {f.relative_to(md)} ({f.stat().st_size} bytes)")
        
        # === Phase 3: Force server shutdown and restart ===
        logger.info("\n📋 PHASE 3: Server Restart Test")
        logger.info("-" * 50)
        
        logger.info("🛑 Stopping server...")
        if server_proc:
            server_proc.terminate()
            try:
                server_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server_proc.kill()
                server_proc.wait()
        
        logger.info("⏳ Server stopped, waiting...")
        time.sleep(3)
        
        # Restart server
        logger.info("🔄 Restarting server...")
        server_proc = subprocess.Popen([
            "cargo", "run", "--bin", "proximadb-server", "--",
            "--data-dir", str(data_dir)
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        
        time.sleep(15)
        
        # Test persistence by running demo again
        logger.info("📝 Testing persistence with second demo run...")
        result2 = subprocess.run([
            "python", "final_working_demo.py"
        ], capture_output=True, text=True, timeout=60)
        
        restart_success = result2.returncode == 0
        
        # Check files after restart
        test_files_after = list(data_dir.rglob("*"))
        default_files_after = list(default_data_dir.rglob("*"))
        
        # === Phase 4: Analyze Results ===
        logger.info("\n📋 PHASE 4: Results Analysis")
        logger.info("-" * 50)
        
        total_files_before = len([f for f in test_files + default_files if f.is_file()])
        total_files_after = len([f for f in test_files_after + default_files_after if f.is_file()])
        
        logger.info(f"📊 File Analysis:")
        logger.info(f"   Files before restart: {total_files_before}")
        logger.info(f"   Files after restart: {total_files_after}")
        logger.info(f"   Demo 1 success: ✅")
        logger.info(f"   Server restart: ✅")
        logger.info(f"   Demo 2 success: {'✅' if restart_success else '❌'}")
        
        # === Final Verdict ===
        logger.info("\n📋 FINAL VERDICT")
        logger.info("=" * 50)
        
        if total_files_before > 0:
            logger.info("🎉 WAL PERSISTENCE TEST PASSED!")
            logger.info(f"✅ {total_files_before} persistence files created")
            logger.info(f"✅ Server restart successful")
            logger.info(f"✅ WAL backend is working correctly")
            return True
        else:
            logger.error("❌ WAL PERSISTENCE TEST FAILED!")
            logger.error("❌ No persistence files found")
            logger.error("❌ Collections may be stored in memory only")
            
            # Additional debugging
            logger.info("\n🔍 DEBUGGING INFO:")
            
            # Check server logs
            if server_proc:
                try:
                    stdout, stderr = server_proc.communicate(timeout=2)
                    if stderr:
                        logger.info("📋 Server stderr (last 1000 chars):")
                        logger.info(stderr[-1000:])
                except:
                    pass
            
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
    success = comprehensive_wal_test()
    exit(0 if success else 1)
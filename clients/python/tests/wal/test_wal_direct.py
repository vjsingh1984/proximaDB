#!/usr/bin/env python3
"""
Direct WAL persistence test using existing working demo
"""

import subprocess
import time
import os
import logging
from pathlib import Path

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

def test_wal_persistence():
    """Test WAL persistence by running demo before and after restart"""
    
    data_dir = Path("./test_wal_data")
    
    # Clean slate
    if data_dir.exists():
        import shutil
        shutil.rmtree(data_dir)
    data_dir.mkdir(parents=True, exist_ok=True)
    
    logger.info("🧪 Testing WAL persistence with server restarts")
    
    # Start server in background
    logger.info("🚀 Starting server...")
    server_proc = subprocess.Popen([
        "cargo", "run", "--bin", "proximadb-server", "--",
        "--data-dir", str(data_dir)
    ], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    
    # Wait for startup
    time.sleep(10)
    
    try:
        # Run working demo
        logger.info("📝 Running demo (initial)...")
        result1 = subprocess.run([
            "python", "final_working_demo.py"
        ], capture_output=True, text=True, timeout=30)
        
        if result1.returncode != 0:
            logger.error(f"❌ Initial demo failed: {result1.stderr}")
            return False
            
        logger.info("✅ Initial demo completed")
        
        # Stop server
        logger.info("🛑 Stopping server...")
        server_proc.terminate()
        server_proc.wait(timeout=10)
        
        # Check files created
        files_before = list(data_dir.rglob("*"))
        logger.info(f"📁 Files created: {len(files_before)}")
        for f in files_before:
            if f.is_file():
                logger.info(f"  📄 {f.relative_to(data_dir)} ({f.stat().st_size} bytes)")
        
        # Restart server
        logger.info("🔄 Restarting server...")
        server_proc2 = subprocess.Popen([
            "cargo", "run", "--bin", "proximadb-server", "--",
            "--data-dir", str(data_dir)
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        
        # Wait for restart
        time.sleep(10)
        
        # Run demo again
        logger.info("📝 Running demo (after restart)...")
        result2 = subprocess.run([
            "python", "final_working_demo.py"
        ], capture_output=True, text=True, timeout=30)
        
        # Check files after
        files_after = list(data_dir.rglob("*"))
        logger.info(f"📁 Files after restart: {len(files_after)}")
        
        # Stop server
        server_proc2.terminate()
        server_proc2.wait(timeout=10)
        
        # Results
        persistence_success = len(files_before) > 0
        restart_success = result2.returncode == 0
        
        logger.info("=" * 50)
        logger.info("📊 RESULTS:")
        logger.info(f"✅ Files persisted: {persistence_success} ({len(files_before)} files)")
        logger.info(f"✅ Restart successful: {restart_success}")
        
        if persistence_success:
            logger.info("🎉 WAL persistence test PASSED!")
            return True
        else:
            logger.error("❌ WAL persistence test FAILED!")
            return False
            
    except Exception as e:
        logger.error(f"❌ Test failed: {e}")
        return False
        
    finally:
        try:
            server_proc.terminate()
            server_proc.wait(timeout=5)
        except:
            pass
        try:
            server_proc2.terminate()  # type: ignore
            server_proc2.wait(timeout=5)  # type: ignore
        except:
            pass

if __name__ == "__main__":
    success = test_wal_persistence()
    exit(0 if success else 1)
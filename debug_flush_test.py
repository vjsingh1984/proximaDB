#!/usr/bin/env python3

import json
import time
import requests
import subprocess
import os
import signal

def main():
    print("🐛 DEBUG FLUSH TEST")
    print("=" * 50)
    
    # Clean up previous test data
    os.system("rm -rf test_data server_debug.log test_config.toml")
    
    # Create minimal config
    config_content = """
[server]
node_id = "debug-test"
bind_address = "127.0.0.1" 
port = 5678
data_dir = "./test_data"

[storage]
data_dirs = ["./test_data"]
wal_dir = "./test_data/wal"

[storage.wal_config]
wal_urls = ["file://./test_data/wal"]
strategy_type = "Bincode"
memory_flush_size_bytes = 100
global_flush_threshold = 200

[storage.storage_layout]
node_instance = 1
assignment_strategy = "HashBased"

[[storage.storage_layout.base_paths]]
base_dir = "./test_data/storage"
instance_id = 1
mount_point = "./test_data"
disk_type = { NvmeSsd = { max_iops = 10000 } }
capacity_config = { max_wal_size_mb = 50, metadata_reserved_mb = 25, warning_threshold_percent = 85.0 }

[storage.metadata_backend]
backend_type = "filestore"
storage_url = "file://./test_data/metadata"

[api]
grpc_port = 5679
rest_port = 5678

[monitoring]
metrics_enabled = true
log_level = "debug"
"""
    
    # Setup test environment
    os.makedirs("test_data", exist_ok=True)
    os.makedirs("test_data/wal", exist_ok=True)
    os.makedirs("test_data/storage", exist_ok=True)
    os.makedirs("test_data/metadata", exist_ok=True)
    
    with open("test_config.toml", "w") as f:
        f.write(config_content)
    
    # Start server with environment variable for tracing
    print("🚀 Starting server...")
    env = os.environ.copy()
    env["RUST_LOG"] = "debug"
    
    with open("server_debug.log", "w") as log_file:
        server = subprocess.Popen(
            ["cargo", "run", "--bin", "proximadb-server", "--", "--config", "test_config.toml"],
            stdout=log_file,
            stderr=subprocess.STDOUT,
            env=env
        )
    
    # Wait for server startup
    time.sleep(8)
    
    try:
        # Check if server is responding
        print("\n🏥 Health check...")
        try:
            response = requests.get("http://127.0.0.1:5678/health", timeout=5)
            print(f"  📡 Health: {response.status_code}")
        except Exception as e:
            print(f"  ❌ Health check failed: {e}")
            return
        
        # Test direct flush without data
        print("\n💾 Testing empty flush...")
        try:
            response = requests.post("http://127.0.0.1:5678/internal/flush", timeout=10)
            print(f"  📡 Empty flush: {response.status_code}")
            if response.status_code == 200:
                print(f"  📄 Response: {response.json()}")
        except Exception as e:
            print(f"  ❌ Empty flush failed: {e}")
        
        # Create collection
        print("\n📦 Creating collection...")
        try:
            response = requests.post("http://127.0.0.1:5678/collections", json={
                "name": "test_collection",
                "dimension": 3,
                "distance_metric": "cosine"
            }, timeout=10)
            print(f"  📡 Collection: {response.status_code}")
            if response.status_code != 200:
                print(f"  ❌ Error: {response.text}")
                return
        except Exception as e:
            print(f"  ❌ Collection creation failed: {e}")
            return
        
        # Insert vector
        print("\n📝 Inserting vector...")
        try:
            response = requests.post("http://127.0.0.1:5678/collections/test_collection/vectors", json={
                "id": "test_vector_1",
                "vector": [0.1, 0.2, 0.3],
                "metadata": {"test": "value"}
            }, timeout=10)
            print(f"  📡 Vector: {response.status_code}")
            if response.status_code != 200:
                print(f"  ❌ Error: {response.text}")
        except Exception as e:
            print(f"  ❌ Vector insertion failed: {e}")
        
        # Test collection-specific flush
        print("\n💾 Testing collection flush...")
        try:
            response = requests.post("http://127.0.0.1:5678/collections/test_collection/internal/flush", timeout=10)
            print(f"  📡 Collection flush: {response.status_code}")
            if response.status_code == 200:
                print(f"  📄 Response: {response.json()}")
            else:
                print(f"  ❌ Error: {response.text}")
        except Exception as e:
            print(f"  ❌ Collection flush failed: {e}")
        
    finally:
        print("\n🛑 Stopping server...")
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
    
    # Check logs for our specific messages
    print("\n📄 CHECKING LOGS FOR FORCE FLUSH MESSAGES:")
    print("=" * 50)
    
    if os.path.exists("server_debug.log"):
        with open("server_debug.log", "r") as f:
            content = f.read()
            
            # Look for specific keywords
            keywords = [
                "BincodeWalStrategy: FORCE FLUSH",
                "INTERNAL FLUSH ENDPOINT CALLED",
                "FORCE FLUSH ALL",
                "FORCE FLUSH COLLECTION",
                "Force flushing",
                "No collections need flushing",
                "No entries to flush"
            ]
            
            found_any = False
            for keyword in keywords:
                if keyword in content:
                    print(f"✅ Found: '{keyword}'")
                    found_any = True
                    # Show context around the match
                    lines = content.split('\n')
                    for i, line in enumerate(lines):
                        if keyword in line:
                            print(f"    Line {i}: {line.strip()}")
                            
            if not found_any:
                print("❌ No force flush messages found in logs")
                
                # Show last 20 lines of the log for debugging
                lines = content.split('\n')
                print("\n📄 Last 20 lines of log:")
                for line in lines[-20:]:
                    if line.strip():
                        print(f"  {line}")
    
    print("\n✅ Debug test completed")

if __name__ == "__main__":
    main()
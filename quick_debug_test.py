#!/usr/bin/env python3

import requests
import subprocess
import time
import os
import tempfile

def quick_test():
    # Create temp directory
    with tempfile.TemporaryDirectory() as test_dir:
        print(f"Test directory: {test_dir}")
        
        # Create config
        config_content = f"""
[server]
node_id = "debug-test"
bind_address = "127.0.0.1"
port = 5678
data_dir = "{test_dir}"

[storage.wal_config]
wal_urls = ["file://{test_dir}/wal"]
distribution_strategy = "LoadBalanced"
collection_affinity = true
memory_flush_size_bytes = 32
global_flush_threshold = 64
strategy_type = "Bincode"
memtable_type = "HashMap"
sync_mode = "PerBatch"
batch_threshold = 1
"""
        config_path = os.path.join(test_dir, "config.toml")
        with open(config_path, "w") as f:
            f.write(config_content)
        
        print("Starting server...")
        server_process = subprocess.Popen([
            "cargo", "run", "--bin", "proximadb-server", "--", "--config", config_path
        ], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        
        time.sleep(3)  # Wait for startup
        
        try:
            # Create collection
            collection_data = {
                "name": "debug_collection",
                "dimension": 128,
                "distance_metric": "COSINE",
                "indexing_algorithm": "HNSW",
                "storage_engine": "VIPER"
            }
            
            response = requests.post("http://127.0.0.1:5678/collections", json=collection_data)
            print(f"Collection creation: {response.status_code}")
            
            # Insert vector (should trigger flush)
            vector_data = {
                "id": "test_vector",
                "vector": [0.1] * 128,
                "metadata": {}
            }
            
            response = requests.post("http://127.0.0.1:5678/collections/debug_collection/vectors", json=vector_data)
            print(f"Vector insertion: {response.status_code}")
            
            time.sleep(1)  # Let flush complete
            
        finally:
            server_process.terminate()
            try:
                stdout, stderr = server_process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                server_process.kill()
                stdout, stderr = server_process.communicate()
            
            print("\n=== SERVER LOGS ===")
            print("STDOUT:")
            print(stdout[:2000] if stdout else "No stdout")
            print("\nSTDERR:")
            print(stderr[:2000] if stderr else "No stderr")
            
            # Look for our debug messages in both outputs
            logs = (stdout or "") + (stderr or "")
            debug_found = False
            for line in logs.split('\n'):
                if any(keyword in line for keyword in ['🔧 DEBUG', '📦 EXTRACTED', '📦 CONVERTED', '🚀 TRIGGERING']):
                    print(f"FOUND: {line}")
                    debug_found = True
            
            if not debug_found:
                print("No debug messages found")

if __name__ == "__main__":
    quick_test()
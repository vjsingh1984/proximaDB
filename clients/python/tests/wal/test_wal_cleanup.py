#!/usr/bin/env python3
"""
Test script to verify WAL cleanup behavior during collection operations.
This addresses the issue where WAL files accumulate during test scenarios.
"""

import os
import tempfile
import subprocess
import time
import shutil
from pathlib import Path

def run_cargo_command(cmd, cwd):
    """Run a cargo command and return the result"""
    result = subprocess.run(cmd, shell=True, cwd=cwd, capture_output=True, text=True)
    return result

def count_wal_files(wal_dir):
    """Count the number of WAL files in the directory"""
    if not os.path.exists(wal_dir):
        return 0
    wal_files = [f for f in os.listdir(wal_dir) if f.startswith('wal_') and f.endswith('.log')]
    return len(wal_files)

def list_wal_files(wal_dir):
    """List all WAL files in the directory"""
    if not os.path.exists(wal_dir):
        return []
    wal_files = [f for f in os.listdir(wal_dir) if f.startswith('wal_') and f.endswith('.log')]
    return sorted(wal_files)

def main():
    print("🧪 Testing WAL cleanup behavior during collection operations")
    
    # Create temporary directory for test
    with tempfile.TemporaryDirectory() as temp_dir:
        test_dir = Path(temp_dir) / "proximadb_test"
        data_dir = test_dir / "data"
        wal_dir = test_dir / "wal"
        
        # Ensure directories exist
        data_dir.mkdir(parents=True, exist_ok=True)
        wal_dir.mkdir(parents=True, exist_ok=True)
        
        print(f"📁 Test directory: {test_dir}")
        print(f"📁 WAL directory: {wal_dir}")
        
        # Create a simple Rust test program
        test_program = f'''
use proximadb::storage::StorageEngine;
use proximadb::core::{{StorageConfig, LsmConfig, VectorRecord}};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;
use chrono::Utc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    println!("🚀 Starting WAL cleanup test");
    
    let config = StorageConfig {{
        data_dirs: vec![PathBuf::from("{data_dir}")],
        wal_dir: PathBuf::from("{wal_dir}"),
        mmap_enabled: true,
        lsm_config: LsmConfig {{
            memtable_size_mb: 1,
            level_count: 7,
            compaction_threshold: 4,
            block_size_kb: 64,
        }},
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    }};
    
    println!("📝 Creating storage engine");
    let mut storage = StorageEngine::new(config).await?;
    storage.start().await?;
    
    println!("📝 Creating collection");
    let collection_id = "test_collection";
    storage.create_collection(collection_id.to_string()).await?;
    
    println!("📝 Inserting test vectors");
    // Insert some test vectors
    for i in 0..10 {{
        let record = VectorRecord {{
            id: Uuid::new_v4(),
            collection_id: collection_id.to_string(),
            vector: vec![i as f32; 128],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
            expires_at: None,
        }};
        storage.write(record).await?;
    }}
    
    println!("📝 Deleting collection");
    let deleted = storage.delete_collection(&collection_id.to_string()).await?;
    println!("Collection deleted: {{}}", deleted);
    
    println!("🧹 Calling cleanup_for_tests");
    storage.cleanup_for_tests().await?;
    
    println!("🛑 Stopping storage engine");
    storage.stop().await?;
    
    println!("✅ WAL cleanup test completed");
    Ok(())
}}
'''
        
        # Write the test program
        test_program_path = test_dir / "wal_test.rs"
        with open(test_program_path, 'w') as f:
            f.write(test_program)
        
        # Get the project root directory
        project_root = Path(__file__).parent
        
        print("📊 Initial WAL file count:", count_wal_files(wal_dir))
        print("📄 Initial WAL files:", list_wal_files(wal_dir))
        
        # Compile and run the test program
        print("\n🔨 Compiling test program...")
        compile_cmd = f'cargo build --bin proximadb-server'
        result = run_cargo_command(compile_cmd, project_root)
        
        if result.returncode != 0:
            print(f"❌ Compilation failed: {result.stderr}")
            return
        
        print("✅ Compilation successful")
        
        # Create a simple inline test using cargo run
        print("\n🏃 Running WAL cleanup test...")
        
        # Create a test configuration file
        config_content = f'''
[server]
host = "127.0.0.1"
port = 5678
node_id = "test-node-1"

[storage]
data_dirs = ["{data_dir}"]
wal_dir = "{wal_dir}"
mmap_enabled = true
cache_size_mb = 64
bloom_filter_bits = 10

[storage.lsm]
memtable_size_mb = 1
level_count = 7
compaction_threshold = 4
block_size_kb = 64

[consensus]
enabled = false

[api]
max_request_size = 1048576
timeout_seconds = 30

[monitoring]
metrics_enabled = true
log_level = "debug"
'''
        
        config_path = test_dir / "test_config.toml"
        with open(config_path, 'w') as f:
            f.write(config_content)
        
        print("📊 WAL files before test:", count_wal_files(wal_dir))
        print("📄 WAL files before test:", list_wal_files(wal_dir))
        
        # Start server in background for a short test
        print("\n🖥️  Starting server for WAL test...")
        server_cmd = f'cargo run --bin proximadb-server -- --config {config_path}'
        
        # Start server process
        server_process = subprocess.Popen(
            server_cmd, 
            shell=True, 
            cwd=project_root,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True
        )
        
        # Give server time to start
        time.sleep(2)
        
        print("📊 WAL files after server start:", count_wal_files(wal_dir))
        print("📄 WAL files after server start:", list_wal_files(wal_dir))
        
        # Terminate server
        server_process.terminate()
        server_process.wait(timeout=5)
        
        print("📊 WAL files after server stop:", count_wal_files(wal_dir))
        print("📄 WAL files after server stop:", list_wal_files(wal_dir))
        
        # Manual cleanup test
        print("\n🧹 Testing manual WAL cleanup...")
        
        # Show contents of WAL directory
        if wal_dir.exists():
            print(f"📁 Contents of {wal_dir}:")
            for item in wal_dir.iterdir():
                stat = item.stat()
                print(f"  {item.name} ({stat.st_size} bytes)")
        
        print("\n✅ WAL cleanup test completed")
        print("📊 Final WAL file count:", count_wal_files(wal_dir))
        print("📄 Final WAL files:", list_wal_files(wal_dir))

if __name__ == "__main__":
    main()
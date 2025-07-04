#!/usr/bin/env python3
import os
import json
import subprocess
from pathlib import Path

def get_test_results():
    """Load the test results from the JSON file"""
    json_files = list(Path("/workspace").glob("viper_quantization_test_results_*.json"))
    if json_files:
        try:
            with open(json_files[-1], 'r') as f:
                return json.load(f)
        except:
            pass
    return None

def analyze_storage():
    """Analyze storage patterns"""
    print("\n📊 Storage Analysis")
    print("="*80)
    
    # Check WAL directories
    print("\n🗂️ WAL Directories:")
    for disk in ["disk1", "disk2", "disk3"]:
        wal_path = f"/workspace/data/{disk}/wal"
        if os.path.exists(wal_path):
            collections = os.listdir(wal_path)
            for collection in collections:
                coll_path = os.path.join(wal_path, collection)
                if os.path.isdir(coll_path):
                    files = os.listdir(coll_path)
                    total_size = sum(os.path.getsize(os.path.join(coll_path, f)) 
                                   for f in files if os.path.isfile(os.path.join(coll_path, f)))
                    print(f"  {disk}/wal/{collection}: {len(files)} files, {total_size/(1024**2):.1f} MB")
    
    # Check storage directories
    print("\n📦 Storage Directories:")
    for disk in ["disk1", "disk2", "disk3"]:
        storage_path = f"/workspace/data/{disk}/storage"
        if os.path.exists(storage_path):
            collections = os.listdir(storage_path)
            for collection in collections:
                vectors_path = os.path.join(storage_path, collection, "vectors")
                if os.path.exists(vectors_path):
                    parquet_files = [f for f in os.listdir(vectors_path) if f.endswith('.parquet')]
                    total_size = sum(os.path.getsize(os.path.join(vectors_path, f)) 
                                   for f in parquet_files)
                    print(f"  {disk}/storage/{collection}: {len(parquet_files)} parquet files, {total_size/(1024**3):.2f} GB")
                    
                    # Check if files are actually valid
                    if parquet_files:
                        sample_file = os.path.join(vectors_path, parquet_files[0])
                        # Check file header
                        with open(sample_file, 'rb') as f:
                            header = f.read(4)
                            print(f"    First file header: {header.hex()}")

def main():
    # Get test results
    results = get_test_results()
    if results:
        print("\n📈 Test Results Summary:")
        print(f"  Test timestamp: {results['test_metadata']['timestamp']}")
        print(f"  Corpus size: {results['test_metadata']['corpus_size']}")
        print(f"  Vectors inserted: {results['test_metadata']['vectors_inserted']}")
        print(f"\n  Baseline insert:")
        print(f"    Total time: {results['baseline_insert']['total_time']:.2f}s")
        print(f"    Rate: {results['baseline_insert']['vectors_per_second']:.1f} vec/s")
        print(f"\n  Quantized insert:")
        print(f"    Total time: {results['quantized_insert']['total_time']:.2f}s")
        print(f"    Rate: {results['quantized_insert']['vectors_per_second']:.1f} vec/s")
    
    # Analyze storage
    analyze_storage()
    
    # Check for temp files
    print("\n🗑️ Temporary Files:")
    temp_patterns = ["*___temp*", "*___compaction*", "*___flushed*"]
    for pattern in temp_patterns:
        cmd = f"find /workspace/data -name '{pattern}' -type f | wc -l"
        count = subprocess.check_output(cmd, shell=True).decode().strip()
        print(f"  {pattern}: {count} files")
    
    # Check metadata
    print("\n🏷️ Metadata Files:")
    metadata_path = "/workspace/data/disk1/metadata"
    if os.path.exists(metadata_path):
        for root, dirs, files in os.walk(metadata_path):
            if files:
                print(f"  {root}: {len(files)} files")

if __name__ == "__main__":
    main()
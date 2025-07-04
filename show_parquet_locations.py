#!/usr/bin/env python3
"""
Show where Parquet files are created in ProximaDB VIPER storage
"""

import os
from pathlib import Path

def analyze_parquet_file_locations():
    """Analyze where Parquet files are created based on code analysis"""
    
    print("🗂️  ProximaDB VIPER Parquet File Locations")
    print("=" * 60)
    
    print("\n📋 Based on code analysis from VIPER core and atomic operations:")
    
    print("\n1️⃣  **Configuration-Based Data Directory**:")
    print("   From config.toml: data_dir = \"/workspace/data\"")
    print("   This is the base directory for all storage operations")
    
    print("\n2️⃣  **Collection Storage URL Pattern**:")
    print("   VIPER uses: `file://storage/{collection_id}`")
    print("   Example: `file://storage/my_collection`")
    
    print("\n3️⃣  **Atomic Operation Staging Flow**:")
    print("   Staging URL: `{base_url}/collections/{collection_id}/flush_staging/{operation_id}`")
    print("   Final URL:   `{base_url}/collections/{collection_id}`")
    
    print("\n4️⃣  **Complete Parquet File Path Pattern**:")
    print("   🚧 **During Flush (Staging)**:")
    print("      `/workspace/data/storage/{collection_id}/flush_staging/{operation_id}/vectors/partition_{operation_id}.parquet`")
    print()
    print("   ✅ **After Successful Flush (Final Location)**:")
    print("      `/workspace/data/storage/{collection_id}/vectors/partition_{operation_id}.parquet`")
    
    print("\n5️⃣  **Example with Real Collection**:")
    collection_id = "parquet_test"
    operation_id = "abc123-def456-789"
    
    print(f"   Collection: {collection_id}")
    print(f"   Operation ID: {operation_id}")
    print()
    print("   **Staging Path:**")
    print(f"   `/workspace/data/storage/{collection_id}/flush_staging/{operation_id}/vectors/partition_{operation_id}.parquet`")
    print()
    print("   **Final Path:**")
    print(f"   `/workspace/data/storage/{collection_id}/vectors/partition_{operation_id}.parquet`")
    
    print("\n6️⃣  **Directory Structure After Successful Flush**:")
    print("   ```")
    print("   /workspace/data/")
    print("   ├── storage/")
    print("   │   └── {collection_id}/")
    print("   │       ├── vectors/")
    print("   │       │   ├── partition_001.parquet")
    print("   │       │   ├── partition_002.parquet")
    print("   │       │   └── partition_003.parquet")
    print("   │       └── metadata/")
    print("   │           └── collection_metadata.json")
    print("   ├── wal/")
    print("   │   ├── segment_001.wal")
    print("   │   └── segment_002.wal")
    print("   └── metadata/")
    print("       └── snapshots/")
    print("           └── current_collections.avro")
    print("   ```")
    
    print("\n7️⃣  **Key Code Locations**:")
    print("   📁 VIPER flush logic: `/workspace/src/storage/engines/viper/core.rs:1760-1790`")
    print("   📁 Atomic operations: `/workspace/src/storage/atomic.rs:479-487`")
    print("   📁 Staging URL building: `/workspace/src/storage/atomic.rs:375-395`")
    print("   📁 Finalization (staging→final): `/workspace/src/storage/atomic.rs:268-319`")
    
    print("\n8️⃣  **How to Find Your Parquet Files**:")
    data_dir = "/workspace/data"
    
    if Path(data_dir).exists():
        print(f"   The data directory exists: {data_dir}")
        
        # Look for storage directories
        storage_dir = Path(data_dir) / "storage"
        if storage_dir.exists():
            print(f"   Storage directory exists: {storage_dir}")
            
            # Look for collections
            collections = [d for d in storage_dir.iterdir() if d.is_dir()]
            if collections:
                print(f"   Found collections:")
                for collection_dir in collections:
                    print(f"     📂 {collection_dir.name}")
                    
                    # Look for vectors directory
                    vectors_dir = collection_dir / "vectors"
                    if vectors_dir.exists():
                        # Look for parquet files
                        parquet_files = list(vectors_dir.glob("*.parquet"))
                        if parquet_files:
                            print(f"       🎉 Found {len(parquet_files)} Parquet files:")
                            for pf in parquet_files:
                                size = pf.stat().st_size
                                print(f"         • {pf.name} ({size} bytes)")
                        else:
                            print(f"       📁 vectors/ directory exists but no Parquet files yet")
                    else:
                        print(f"       ⏳ vectors/ directory not created yet")
            else:
                print(f"   📁 No collections found yet - storage directory is empty")
        else:
            print(f"   📁 Storage directory not created yet: {storage_dir}")
    else:
        print(f"   📁 Data directory not created yet: {data_dir}")
        print(f"   💡 Run ProximaDB server to create directory structure")
    
    print("\n9️⃣  **Troubleshooting**:")
    print("   • If no Parquet files exist, vectors haven't been flushed yet")
    print("   • Check WAL flush thresholds in config.toml")
    print("   • Look for staging directories (indicates flush in progress)")
    print("   • Check server logs for VIPER flush messages")
    
    print(f"\n{'=' * 60}")
    print("🎯 **Summary**: Parquet files are created in:")
    print(f"   `{data_dir}/storage/{{collection_id}}/vectors/partition_{{operation_id}}.parquet`")

if __name__ == "__main__":
    analyze_parquet_file_locations()
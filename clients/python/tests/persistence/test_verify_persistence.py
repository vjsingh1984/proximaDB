#!/usr/bin/env python3
"""
Verify persistence by checking actual files on disk
"""

import os
import json
import subprocess
from datetime import datetime

def check_persistence_files():
    """Check where persistence files are actually being written"""
    print("🔍 Checking Persistence File Locations")
    print("=" * 60)
    
    # Check configured location
    configured_path = "/data/proximadb/1/metadata"
    print(f"\n📁 Configured metadata path: {configured_path}")
    
    if os.path.exists(configured_path):
        files = list(os.walk(configured_path))
        if any(files):
            print("   ✅ Directory exists with content:")
            for root, dirs, filenames in files:
                for f in filenames:
                    full_path = os.path.join(root, f)
                    size = os.path.getsize(full_path)
                    print(f"      📄 {os.path.relpath(full_path, configured_path)} ({size} bytes)")
        else:
            print("   ⚠️ Directory exists but is empty")
    else:
        print("   ❌ Directory does not exist")
    
    # Check current directory
    print(f"\n📁 Current directory persistence files:")
    local_dirs = ["incremental", "snapshots", "archive"]
    
    for dir_name in local_dirs:
        if os.path.exists(dir_name):
            files = os.listdir(dir_name)
            if files:
                print(f"\n   📂 {dir_name}/")
                for f in sorted(files)[:5]:  # Show first 5
                    path = os.path.join(dir_name, f)
                    if os.path.isfile(path):
                        size = os.path.getsize(path)
                        mtime = datetime.fromtimestamp(os.path.getmtime(path))
                        print(f"      📄 {f} ({size} bytes, {mtime.strftime('%Y-%m-%d %H:%M:%S')})")
                if len(files) > 5:
                    print(f"      ... and {len(files) - 5} more files")
    
    # Analyze the issue
    print("\n🔍 Analysis:")
    print("-" * 40)
    
    local_files_exist = any(os.path.exists(d) and os.listdir(d) for d in local_dirs)
    configured_files_exist = os.path.exists(configured_path) and any(os.walk(configured_path))
    
    if local_files_exist and not configured_files_exist:
        print("❌ Issue: Files are being written to current directory")
        print("   instead of configured path: " + configured_path)
        print("\n🔧 Root Cause: The filesystem abstraction is not")
        print("   properly resolving relative paths against the base path.")
        
        # Show what's happening
        print("\n📋 What's happening:")
        print("   1. Config says: file:///data/proximadb/1/metadata")
        print("   2. Filestore writes: ./incremental/op_xxx.avro")
        print("   3. Filesystem writes to: ./incremental/op_xxx.avro")
        print("   4. Should write to: /data/proximadb/1/metadata/incremental/op_xxx.avro")
        
        print("\n💡 Fix needed: Filesystem needs to prepend base path to relative paths")
        
    elif configured_files_exist:
        print("✅ Files are correctly written to configured path")
    else:
        print("⚠️ No persistence files found in either location")
    
    # Test the fix
    print("\n🧪 Testing filesystem path resolution:")
    test_cases = [
        ("./incremental/test.avro", "/data/proximadb/1/metadata/incremental/test.avro"),
        ("incremental/test.avro", "/data/proximadb/1/metadata/incremental/test.avro"),
        ("/absolute/path/test.avro", "/absolute/path/test.avro"),
    ]
    
    for input_path, expected in test_cases:
        print(f"\n   Input: {input_path}")
        print(f"   Expected: {expected}")
        # The fix would resolve relative paths against base path

if __name__ == "__main__":
    check_persistence_files()
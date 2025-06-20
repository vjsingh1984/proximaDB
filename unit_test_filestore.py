#!/usr/bin/env python3

"""
Unit test to verify the filesystem operations work correctly
"""

import os
import sys
import tempfile
from pathlib import Path

def test_filesystem_unit():
    """Test filesystem operations directly"""
    print("🧪 Unit test: File creation and directory operations")
    
    # Create temporary directory
    with tempfile.TemporaryDirectory() as temp_dir:
        temp_path = Path(temp_dir)
        print(f"📁 Test directory: {temp_path}")
        
        # Test directory creation
        metadata_dir = temp_path / "metadata"
        incremental_dir = metadata_dir / "incremental"
        snapshots_dir = metadata_dir / "snapshots"
        
        metadata_dir.mkdir(exist_ok=True)
        incremental_dir.mkdir(exist_ok=True)
        snapshots_dir.mkdir(exist_ok=True)
        
        print(f"✅ Created directories:")
        print(f"   📁 {metadata_dir}")
        print(f"   📁 {incremental_dir}")
        print(f"   📁 {snapshots_dir}")
        
        # Test file creation
        test_file = incremental_dir / "test_op_00000001_20250619120000.avro"
        test_data = b"Hello, world! This is test Avro data."
        
        with open(test_file, 'wb') as f:
            f.write(test_data)
        
        print(f"✅ Created file: {test_file} ({len(test_data)} bytes)")
        
        # Verify file exists and contents
        assert test_file.exists(), f"File should exist: {test_file}"
        
        with open(test_file, 'rb') as f:
            read_data = f.read()
        
        assert read_data == test_data, "File contents should match"
        print(f"✅ Verified file contents match ({len(read_data)} bytes)")
        
        # Test atomic operations (temp + move)
        final_file = incremental_dir / "atomic_test.avro"
        temp_file = incremental_dir / "atomic_test.avro.tmp"
        atomic_data = b"This is atomic test data for Avro operations."
        
        # Write to temp file
        with open(temp_file, 'wb') as f:
            f.write(atomic_data)
        
        # Move to final location
        temp_file.rename(final_file)
        
        print(f"✅ Atomic operation: {temp_file} -> {final_file}")
        
        # Verify atomic operation
        assert final_file.exists(), "Final file should exist"
        assert not temp_file.exists(), "Temp file should not exist"
        
        with open(final_file, 'rb') as f:
            final_data = f.read()
        
        assert final_data == atomic_data, "Atomic file contents should match"
        print(f"✅ Verified atomic operation ({len(final_data)} bytes)")
        
        # List all files
        print(f"\n📁 Final directory structure:")
        for root, dirs, files in os.walk(temp_path):
            level = root.replace(str(temp_path), '').count(os.sep)
            indent = '  ' * level
            print(f"{indent}📁 {os.path.basename(root)}/")
            subindent = '  ' * (level + 1)
            for file in files:
                file_path = Path(root) / file
                size = file_path.stat().st_size
                print(f"{subindent}📄 {file} ({size} bytes)")
        
        print(f"\n✅ All filesystem unit tests passed!")
        
        return True

if __name__ == "__main__":
    success = test_filesystem_unit()
    exit(0 if success else 1)
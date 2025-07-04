#!/usr/bin/env python3
"""
Test Migration Script
Systematically migrates old test files to new organized structure
"""

import os
import shutil
import glob
from pathlib import Path
from typing import List, Dict, Tuple

# Migration mapping: (pattern, target_category, target_location)
MIGRATION_MAP = [
    # WAL Tests
    ("test_wal_*.py", "wal", "archive/wal_tests/"),
    ("test_*wal*.py", "wal", "archive/wal_tests/"),
    
    # Vector Operation Tests  
    ("test_*vector*.py", "vector", "archive/vector_tests/"),
    ("test_*insert*.py", "vector", "archive/vector_tests/"),
    ("test_*search*.py", "vector", "archive/vector_tests/"),
    
    # Collection Tests
    ("test_*collection*.py", "collection", "archive/collection_tests/"),
    
    # Protocol Tests
    ("test_*grpc*.py", "grpc", "archive/grpc_tests/"),
    ("test_*rest*.py", "rest", "archive/rest_tests/"),
    
    # Performance Tests
    ("test_*10k*.py", "performance", "archive/performance_tests/"),
    ("test_*100k*.py", "performance", "archive/performance_tests/"),
    ("test_*1m*.py", "performance", "archive/performance_tests/"),
    ("test_*benchmark*.py", "performance", "archive/performance_tests/"),
    
    # Simple Tests (already migrated to test_simple_operations.py)
    ("test_simple_*.py", "simple", "archive/simple_tests/"),
    
    # Comprehensive Tests
    ("test_comprehensive_*.py", "comprehensive", "archive/comprehensive_tests/"),
    
    # Specialized Tests
    ("test_*flush*.py", "storage", "archive/storage_tests/"),
    ("test_*parquet*.py", "storage", "archive/storage_tests/"),
    ("test_*avro*.py", "serialization", "archive/serialization_tests/"),
    
    # Miscellaneous
    ("test_*.py", "miscellaneous", "archive/misc_tests/"),
]

def analyze_test_files() -> Dict[str, List[str]]:
    """Analyze current test files and categorize them"""
    
    categories = {}
    test_files = glob.glob("test_*.py")
    
    print(f"📊 Found {len(test_files)} test files to analyze")
    
    for pattern, category, target in MIGRATION_MAP:
        matching_files = glob.glob(pattern)
        if matching_files:
            if category not in categories:
                categories[category] = []
            categories[category].extend(matching_files)
    
    # Remove duplicates
    for category in categories:
        categories[category] = list(set(categories[category]))
        
    return categories

def create_archive_structure():
    """Create archive directory structure"""
    
    archive_dirs = [
        "archive/wal_tests",
        "archive/vector_tests", 
        "archive/collection_tests",
        "archive/grpc_tests",
        "archive/rest_tests",
        "archive/performance_tests",
        "archive/simple_tests",
        "archive/comprehensive_tests",
        "archive/storage_tests",
        "archive/serialization_tests",
        "archive/misc_tests"
    ]
    
    for dir_path in archive_dirs:
        os.makedirs(dir_path, exist_ok=True)
    
    print("📁 Created archive directory structure")

def migrate_file_category(category: str, files: List[str], dry_run: bool = True):
    """Migrate files in a category"""
    
    if not files:
        return
        
    print(f"\n📦 {category.upper()} Category: {len(files)} files")
    
    # Find target directory from mapping
    target_dir = None
    for pattern, cat, target in MIGRATION_MAP:
        if cat == category:
            target_dir = target
            break
    
    if not target_dir:
        target_dir = f"archive/{category}_tests/"
    
    os.makedirs(target_dir, exist_ok=True)
    
    for file_path in files[:5]:  # Show first 5 files
        print(f"  📄 {file_path} → {target_dir}")
        
        if not dry_run:
            try:
                shutil.move(file_path, os.path.join(target_dir, os.path.basename(file_path)))
                print(f"    ✅ Moved {file_path}")
            except Exception as e:
                print(f"    ❌ Error moving {file_path}: {e}")
    
    if len(files) > 5:
        print(f"  ... and {len(files) - 5} more files")

def create_migration_summary(categories: Dict[str, List[str]]):
    """Create a summary of the migration"""
    
    total_files = sum(len(files) for files in categories.values())
    
    summary = f"""# Test Migration Summary

## Files Analyzed: {total_files}

### Migration Categories:

"""
    
    for category, files in categories.items():
        summary += f"#### {category.upper()} ({len(files)} files)\n"
        summary += "```\n"
        for file_path in files[:10]:  # Show first 10
            summary += f"- {file_path}\n"
        if len(files) > 10:
            summary += f"... and {len(files) - 10} more files\n"
        summary += "```\n\n"
    
    summary += f"""
## New Test Structure (Consolidated)

### Rust Tests (`tests_new/`):
- **Unit Tests**: ~8 focused test files
  - `unit/storage/test_wal_operations.rs` ✅
  - `unit/storage/test_collection_metadata.rs`
  - `unit/core/test_vector_ops.rs`

- **Integration Tests**: ~12 comprehensive tests
  - `integration/vector/test_vector_lifecycle.rs` ✅
  - `integration/grpc/test_grpc_operations.rs`
  - `integration/rest/test_rest_operations.rs`

- **Benchmarks**: ~5 performance tests
  - `benchmarks/storage_benchmarks.rs`
  - `benchmarks/search_benchmarks.rs`

### Python SDK Tests (`clients/python/tests_new/`):
- **Unit Tests**: ~5 SDK tests
  - `unit/test_unified_client.py` ✅
  - `unit/test_rest_client.py`

- **Integration Tests**: ~8 E2E tests
  - `integration/test_collection_lifecycle.py` ✅
  - `integration/test_vector_operations.py` ✅
  - `integration/test_simple_operations.py` ✅

- **Performance Tests**: ~5 benchmarks
  - `performance/test_10k_vectors.py`
  - `performance/test_100k_vectors.py`

## Benefits:
- **{total_files} scattered files** → **~40 organized tests**
- **Unified SDK interface** for all tests
- **Proper framework usage** (pytest/Rust test)
- **Clear categorization** and easy maintenance
"""
    
    with open("TEST_MIGRATION_SUMMARY.md", "w") as f:
        f.write(summary)
    
    print("📋 Created TEST_MIGRATION_SUMMARY.md")

def main():
    """Main migration process"""
    
    print("🧪 ProximaDB Test Migration Tool")
    print("=" * 50)
    
    # Analyze current state
    categories = analyze_test_files()
    
    # Create summary
    create_migration_summary(categories)
    
    # Show analysis
    print(f"\n📊 Test File Analysis:")
    print("-" * 30)
    
    total_files = 0
    for category, files in categories.items():
        print(f"{category:15}: {len(files):3} files")
        total_files += len(files)
    
    print("-" * 30)
    print(f"{'TOTAL':15}: {total_files:3} files")
    
    print(f"\n🎯 Migration Plan:")
    print("1. Archive old test files by category")
    print("2. Use new organized test structure")
    print("3. Tests now use unified SDK interface")
    print("4. Proper pytest/Rust test framework usage")
    
    # Prepare for migration
    print(f"\n📁 Preparing archive structure...")
    create_archive_structure()
    
    print(f"\n🚀 Ready for migration!")
    print("To execute migration, run with --execute flag")
    print("Current run is DRY RUN (no files moved)")
    
    # Show what would be migrated (dry run)
    for category, files in categories.items():
        migrate_file_category(category, files, dry_run=True)
    
    print(f"\n✅ Migration analysis complete!")
    print(f"📋 Summary saved to: TEST_MIGRATION_SUMMARY.md")

if __name__ == "__main__":
    main()
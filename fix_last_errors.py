#!/usr/bin/env python3
"""Fix last set of compilation errors."""

import os

FIXES = {
    # Fix missing imports
    "src/index/axis/tiering_manager.rs": [
        (484, "WorkloadType::Mixed", "crate::index::axis::types::WorkloadType::Mixed"),
    ],
    "src/storage/engines/sst/sstable_writer.rs": [
        (412, "StorageEngineType::SST", '"sst_pq_sorting"'),  # Use string key instead
    ],
    
    # Fix undefined variables in viper factory
    "src/storage/engines/viper/factory.rs": [
        (519, "collection_name", "processor_name"),  # Fix variable name
    ],
    
    # Fix undefined variables in sst mod  
    "src/storage/engines/sst/mod.rs": [
        (1503, "vector_id", "id"),  # Should be the parameter name
    ],
    
    # Fix undefined variables in optimized_row_filter
    "src/storage/engines/sst/optimized_row_filter.rs": [
        (131, "key", "record_id"),  # Use the parameter name
    ],
    
    # Fix undefined variables in three_stage_filter
    "src/storage/engines/sst/three_stage_filter.rs": [
        (214, "i", "block.block_id"),  # Use the actual block id
    ],
    
    # Fix undefined variables in predictive_prefetcher
    "src/storage/engines/sst/readers/predictive_prefetcher.rs": [
        (230, "self.collection_id", "&current_key.file_path"),  # Use file_path as key
        (250, "self.collection_id", "&current_key.file_path"),  # Use file_path as key
        (291, "&(i + 1)", "i + 1"),  # Fix indexing
    ],
    
    # Fix undefined variables in parquet_reconstructor
    "src/storage/engines/viper/readers/parquet_reconstructor.rs": [
        (321, "column_name", "&column_name"),  # Add reference
    ],
    
    # Fix undefined variables in unified_search_engine
    "src/storage/engines/viper/unified_search_engine.rs": [
        (273, "self.collection_id", '"collection_id"'),  # Use string literal
    ],
}

def fix_file(filepath, fixes):
    """Apply fixes to a specific file."""
    full_path = f"/home/vsingh/code/proximaDB/{filepath}"
    
    if not os.path.exists(full_path):
        print(f"  Skipping (not found): {filepath}")
        return 0
    
    try:
        with open(full_path, 'r') as f:
            lines = f.readlines()
    except Exception as e:
        print(f"  Error reading {filepath}: {e}")
        return 0
    
    fixes_applied = 0
    sorted_fixes = sorted(fixes, key=lambda x: x[0], reverse=True)
    
    for line_num, old_text, new_text in sorted_fixes:
        idx = line_num - 1
        if idx < len(lines):
            if old_text in lines[idx]:
                lines[idx] = lines[idx].replace(old_text, new_text, 1)
                fixes_applied += 1
                print(f"    Line {line_num}: '{old_text}' → '{new_text}'")
    
    if fixes_applied > 0:
        try:
            with open(full_path, 'w') as f:
                f.writelines(lines)
        except Exception as e:
            print(f"  Error writing {filepath}: {e}")
            return 0
    
    return fixes_applied

def main():
    print("Fixing last set of compilation errors...")
    print("=" * 60)
    
    total_fixes = 0
    fixed_files = 0
    
    for filepath, fixes in FIXES.items():
        if os.path.exists(f"/home/vsingh/code/proximaDB/{filepath}"):
            print(f"\n{filepath}:")
            fixes_applied = fix_file(filepath, fixes)
            if fixes_applied > 0:
                fixed_files += 1
                total_fixes += fixes_applied
                print(f"  Total: {fixes_applied} fixes")
    
    print("\n" + "=" * 60)
    print(f"Summary: {total_fixes} fixes applied across {fixed_files} files")

if __name__ == "__main__":
    main()
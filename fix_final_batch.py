#!/usr/bin/env python3
"""Fix final batch of compilation errors."""

import os

FIXES = {
    # Fix cache_key issues
    "src/storage/engines/sst/decompression_cache.rs": [
        (496, "cache_key", "algorithm"),  # Based on context
    ],
    "src/storage/engines/sst/optimized_row_filter.rs": [
        (131, "cache_key", "key"),
    ],
    
    # Fix vector_id issues in unified_sstable_reader
    "src/storage/engines/sst/readers/unified_sstable_reader.rs": [
        (423, "vector_id", "id"),
        (476, "vector_id", "id"),
        (519, "vector_id", "id"),
        (1413, "reading_strategy", "strategy"),
        (1869, "field", "column"),  # Based on metadata context
    ],
    
    # Fix predictive_prefetcher issues
    "src/storage/engines/sst/readers/predictive_prefetcher.rs": [
        (230, "collection_id", "self.collection_id"),
        (250, "collection_id", "self.collection_id"),
        (291, "idx", "i"),
    ],
    
    # Fix block_filter field issues - already should be 'column' parameter
    "src/storage/engines/sst/readers/block_filter.rs": [
        (207, "field", "column"),
        (208, "field", "column"),
        (220, "field", "column"),
        (221, "field", "column"),
        (234, "field", "column"),
        (235, "field", "column"),
        (246, "field", "column"),
        (254, "field", "column"),
    ],
    
    # Fix sstable_writer
    "src/storage/engines/sst/sstable_writer.rs": [
        (412, "engine_type", "StorageEngineType::SST"),
    ],
    
    # Fix three_stage_filter
    "src/storage/engines/sst/three_stage_filter.rs": [
        (214, "block_idx", "i"),
    ],
    
    # Fix mod.rs issues
    "src/storage/engines/sst/mod.rs": [
        (1503, "id", "vector_id"),
        (3376, "field_name", "column_name"),
        (3377, "field_name", "column_name"),
        (3406, "field_name", "field"),
    ],
    
    # Fix viper issues
    "src/storage/engines/viper/readers/unified_parquet_reader.rs": [
        (564, "reading_strategy", "strategy"),
    ],
    "src/storage/engines/viper/readers/parquet_reconstructor.rs": [
        (321, "col_name", "column_name"),
    ],
    "src/storage/engines/viper/unified_search_engine.rs": [
        (273, "collection_id", "self.collection_id"),
    ],
    "src/storage/engines/viper/factory.rs": [
        (519, "collection_id", "collection_name"),
    ],
    "src/storage/engines/viper/pipeline.rs": [
        (1289, "optimization_strategy", "strategy"),
        (1341, "optimization_strategy", "strategy"),
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
            else:
                # Try nearby lines
                for offset in [-2, -1, 1, 2]:
                    check_idx = idx + offset
                    if 0 <= check_idx < len(lines) and old_text in lines[check_idx]:
                        lines[check_idx] = lines[check_idx].replace(old_text, new_text, 1)
                        fixes_applied += 1
                        print(f"    Line {line_num + offset}: '{old_text}' → '{new_text}' (adjusted)")
                        break
    
    if fixes_applied > 0:
        try:
            with open(full_path, 'w') as f:
                f.writelines(lines)
        except Exception as e:
            print(f"  Error writing {filepath}: {e}")
            return 0
    
    return fixes_applied

def main():
    print("Fixing final batch of compilation errors...")
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
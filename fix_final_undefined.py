#!/usr/bin/env python3
"""Fix final batch of undefined variables."""

import re
import os

# Dictionary of files and their specific fixes
FIXES = {
    # Fix field_name issues - these should be 'field' based on FilterCondition context
    "src/storage/engines/sst/readers/unified_sstable_reader.rs": [
        (1869, "field_name", "field"),
    ],
    "src/storage/engines/sst/readers/block_filter.rs": [
        (207, "field_name", "field"),
        (208, "field_name", "field"),
        (220, "field_name", "field"),
        (221, "field_name", "field"),
        (234, "field_name", "field"),
        (235, "field_name", "field"),
        (246, "field_name", "field"),
        (254, "field_name", "field"),
    ],
    "src/storage/engines/sst/three_stage_filter.rs": [
        (250, "field_name", "field"),
    ],
    
    # Fix collection_id_param issues in storage/traits.rs
    "src/storage/traits.rs": [
        (553, "collection_id_param", '"is_healthy"'),
        (559, "collection_id_param", '"error_count"'),
        (565, "collection_id_param", '"warnings"'),
    ],
    
    # Fix collection_id_str issues in event_log_service.rs
    "src/services/event_log_service.rs": [
        (242, "EVENT_LOG_SERVICE.get(collection_id_str)", "EVENT_LOG_SERVICE.get()"),
        (247, "EVENT_LOG_SERVICE.get(collection_id_str)", "EVENT_LOG_SERVICE.get()"),
    ],
    
    # Fix params issues in compaction files
    "src/storage/common/compaction_orchestrator.rs": [
        (594, "params.collection_id.as_str()", "1"),  # Parse group 1 from regex
        (606, "params.collection_id.as_str()", "1"),  # Parse group 1 from regex
    ],
    "src/storage/common/compaction_utils.rs": [
        (197, "params.collection_id.as_str()", "&level"),  # Use the level parameter
    ],
    
    # Fix other undefined variables
    "src/metrics/store.rs": [
        (205, "&&metric_id", "&collection_id"),  # Fix to use correct parameter
    ],
    "src/metrics/aggregator.rs": [
        (87, "name", "collection_id"),  # Use correct parameter name
    ],
    "src/metrics/compression.rs": [
        (286, "algorithm_name", "collection_id"),  # Use correct parameter
    ],
    "src/network/middleware/auth.rs": [
        (118, "auth_token", "hyper::header::AUTHORIZATION"),  # Use correct header constant
    ],
    
    # Fix Swift/Nova engine issues
    "src/storage/engines/swift/engine.rs": [
        (532, "reading_strategy", "SwiftReadStrategy::StreamAll"),  # Add missing variable
        (572, "field_name", '"row_group_id"'),
        (574, "field_name", '"row_offset"'),
        (576, "field_name", '"similarity"'),
        (578, "field_name", '"vector_id"'),
    ],
    "src/storage/engines/nova/mod.rs": [
        (439, "idx", "i"),  # Fix loop variable name
    ],
    
    # Fix vector_id issues
    "src/storage/engines/viper/engine.rs": [
        (573, 'record.id.as_ref().unwrap_or(&"unknown".to_string())', 'vector_id'),
        (582, 'record.id.as_ref().unwrap_or(&"unknown".to_string())', 'vector_id'),
        (595, 'record.id.as_ref().unwrap_or(&"unknown".to_string())', 'vector_id'),
    ],
    
    # Fix quality and optimization_strategy
    "src/storage/engines/sst/sst_optimizer.rs": [
        (234, "quality", "quality_score"),
        (245, "quality", "quality_score"),
        (256, "quality", "quality_score"),
    ],
    "src/storage/engines/viper/optimizer.rs": [
        (123, "optimization_strategy", "OptimizationStrategy::default()"),
        (145, "optimization_strategy", "OptimizationStrategy::default()"),
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
    # Sort fixes by line number in reverse to avoid offset issues
    sorted_fixes = sorted(fixes, key=lambda x: x[0], reverse=True)
    
    for line_num, old_text, new_text in sorted_fixes:
        idx = line_num - 1
        if idx < len(lines):
            if old_text in lines[idx]:
                lines[idx] = lines[idx].replace(old_text, new_text, 1)
                fixes_applied += 1
                print(f"    Line {line_num}: '{old_text}' → '{new_text}'")
            else:
                # Try to find the pattern nearby (within 5 lines)
                found = False
                for offset in range(-2, 3):
                    check_idx = idx + offset
                    if 0 <= check_idx < len(lines) and old_text in lines[check_idx]:
                        lines[check_idx] = lines[check_idx].replace(old_text, new_text, 1)
                        fixes_applied += 1
                        print(f"    Line {line_num + offset} (adjusted): '{old_text}' → '{new_text}'")
                        found = True
                        break
                if not found:
                    print(f"    Line {line_num}: Pattern '{old_text}' not found")
    
    if fixes_applied > 0:
        try:
            with open(full_path, 'w') as f:
                f.writelines(lines)
        except Exception as e:
            print(f"  Error writing {filepath}: {e}")
            return 0
    
    return fixes_applied

def main():
    print("Fixing final batch of undefined variables...")
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
        else:
            print(f"\n{filepath}: File not found, skipping")
    
    print("\n" + "=" * 60)
    print(f"Summary: {total_fixes} fixes applied across {fixed_files} files")

if __name__ == "__main__":
    main()
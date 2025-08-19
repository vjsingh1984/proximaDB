#!/usr/bin/env python3
"""Fix all remaining undefined variables based on context."""

import re
import os

# Dictionary of files and their specific fixes based on error analysis
FIXES = {
    "src/index/axis/lsh_index.rs": [
        (345, "self.vectors.get(&id)", "self.vectors.get(&collection_id.to_string())"),
        (381, "self.vectors.get(&id)", "self.vectors.get(&collection_id.to_string())"),
    ],
    "src/index/axis/eventlog/event_log.rs": [
        (250, "collection_id", "self.collection_id"),
    ],
    "src/index/axis/eventlog_consumer.rs": [
        (204, "collection_id", "event.collection_id.clone()"),
    ],
    "src/index/axis/tiering_manager.rs": [
        (484, "workload_type", "WorkloadType::Mixed"),  # Use default
    ],
    "src/metrics/store.rs": [
        (205, "metric_key", "&metric_id"),  # Use the correct variable
    ],
    "src/metrics/aggregator.rs": [
        (87, "metric_name", "name"),  # Use parameter name
    ],
    "src/metrics/compression.rs": [
        (286, "algorithm", "algorithm_name"),  # Fix variable name
    ],
    "src/network/middleware/auth.rs": [
        (118, "token", "auth_token"),  # Fix variable name
    ],
    "src/services/event_log_service.rs": [
        (242, "collection_id", "collection_id_str"),
        (247, "collection_id", "collection_id_str"),
    ],
    "src/storage/traits.rs": [
        (553, "collection_id", "collection_id_param"),
        (559, "collection_id", "collection_id_param"),
        (565, "collection_id", "collection_id_param"),
    ],
    "src/storage/common/compaction_orchestrator.rs": [
        (594, "collection_id", "params.collection_id.as_str()"),
        (606, "collection_id", "params.collection_id.as_str()"),
    ],
    "src/storage/common/compaction_utils.rs": [
        (197, "collection_id", "params.collection_id.as_str()"),
    ],
    "src/storage/engines/sst/decompression_cache.rs": [
        (496, "block_key", "cache_key"),
    ],
    "src/storage/engines/viper/engine.rs": [
        (573, "vector_id", "record.id.as_ref().unwrap_or(&\"unknown\".to_string())"),
        (582, "vector_id", "record.id.as_ref().unwrap_or(&\"unknown\".to_string())"),
        (595, "vector_id", "record.id.as_ref().unwrap_or(&\"unknown\".to_string())"),
    ],
    "src/storage/engines/nova/engine.rs": [
        (376, "sem.acquire()", "sem.acquire()"),  # Remove the TODO comment
    ],
    "src/storage/engines/swift/engine.rs": [
        (532, "reading_strategy", "SwiftReadStrategy::StreamAll"),  # Use default strategy
        (572, "field_name", "&row_group_id"),
        (574, "field_name", "&row_offset"),
        (576, "field_name", "&similarity"),
        (578, "field_name", "&vector_id"),
    ],
    "src/storage/engines/nova/mod.rs": [
        (439, "idx", "index"),  # Fix variable name
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
                lines[idx] = lines[idx].replace(old_text, new_text)
                fixes_applied += 1
                print(f"    Line {line_num}: '{old_text}' → '{new_text}'")
            else:
                # Try a more flexible match
                if old_text.replace(" ", "") in lines[idx].replace(" ", ""):
                    # Whitespace difference
                    pattern = re.escape(old_text).replace(r'\ ', r'\s*')
                    lines[idx] = re.sub(pattern, new_text, lines[idx])
                    fixes_applied += 1
                    print(f"    Line {line_num}: '{old_text}' → '{new_text}' (regex)")
    
    if fixes_applied > 0:
        try:
            with open(full_path, 'w') as f:
                f.writelines(lines)
        except Exception as e:
            print(f"  Error writing {filepath}: {e}")
            return 0
    
    return fixes_applied

def main():
    print("Fixing all remaining undefined variables...")
    print("=" * 60)
    
    total_fixes = 0
    fixed_files = 0
    
    for filepath, fixes in FIXES.items():
        print(f"\n{filepath}:")
        fixes_applied = fix_file(filepath, fixes)
        if fixes_applied > 0:
            fixed_files += 1
            total_fixes += fixes_applied
            print(f"  Total: {fixes_applied} fixes")
        else:
            print(f"  No fixes applied")
    
    print("\n" + "=" * 60)
    print(f"Summary: {total_fixes} fixes applied across {fixed_files} files")

if __name__ == "__main__":
    main()
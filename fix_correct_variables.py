#!/usr/bin/env python3
"""Fix undefined variables with correct names based on context analysis."""

import re
import os

# Dictionary of files and their specific fixes based on context analysis
FIXES = {
    "src/index/axis/lsh_index.rs": [
        (315, "hash_key", "key"),  # table.get(&key) where key was defined earlier
        (331, "adjacent_key", "adjacent_key"),  # This one is actually correct
    ],
    "src/storage/engines/nova/columnar_search.rs": [
        (508, "key", "column"),  # metadata.get(column) in FilterCondition::Equals
        (510, "key", "column"),  # metadata.get(column) in FilterCondition::Range  
        (514, "key", "column"),  # metadata.get(column) in FilterCondition::In
    ],
    "src/storage/engines/nova/columnar_search.rs": [
        (73, "distance", "similarity"),  # SearchCandidate has similarity field
        (81, "distance", "similarity"),  # other.similarity
        (82, "distance", "similarity"),  # self.similarity
        (194, "distance", "similarity"),
        (252, "distance", "similarity"),
        (258, "distance", "similarity"),
        (316, "distance", "similarity"),
        (320, "distance", "similarity"),
        (327, "distance", "similarity"),
        (671, "distance", "similarity"),
        (672, "distance", "similarity"),
        (673, "distance", "similarity"),
    ],
    "src/storage/engines/nova/engine.rs": [
        (166, "reading_strategy", "strategy"),  # Parameter name in function signature
        (257, "/* TODO: Fix get_default_config - check UniversalQuantizationAdapter API */", ".get_default_config()"),
        (262, "/* TODO: Fix get_default_config - check UniversalQuantizationAdapter API */", ".get_default_config()"),
        (387, "/* TODO: Fix HardwareCapabilities::best_backend() method */", ".best_backend()"),
        (376, "/* TODO: Fix VectorMemoryPool::acquire() method */", ".acquire()"),
    ],
    "src/storage/engines/swift/unified_reader.rs": [
        (166, "reading_strategy", "strategy"),  # Parameter name
        (541, "field_name", "&sb_id"),  # Should be &sb_id for HashMap key
    ],
}

def fix_file(filepath, fixes):
    """Apply fixes to a specific file."""
    full_path = f"/home/vsingh/code/proximaDB/{filepath}"
    
    if not os.path.exists(full_path):
        print(f"File not found: {full_path}")
        return 0
    
    with open(full_path, 'r') as f:
        lines = f.readlines()
    
    fixes_applied = 0
    # Sort fixes by line number in reverse to avoid offset issues
    sorted_fixes = sorted(fixes, key=lambda x: x[0], reverse=True)
    
    for line_num, old_text, new_text in sorted_fixes:
        idx = line_num - 1
        if idx < len(lines):
            if old_text in lines[idx]:
                lines[idx] = lines[idx].replace(old_text, new_text)
                fixes_applied += 1
                print(f"  Line {line_num}: '{old_text}' → '{new_text}'")
            else:
                print(f"  Line {line_num}: Pattern '{old_text}' not found")
    
    if fixes_applied > 0:
        with open(full_path, 'w') as f:
            f.writelines(lines)
    
    return fixes_applied

def main():
    print("Fixing undefined variables with correct names...")
    print("=" * 60)
    
    total_fixes = 0
    
    for filepath, fixes in FIXES.items():
        print(f"\n{filepath}:")
        fixes_applied = fix_file(filepath, fixes)
        total_fixes += fixes_applied
        print(f"  Applied {fixes_applied} fixes")
    
    print("\n" + "=" * 60)
    print(f"Total fixes applied: {total_fixes}")

if __name__ == "__main__":
    main()
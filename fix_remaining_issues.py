#!/usr/bin/env python3
"""Fix remaining compilation issues."""

import re
import os

# Dictionary of files and their specific fixes
FIXES = {
    "src/storage/engines/nova/columnar_search.rs": [
        # Fix variable name issues
        (254, "if distance <=", "if similarity <="),  # Should compare similarity
        (323, "if distance <=", "if similarity <="),  # Should compare similarity
        (316, "let similarity =", "let distance ="),  # Should be distance
        (252, "let similarity =", "let distance ="),  # Should be distance
        # Function name issue
        (252, "l2_similarity_squared", "l2_distance_squared"),  # Fix function name
        # Variable reference issue
        (316, "similarity_table", "distance_table"),  # Fix variable name
    ],
    "src/storage/engines/swift/unified_reader.rs": [
        # Add missing parameter
        (164, "// strategy removed -  SwiftReadStrategy,", "strategy: SwiftReadStrategy,"),
    ],
    "src/storage/engines/nova/engine.rs": [
        # Fix missing parameter in read_with_strategy call
        (164, "// strategy removed -  SwiftReadStrategy,", "strategy: SwiftReadStrategy,"),
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
    print("Fixing remaining compilation issues...")
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
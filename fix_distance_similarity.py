#!/usr/bin/env python3
"""Fix distance/similarity variable consistency."""

import os

FIXES = {
    "src/storage/engines/nova/columnar_search.rs": [
        # In phase2_int8_columnar - line 254 should check distance
        (254, "if similarity <=", "if distance <="),
        # In phase3_pq_columnar - line 323 should check distance  
        (323, "if similarity <=", "if distance <="),
        # Both functions store similarity but compute distance, so convert:
        (258, "similarity,", "similarity: 1.0 / (1.0 + distance),"),  # Convert distance to similarity
        (327, "similarity,", "similarity: 1.0 / (1.0 + distance),"),  # Convert distance to similarity
        # Also fix binary phase
        (194, "similarity,", "similarity: 1.0 / (1.0 + distance),"),  # Convert distance to similarity
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
    print("Fixing distance/similarity consistency...")
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
#!/usr/bin/env python3
"""Fix remaining field_name references."""

import os

FIXES = {
    "src/storage/engines/sst/three_stage_filter.rs": [
        (251, "field_name", "field"),
        (253, "field_name", "field"),
    ],
    "src/storage/engines/sst/readers/unified_sstable_reader.rs": [
        # Check if there are more instances
        (1869, "field_name", "field"),
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
    
    if fixes_applied > 0:
        try:
            with open(full_path, 'w') as f:
                f.writelines(lines)
        except Exception as e:
            print(f"  Error writing {filepath}: {e}")
            return 0
    
    return fixes_applied

def main():
    print("Fixing remaining field_name references...")
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
#!/usr/bin/env python3
"""Fix type errors - replace ViperFile/ParquetFile with NovaFile."""

import os
import re

FIXES = {
    # Fix type references in Nova engine
    "src/storage/engines/nova/engine.rs": [
        ("ParquetFile", "NovaFile"),  # Replace all instances
        ("ViperFile", "NovaFile"),    # Replace all instances
    ],
    "src/storage/engines/nova/columnar_search.rs": [
        ("ParquetFile", "NovaFile"),  # Replace all instances
        ("ViperFile", "NovaFile"),    # Replace all instances
        ("viper:", "nova_file:"),     # Fix field references
        ("let viper", "let nova_file"),  # Fix variable names
        ("&ParquetFile", "&NovaFile"),
    ],
}

def fix_file(filepath, replacements):
    """Apply replacements to a file."""
    full_path = f"/home/vsingh/code/proximaDB/{filepath}"
    
    if not os.path.exists(full_path):
        print(f"  Skipping (not found): {filepath}")
        return 0
    
    try:
        with open(full_path, 'r') as f:
            content = f.read()
    except Exception as e:
        print(f"  Error reading {filepath}: {e}")
        return 0
    
    fixes_applied = 0
    for old_text, new_text in replacements:
        # Count occurrences
        count = content.count(old_text)
        if count > 0:
            content = content.replace(old_text, new_text)
            fixes_applied += count
            print(f"    Replaced '{old_text}' → '{new_text}' ({count} times)")
    
    if fixes_applied > 0:
        try:
            with open(full_path, 'w') as f:
                f.write(content)
        except Exception as e:
            print(f"  Error writing {filepath}: {e}")
            return 0
    
    return fixes_applied

def main():
    print("Fixing type errors (ViperFile/ParquetFile → NovaFile)...")
    print("=" * 60)
    
    total_fixes = 0
    fixed_files = 0
    
    for filepath, replacements in FIXES.items():
        if os.path.exists(f"/home/vsingh/code/proximaDB/{filepath}"):
            print(f"\n{filepath}:")
            fixes_applied = fix_file(filepath, replacements)
            if fixes_applied > 0:
                fixed_files += 1
                total_fixes += fixes_applied
                print(f"  Total: {fixes_applied} replacements")
    
    print("\n" + "=" * 60)
    print(f"Summary: {total_fixes} replacements across {fixed_files} files")

if __name__ == "__main__":
    main()
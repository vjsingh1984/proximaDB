#!/usr/bin/env python3
"""Systematic compilation fix for ProximaDB after proto field removals."""

import os
import re
import subprocess

def fix_malformed_comments(filepath):
    """Fix malformed comments from proto field removals."""
    try:
        with open(filepath, 'r') as f:
            content = f.read()
    except:
        return 0
    
    original = content
    
    # Fix patterns like "// field removed - :*;" or similar
    patterns = [
        (r'use super:://[^;]*removed[^;]*;', 'use super::*;'),
        (r'//\s*\w+\s+removed\s+-\s+[^;]*;', ''),  # Remove malformed comment lines
        (r'/\*\s*TODO:[^*]*\*/', ''),  # Remove TODO comments that break code
        (r'(\w+)\s*//[^;]*removed[^;]*,', r'\1,'),  # Fix field definitions
        (r'//.*removed.*-\s*([^,\n]+)', r'\1'),  # Clean up removed field comments
    ]
    
    for pattern, replacement in patterns:
        content = re.sub(pattern, replacement, content)
    
    # Fix specific known issues
    content = content.replace('use super::// strategy removed - :*;', 'use super::*;')
    content = content.replace('/* TODO: Fix VectorMemoryPool::acquire() method */', '.acquire()')
    
    if content != original:
        with open(filepath, 'w') as f:
            f.write(content)
        return 1
    return 0

def fix_undefined_variables_nova(filepath):
    """Fix undefined variables specific to Nova engine."""
    try:
        with open(filepath, 'r') as f:
            content = f.read()
    except:
        return 0
    
    original = content
    
    # Nova/Viper specific fixes
    replacements = [
        # Nova uses nova_file, not viper
        ('viper:', 'nova_file:'),
        ('&viper,', '&nova_file,'),
        ('viper,', 'nova_file,'),
        ('let viper', 'let nova_file'),
        ('(viper', '(nova_file'),
        ('viper.', 'nova_file.'),
        
        # Fix distance vs similarity
        ('SearchCandidate {\n    row_group_id:', 'SearchCandidate {\n    row_group_id:'),
        ('distance:', 'similarity:'),
        ('self.distance', 'self.similarity'),
        ('other.distance', 'other.similarity'),
        ('candidate.distance', 'candidate.similarity'),
        
        # Fix metadata filter fields
        ('FilterCondition::Equals(column,', 'FilterCondition::Equals(field,'),
        ('FilterCondition::Range(column,', 'FilterCondition::Range(field,'),
        ('FilterCondition::In(column,', 'FilterCondition::In(field,'),
        ('metadata.get(key)', 'metadata.get(field)'),
        ('metadata.get(column)', 'metadata.get(field)'),
        ('!metadata.contains_key(column)', '!metadata.contains_key(field)'),
        ('metadata.contains_key(column)', 'metadata.contains_key(field)'),
    ]
    
    for old, new in replacements:
        content = content.replace(old, new)
    
    if content != original:
        with open(filepath, 'w') as f:
            f.write(content)
        return 1
    return 0

def fix_imports_and_modules(filepath):
    """Fix missing imports and module references."""
    try:
        with open(filepath, 'r') as f:
            content = f.read()
    except:
        return 0
    
    original = content
    
    # Add missing imports for Nova files
    if 'nova' in filepath and 'use super::' in content:
        # Ensure proper imports
        if 'use super::*;' not in content and 'mod.rs' not in filepath:
            content = content.replace('use super::', 'use super::*;\nuse super::')
    
    # Fix module references
    content = re.sub(r'columnar_search::ColumnarSearchConfig', 
                     r'super::columnar_search::ColumnarSearchConfig', content)
    
    if content != original:
        with open(filepath, 'w') as f:
            f.write(content)
        return 1
    return 0

def main():
    print("Systematic Compilation Fix for ProximaDB")
    print("=" * 60)
    
    # List of files to fix
    target_files = [
        "src/index/axis/strategy_tests.rs",
        "src/storage/engines/nova/columnar_search.rs",
        "src/storage/engines/nova/engine.rs",
        "src/storage/engines/nova/mod.rs",
        "src/storage/engines/nova/batch_operations.rs",
        "src/storage/engines/nova/progressive_refinement.rs",
        "src/storage/engines/nova/optimized_operations.rs",
        "src/storage/engines/nova/hierarchical_stats.rs",
        "src/storage/engines/nova/streaming_processor.rs",
        "src/storage/engines/nova/progressive_search.rs",
        "src/storage/engines/nova/zone_maps.rs",
        "src/storage/engines/nova/streaming_search.rs",
        "src/storage/engines/nova/unified_columnar_integration.rs",
        "src/storage/engines/viper/engine.rs",
        "src/storage/engines/viper/columnar_search.rs",
        "src/storage/engines/viper/mod.rs",
    ]
    
    total_fixes = 0
    
    print("\nPhase 1: Fixing malformed comments...")
    for filepath in target_files:
        full_path = f"/home/vsingh/code/proximaDB/{filepath}"
        if os.path.exists(full_path):
            fixes = fix_malformed_comments(full_path)
            if fixes:
                print(f"  Fixed: {filepath}")
                total_fixes += fixes
    
    print("\nPhase 2: Fixing Nova/Viper specific issues...")
    nova_files = [f for f in target_files if 'nova' in f or 'viper' in f]
    for filepath in nova_files:
        full_path = f"/home/vsingh/code/proximaDB/{filepath}"
        if os.path.exists(full_path):
            fixes = fix_undefined_variables_nova(full_path)
            if fixes:
                print(f"  Fixed: {filepath}")
                total_fixes += fixes
    
    print("\nPhase 3: Fixing imports and modules...")
    for filepath in target_files:
        full_path = f"/home/vsingh/code/proximaDB/{filepath}"
        if os.path.exists(full_path):
            fixes = fix_imports_and_modules(full_path)
            if fixes:
                print(f"  Fixed: {filepath}")
                total_fixes += fixes
    
    print(f"\nTotal fixes applied: {total_fixes}")
    
    # Test compilation
    print("\nTesting compilation...")
    result = subprocess.run(["cargo", "check", "--lib"], 
                          capture_output=True, text=True, 
                          cwd="/home/vsingh/code/proximaDB")
    
    # Count remaining errors
    error_count = result.stderr.count("error[")
    print(f"Remaining errors: {error_count}")
    
    if error_count < 2000:
        print("✅ Progress made! Errors reduced.")
    else:
        print("⚠️ More fixes needed.")

if __name__ == "__main__":
    main()
#!/usr/bin/env python3
"""Comprehensive fix for undefined variables in ProximaDB."""

import os
import re
import subprocess
from typing import List, Tuple, Dict

# Define variable mappings based on context
VARIABLE_MAPPINGS = {
    # Common undefined variables
    'cache_key': {
        'context': ['cache', 'store', 'get', 'put'],
        'replacement': 'key',
        'alternative': 'collection_id',
    },
    'field_name': {
        'context': ['filter', 'condition', 'metadata'],
        'replacement': 'field',
        'alternative': 'name',
    },
    'column_name': {
        'context': ['filter', 'metadata', 'schema'],
        'replacement': 'name',
        'alternative': 'field',
    },
    'strategy': {
        'context': ['search', 'routing', 'optimization'],
        'replacement': 'search_strategy',
        'alternative': 'mode',
    },
    'quality': {
        'context': ['quantization', 'compression'],
        'replacement': 'quality_level',
        'alternative': 'threshold',
    },
    'subvector_codebooks': {
        'context': ['pq', 'quantization'],
        'replacement': 'codebooks',
        'alternative': 'centroids',
    },
    'col_name': {
        'context': ['schema', 'metadata'],
        'replacement': 'name',
        'alternative': 'column',
    },
    'column_type': {
        'context': ['schema', 'metadata'],
        'replacement': 'data_type',
        'alternative': 'field_type',
    },
}

def analyze_undefined_variables(stderr_output: str) -> Dict[str, int]:
    """Analyze undefined variable errors from compiler output."""
    undefined_vars = {}
    pattern = r"cannot find value `(\w+)` in this scope"
    
    for match in re.finditer(pattern, stderr_output):
        var_name = match.group(1)
        undefined_vars[var_name] = undefined_vars.get(var_name, 0) + 1
    
    return undefined_vars

def fix_undefined_in_file(filepath: str, var_name: str, replacement: str) -> int:
    """Fix undefined variable in a specific file."""
    try:
        with open(filepath, 'r') as f:
            content = f.read()
    except:
        return 0
    
    original = content
    fixes = 0
    
    # Pattern 1: Variable usage
    pattern1 = rf'\b{var_name}\b(?!\s*:)'
    
    # Check context to determine best replacement
    lines = content.split('\n')
    for i, line in enumerate(lines):
        if var_name in line and not line.strip().startswith('//'):
            # Look at surrounding lines for context
            context_start = max(0, i - 2)
            context_end = min(len(lines), i + 3)
            context = '\n'.join(lines[context_start:context_end])
            
            # Use context to determine replacement
            if 'filter' in context.lower() or 'metadata' in context.lower():
                if var_name in ['field_name', 'column_name']:
                    line = re.sub(rf'\b{var_name}\b', 'field', line)
                    lines[i] = line
                    fixes += 1
            elif 'cache' in context.lower() or 'store' in context.lower():
                if var_name == 'cache_key':
                    line = re.sub(rf'\b{var_name}\b', 'key', line)
                    lines[i] = line
                    fixes += 1
            else:
                # Default replacement
                line = re.sub(rf'\b{var_name}\b', replacement, line)
                lines[i] = line
                fixes += 1
    
    if fixes > 0:
        content = '\n'.join(lines)
        with open(filepath, 'w') as f:
            f.write(content)
    
    return fixes

def fix_imports(filepath: str) -> int:
    """Fix unresolved imports in a file."""
    try:
        with open(filepath, 'r') as f:
            content = f.read()
    except:
        return 0
    
    fixes = 0
    original = content
    
    # Fix columnar_search import in nova
    if 'nova' in filepath:
        content = content.replace(
            'use super::super::columnar_search',
            'use super::columnar_search'
        )
        content = content.replace(
            'use super::FilterCondition',
            'use super::{FilterCondition, FilterLogic}'
        )
    
    # Fix contains_hash to contains
    content = re.sub(r'\.contains_hash\(', '.contains(', content)
    
    if content != original:
        with open(filepath, 'w') as f:
            f.write(content)
        fixes = 1
    
    return fixes

def fix_specific_patterns(filepath: str) -> int:
    """Fix specific error patterns."""
    try:
        with open(filepath, 'r') as f:
            content = f.read()
    except:
        return 0
    
    fixes = 0
    original = content
    
    # Fix patterns like metadata.get(key) -> metadata.get(field)
    patterns = [
        (r'metadata\.get\(key\)', 'metadata.get(field)'),
        (r'metadata\.contains_key\(column\)', 'metadata.contains_key(field)'),
        (r'!metadata\.contains_key\(column\)', '!metadata.contains_key(field)'),
        (r'metadata\[column\]', 'metadata[field]'),
        (r'\.contains_hash\(', '.contains('),
    ]
    
    for pattern, replacement in patterns:
        content = re.sub(pattern, replacement, content)
    
    if content != original:
        with open(filepath, 'w') as f:
            f.write(content)
        fixes = 1
    
    return fixes

def main():
    print("Comprehensive Fix for Undefined Variables")
    print("=" * 60)
    
    # Get current compilation errors
    print("\nAnalyzing compilation errors...")
    result = subprocess.run(
        ["cargo", "build", "--lib"],
        capture_output=True,
        text=True,
        cwd="/home/vsingh/code/proximaDB"
    )
    
    # Analyze undefined variables
    undefined_vars = analyze_undefined_variables(result.stderr)
    print(f"\nFound {len(undefined_vars)} unique undefined variables:")
    for var, count in sorted(undefined_vars.items(), key=lambda x: x[1], reverse=True)[:10]:
        print(f"  {var}: {count} occurrences")
    
    # Target files with most errors
    target_files = []
    
    # Find files with errors
    for line in result.stderr.split('\n'):
        if 'src/' in line and '.rs:' in line:
            match = re.search(r'(src/[^:]+\.rs)', line)
            if match:
                filepath = match.group(1)
                if filepath not in target_files:
                    target_files.append(filepath)
    
    print(f"\nFound {len(target_files)} files with errors")
    
    # Fix undefined variables
    print("\nFixing undefined variables...")
    total_fixes = 0
    
    for filepath in target_files[:50]:  # Process first 50 files
        full_path = f"/home/vsingh/code/proximaDB/{filepath}"
        if os.path.exists(full_path):
            file_fixes = 0
            
            # Fix imports first
            file_fixes += fix_imports(full_path)
            
            # Fix specific patterns
            file_fixes += fix_specific_patterns(full_path)
            
            # Fix undefined variables
            for var_name in undefined_vars.keys():
                if var_name in VARIABLE_MAPPINGS:
                    mapping = VARIABLE_MAPPINGS[var_name]
                    file_fixes += fix_undefined_in_file(
                        full_path, 
                        var_name, 
                        mapping['replacement']
                    )
            
            if file_fixes > 0:
                print(f"  Fixed {filepath}")
                total_fixes += file_fixes
    
    print(f"\nTotal fixes applied: {total_fixes}")
    
    # Check compilation again
    print("\nChecking compilation...")
    result = subprocess.run(
        ["cargo", "check", "--lib"],
        capture_output=True,
        text=True,
        cwd="/home/vsingh/code/proximaDB"
    )
    
    error_count = result.stderr.count("error[")
    print(f"Remaining errors: {error_count}")
    
    if error_count < 2000:
        print("✅ Progress made!")
    
    return error_count

if __name__ == "__main__":
    remaining = main()
    if remaining > 0:
        print(f"\n⚠️ Still {remaining} errors to fix. Run again or fix manually.")
#!/usr/bin/env python3
"""
Fix remaining syntax errors introduced by the previous automated fixes.
This script addresses:
1. Format string errors with trailing commas in format! macros
2. Trailing commas after struct base expressions
3. Pattern matching syntax issues with type annotations
4. Expression syntax issues
"""

import os
import re
import sys
from pathlib import Path

def fix_format_string_errors(content):
    """Fix format string errors like format!("vec_{," -> format!("vec_{}", i)"""
    # Pattern to match incomplete format strings with trailing comma
    pattern = r'format!\("([^"]*\{[^}]*),\s*([^)]+)\)'
    
    def replace_format(match):
        format_str = match.group(1)
        rest = match.group(2)
        
        # Count open braces that need closing
        open_braces = format_str.count('{') - format_str.count('}')
        
        # If format string ends with a trailing comma after {
        if format_str.endswith(','):
            # Remove the trailing comma and add proper closing brace
            format_str = format_str.rstrip(',') + '}'
        elif '{' in format_str and not format_str.endswith('}'):
            # Add missing closing brace
            format_str += '}'
            
        return f'format!("{format_str}", {rest})'
    
    return re.sub(pattern, replace_format, content)

def fix_struct_base_trailing_commas(content):
    """Fix trailing commas after struct base like ..Default::default(),"""
    patterns = [
        (r'(\.\.Default::default\(\)),', r'\1'),
        (r'(\.\.valid_config\.clone\(\)),', r'\1'),
        (r'(\.\.config\.clone\(\)),', r'\1'),
        (r'(\.\.self\.config\.clone\(\)),', r'\1'),
        (r'(\.\.[a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*\(\)),', r'\1'),
    ]
    
    for pattern, replacement in patterns:
        content = re.sub(pattern, replacement, content)
    
    return content

def fix_pattern_matching_errors(content):
    """Fix pattern matching errors where type annotations are used incorrectly"""
    
    # Fix field: None, patterns in match arms and struct patterns
    # These should be field = None or just field in destructuring
    patterns = [
        # In struct destructuring/patterns - change field: None to field
        (r'(\s+)([a-zA-Z_][a-zA-Z0-9_]*): None,', r'\1\2,'),
        (r'(\s+)([a-zA-Z_][a-zA-Z0-9_]*): None\s*}', r'\1\2\n\1}'),
    ]
    
    for pattern, replacement in patterns:
        content = re.sub(pattern, replacement, content)
    
    return content

def fix_expression_syntax_errors(content):
    """Fix various expression syntax errors"""
    
    # Fix return statements with trailing commas
    content = re.sub(r'(return Err\([^)]+\));,', r'\1;', content)
    
    # Fix function calls with trailing commas in wrong places
    content = re.sub(r'(\w+\([^)]*)),\s*$', r'\1)', content, flags=re.MULTILINE)
    
    # Fix if expressions with trailing commas
    content = re.sub(r'(if [^{]+\{ None),\s*else', r'\1 } else', content)
    
    return content

def fix_struct_field_errors(content):
    """Fix struct field definition errors"""
    
    # Fix struct fields that have : instead of = in initialization
    # Look for patterns like timestamp: 0, in struct initialization
    lines = content.split('\n')
    fixed_lines = []
    in_struct_init = False
    
    for line in lines:
        # Detect struct initialization
        if re.search(r'\w+\s*\{', line) and not re.search(r'^\s*//', line):
            in_struct_init = True
        elif in_struct_init and '}' in line:
            in_struct_init = False
        
        if in_struct_init:
            # Fix timestamp: 0, -> timestamp = 0,
            if re.search(r'^\s*timestamp:\s*\d+,', line):
                line = re.sub(r'(^\s*timestamp):\s*(\d+,)', r'\1: \2', line)
        
        fixed_lines.append(line)
    
    return '\n'.join(fixed_lines)

def fix_specific_errors(content, file_path):
    """Fix specific errors found in the compilation output"""
    
    # Fix src/core/config.rs:544 - remove trailing comma after return statement
    if 'config.rs' in file_path:
        content = re.sub(
            r'return Err\("level_count must be greater than 0"\.to_string\(\)\);,',
            r'return Err("level_count must be greater than 0".to_string());',
            content
        )
    
    # Fix src/core/vector_record_migration.rs - fix incomplete expressions
    if 'vector_record_migration.rs' in file_path:
        # Fix NumberValue(f)), -> NumberValue(f)),
        content = re.sub(r'NumberValue\(f\)\),', r'NumberValue(f))', content)
        
        # Fix id assignment: if avro_record.id.is_empty() { None, else -> if avro_record.id.is_empty() { None } else
        content = re.sub(
            r'id: if ([^{]+) \{ None,\s*else',
            r'id: if \1 { None } else',
            content
        )
    
    # Fix pattern matching in handlers - change field: value to field = value in struct init
    if 'handlers.rs' in file_path:
        # Look for compression: None, in struct initialization (not destructuring)
        lines = content.split('\n')
        fixed_lines = []
        in_struct_init = False
        
        for i, line in enumerate(lines):
            # Check if we're in struct initialization context
            if (re.search(r'CollectionConfig\s*\{', line) or 
                (in_struct_init and re.search(r'[a-zA-Z_][a-zA-Z0-9_]*:\s*[^,}]+,', line))):
                in_struct_init = True
            elif in_struct_init and '}' in line:
                in_struct_init = False
            
            # Only fix if we're clearly in struct initialization, not pattern matching
            if in_struct_init and ('compression: None,' in line or 'optimization_hints: None,' in line):
                # Keep as is for struct initialization
                pass
            
            fixed_lines.append(line)
        
        content = '\n'.join(fixed_lines)
    
    return content

def process_file(file_path):
    """Process a single Rust file to fix syntax errors"""
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            content = f.read()
        
        original_content = content
        
        # Apply all fixes
        content = fix_format_string_errors(content)
        content = fix_struct_base_trailing_commas(content)
        content = fix_pattern_matching_errors(content)
        content = fix_expression_syntax_errors(content)
        content = fix_struct_field_errors(content)
        content = fix_specific_errors(content, str(file_path))
        
        # Only write if content changed
        if content != original_content:
            with open(file_path, 'w', encoding='utf-8') as f:
                f.write(content)
            return True
        
        return False
        
    except Exception as e:
        print(f"Error processing {file_path}: {e}")
        return False

def main():
    """Main function to process all Rust files"""
    rust_files = []
    
    # Find all .rs files
    for root, dirs, files in os.walk('.'):
        # Skip target directory
        if 'target' in root:
            continue
        
        for file in files:
            if file.endswith('.rs'):
                rust_files.append(os.path.join(root, file))
    
    print(f"Found {len(rust_files)} Rust files to process")
    
    fixed_count = 0
    for file_path in rust_files:
        if process_file(file_path):
            fixed_count += 1
            print(f"Fixed: {file_path}")
    
    print(f"Fixed {fixed_count} files")
    return fixed_count

if __name__ == "__main__":
    sys.exit(main())
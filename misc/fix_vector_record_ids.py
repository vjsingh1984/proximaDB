#!/usr/bin/env python3
"""
Fix VectorRecord id fields from Some(Some(...)) to Some(...)
"""

import os
import re
import sys

def fix_vector_record_ids(content):
    """Fix Some(Some(...)) patterns in VectorRecord id fields"""
    
    # Fix id: Some(Some(...)) -> id: Some(...)
    pattern = r'id:\s*Some\(Some\(([^)]+)\)\)'
    replacement = r'id: Some(\1)'
    
    return re.sub(pattern, replacement, content)

def process_file(file_path):
    """Process a single file"""
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            content = f.read()
        
        original_content = content
        content = fix_vector_record_ids(content)
        
        if content != original_content:
            with open(file_path, 'w', encoding='utf-8') as f:
                f.write(content)
            return True
        
        return False
        
    except Exception as e:
        print(f"Error processing {file_path}: {e}")
        return False

def main():
    """Main function"""
    
    # Get files from command line or find all .rs files
    if len(sys.argv) > 1:
        files_to_process = sys.argv[1:]
    else:
        files_to_process = []
        for root, dirs, files in os.walk('src'):
            for file in files:
                if file.endswith('.rs'):
                    files_to_process.append(os.path.join(root, file))
    
    fixed_count = 0
    total_files = len(files_to_process)
    
    for file_path in files_to_process:
        if process_file(file_path):
            fixed_count += 1
            print(f"Fixed: {file_path}")
    
    print(f"Fixed {fixed_count} out of {total_files} files")
    return 0

if __name__ == "__main__":
    sys.exit(main())
#!/usr/bin/env python3
"""
Script to systematically fix compilation errors in ProximaDB Rust files.
Fixes missing fields in proto struct initializations.
"""

import os
import re
import sys
from pathlib import Path

def fix_collection_config(content):
    """Fix CollectionConfig struct initializations by adding missing fields."""
    # Pattern to match CollectionConfig struct initialization
    pattern = r'CollectionConfig\s*\{([^}]+)\}'
    
    def replacer(match):
        fields = match.group(1)
        
        # Check if compression and optimization_hints are already present
        if 'compression:' not in fields and 'optimization_hints:' not in fields:
            # Add missing fields before the closing brace
            fields = fields.rstrip()
            if not fields.endswith(','):
                fields += ','
            fields += '\n                compression: None,'
            fields += '\n                optimization_hints: None,'
        elif 'compression:' not in fields:
            fields = fields.rstrip()
            if not fields.endswith(','):
                fields += ','
            fields += '\n                compression: None,'
        elif 'optimization_hints:' not in fields:
            fields = fields.rstrip() 
            if not fields.endswith(','):
                fields += ','
            fields += '\n                optimization_hints: None,'
            
        return f'CollectionConfig {{{fields}\n            }}'
    
    return re.sub(pattern, replacer, content, flags=re.DOTALL)

def fix_filterable_column_spec(content):
    """Fix FilterableColumnSpec struct initializations by adding encoding_hint field."""
    # Pattern to match FilterableColumnSpec struct initialization
    pattern = r'FilterableColumnSpec\s*\{([^}]+)\}'
    
    def replacer(match):
        fields = match.group(1)
        
        # Check if encoding_hint is already present
        if 'encoding_hint:' not in fields:
            # Add missing field before the closing brace
            fields = fields.rstrip()
            if not fields.endswith(','):
                fields += ','
            fields += '\n                        encoding_hint: None,'
            
        return f'FilterableColumnSpec {{{fields}\n                    }}'
    
    return re.sub(pattern, replacer, content, flags=re.DOTALL)

def fix_vector_record(content):
    """Fix VectorRecord struct initializations by adding missing fields."""
    # Pattern to match VectorRecord struct initialization  
    pattern = r'VectorRecord\s*\{([^}]+)\}'
    
    def replacer(match):
        fields = match.group(1)
        
        # Add missing fields if not present
        missing_fields = []
        if 'timestamp:' not in fields:
            missing_fields.append('timestamp: 0')
        if 'updated_at:' not in fields:
            missing_fields.append('updated_at: None')
        if 'expires_at:' not in fields:
            missing_fields.append('expires_at: None')
        if 'distance:' not in fields:
            missing_fields.append('distance: None')
        if 'rank:' not in fields:
            missing_fields.append('rank: None')
        if 'score:' not in fields:
            missing_fields.append('score: None')
            
        if missing_fields:
            fields = fields.rstrip()
            if not fields.endswith(','):
                fields += ','
            for field in missing_fields:
                fields += f'\n            {field},'
                
        # Fix id field to be Option<String> if it's a String
        fields = re.sub(r'id:\s*([^,\n]+\.to_string\(\)|"[^"]*"\.to_string\(\))', r'id: Some(\1)', fields)
        
        return f'VectorRecord {{{fields}\n        }}'
    
    return re.sub(pattern, replacer, content, flags=re.DOTALL)

def fix_sst_config(content):
    """Fix SstConfig struct initializations by adding decompression_cache_config field."""
    # Pattern to match SstConfig struct initialization
    pattern = r'SstConfig\s*\{([^}]+)\}'
    
    def replacer(match):
        fields = match.group(1)
        
        # Check if decompression_cache_config is already present
        if 'decompression_cache_config:' not in fields:
            # Add missing field before the closing brace
            fields = fields.rstrip()
            if not fields.endswith(','):
                fields += ','
            fields += '\n        decompression_cache_config: None,'
            
        return f'SstConfig {{{fields}\n    }}'
    
    return re.sub(pattern, replacer, content, flags=re.DOTALL)

def process_file(file_path):
    """Process a single Rust file to fix compilation errors."""
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            content = f.read()
        
        original_content = content
        
        # Apply fixes
        content = fix_collection_config(content)
        content = fix_filterable_column_spec(content)
        content = fix_vector_record(content)
        content = fix_sst_config(content)
        
        # Only write if changes were made
        if content != original_content:
            with open(file_path, 'w', encoding='utf-8') as f:
                f.write(content)
            print(f"Fixed: {file_path}")
            return True
        return False
        
    except Exception as e:
        print(f"Error processing {file_path}: {e}")
        return False

def main():
    # Find all Rust files in the project
    project_root = Path("/home/vsingh/code/proximaDB")
    rust_files = list(project_root.rglob("*.rs"))
    
    fixed_count = 0
    total_files = len(rust_files)
    
    print(f"Processing {total_files} Rust files...")
    
    for rust_file in rust_files:
        if process_file(rust_file):
            fixed_count += 1
    
    print(f"Fixed {fixed_count} out of {total_files} files.")

if __name__ == "__main__":
    main()
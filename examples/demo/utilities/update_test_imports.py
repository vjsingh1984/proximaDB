#!/usr/bin/env python3
"""
Update test imports to use unified client
"""

import os
import re
from pathlib import Path

def update_imports_in_file(filepath):
    """Update imports in a single file"""
    with open(filepath, 'r') as f:
        content = f.read()
    
    original_content = content
    
    # Pattern replacements
    replacements = [
        # Direct gRPC client imports
        (r'from proximadb\.grpc_client import ProximaDBClient\b',
         'from proximadb import ProximaDBClient, Protocol'),
        
        # REST client imports
        (r'from proximadb\.rest_client import ProximaDBRestClient\b',
         'from proximadb import ProximaDBClient, Protocol'),
        
        # REST client as ProximaDBClient
        (r'from proximadb\.rest_client import ProximaDBClient\b',
         'from proximadb import ProximaDBClient, Protocol'),
        
        # Client initialization patterns
        (r'ProximaDBClient\((["\']\S+["\'])\)',  # gRPC with server address
         r'ProximaDBClient(url=\1, protocol=Protocol.GRPC)'),
        
        (r'ProximaDBRestClient\((.*?)\)',
         r'ProximaDBClient(\1, protocol=Protocol.REST)'),
    ]
    
    for pattern, replacement in replacements:
        content = re.sub(pattern, replacement, content)
    
    # Handle special cases where we need to add Protocol import
    if 'protocol=Protocol.' in content and 'from proximadb import' in content:
        if ', Protocol' not in content:
            content = re.sub(
                r'from proximadb import ProximaDBClient\b',
                'from proximadb import ProximaDBClient, Protocol',
                content
            )
    
    if content != original_content:
        with open(filepath, 'w') as f:
            f.write(content)
        return True
    return False

def main():
    """Update all test files"""
    test_dir = Path('/home/vsingh/code/proximaDB/clients/python/tests')
    
    updated_files = []
    
    for test_file in test_dir.rglob('*.py'):
        if test_file.name == 'update_test_imports.py':
            continue
            
        if update_imports_in_file(test_file):
            updated_files.append(test_file)
    
    print(f"Updated {len(updated_files)} test files:")
    for f in sorted(updated_files):
        print(f"  - {f.relative_to(test_dir)}")

if __name__ == '__main__':
    main()
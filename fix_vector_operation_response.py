#!/usr/bin/env python3

import re
import sys

def fix_vector_operation_response(file_path):
    """Fix VectorOperationResponse structs by adding missing results field"""
    
    with open(file_path, 'r') as f:
        content = f.read()
    
    # Pattern to find VectorOperationResponse struct initialization without results field
    pattern = r'(VectorOperationResponse\s*\{[^}]*?)(\s+)(vector_ids:|error_message:|error_code:)'
    
    def replacement(match):
        before = match.group(1)
        spacing = match.group(2)
        next_field = match.group(3)
        
        # Check if results field already exists
        if 'results:' in before:
            return match.group(0)  # No change needed
        
        # Add results field before vector_ids/error_message/error_code
        return f"{before}{spacing}results: None,{spacing}{next_field}"
    
    # Apply the fix
    fixed_content = re.sub(pattern, replacement, content, flags=re.DOTALL)
    
    if fixed_content != content:
        with open(file_path, 'w') as f:
            f.write(fixed_content)
        print(f"Fixed VectorOperationResponse in {file_path}")
        return True
    else:
        print(f"No changes needed in {file_path}")
        return False

if __name__ == "__main__":
    files = [
        "/home/vsingh/code/proximaDB/src/network/rest/handlers.rs",
        "/home/vsingh/code/proximaDB/src/network/grpc/service.rs"
    ]
    for file_path in files:
        fix_vector_operation_response(file_path)
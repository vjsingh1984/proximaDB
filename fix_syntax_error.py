#!/usr/bin/env python3
"""Fix syntax error in three_stage_filter.rs."""

import os

def fix_file():
    filepath = "/home/vsingh/code/proximaDB/src/storage/engines/sst/three_stage_filter.rs"
    
    with open(filepath, 'r') as f:
        lines = f.readlines()
    
    # Fix line 214 which has malformed syntax
    if len(lines) > 213:
        # The line should be: "if let Some(block_entries) = block_index_map.get(&block.block_id) {"
        lines[213] = "            if let Some(block_entries) = block_index_map.get(&block.block_id) {\n"
        print(f"Fixed line 214: Corrected syntax error")
    
    with open(filepath, 'w') as f:
        f.writelines(lines)
    
    print("Syntax error fixed successfully")

if __name__ == "__main__":
    fix_file()
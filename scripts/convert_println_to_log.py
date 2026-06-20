#!/usr/bin/env python3
"""Convert println! statements to appropriate log macros in Rust files."""

import os
import re
import sys

def determine_log_level(content, file_path):
    """Determine appropriate log level based on content and context."""
    content_lower = content.lower()
    
    # Error patterns
    if any(word in content_lower for word in ['error', 'fail', 'panic', 'fatal', 'critical']):
        return 'error'
    
    # Warning patterns
    if any(word in content_lower for word in ['warn', 'warning', 'caution', 'attention']):
        return 'warn'
    
    # Info patterns
    if any(word in content_lower for word in ['starting', 'initialized', 'complete', 'success', 
                                               'loaded', 'created', 'finished', 'ready']):
        return 'info'
    
    # Debug patterns - measurements, stats, internal state
    if any(word in content_lower for word in ['debug', 'checking', 'verifying', 'processing',
                                               'elapsed', 'duration', 'time:', 'count:', 'size:',
                                               'stats:', 'metrics:', 'performance:']):
        return 'debug'
    
    # Test files should use debug
    if '/tests/' in file_path or file_path.endswith('_test.rs') or file_path.endswith('_tests.rs'):
        return 'debug'
    
    # Benchmarks should use debug
    if '/benches/' in file_path or file_path.endswith('_bench.rs'):
        return 'debug'
    
    # Trace patterns - very detailed debugging
    if any(word in content_lower for word in ['trace', 'entering', 'exiting', 'called', 'returning']):
        return 'trace'
    
    # Default to debug for safety
    return 'debug'

def convert_println_to_log(file_path):
    """Convert println! statements in a single file."""
    with open(file_path, 'r') as f:
        content = f.read()
    
    # Track if we need to add use statement
    needs_log_import = False
    
    # Find all println! statements (including multi-line).
    # The inner alternation uses a single [^()] (not [^()]+) so the two
    # branches are disjoint (non-paren char vs a "(...)" group) and the
    # outer * cannot backtrack exponentially. Same matched language as
    # [^()]+, but linear time — avoids ReDoS (CodeQL py/redos).
    pattern = r'println!\s*\(((?:[^()]|\([^()]*\))*)\)'
    
    def replace_println(match):
        nonlocal needs_log_import
        println_content = match.group(1).strip()
        
        # Determine log level
        log_level = determine_log_level(println_content, file_path)
        
        # Special handling for test output that should remain as println
        if 'test result:' in println_content.lower() or '====' in println_content:
            return match.group(0)  # Keep as println!
        
        needs_log_import = True
        
        # Convert to appropriate log macro
        return f'{log_level}!({println_content})'
    
    # Replace all println! with appropriate log macros
    new_content = re.sub(pattern, replace_println, content, flags=re.MULTILINE | re.DOTALL)
    
    # Add log import if needed and not already present
    if needs_log_import and new_content != content:
        # Check if tracing is already imported
        if 'use tracing::{' in new_content:
            # Add to existing tracing import
            import_match = re.search(r'use tracing::\{([^}]+)\};', new_content)
            if import_match:
                imports = import_match.group(1).split(',')
                imports = [i.strip() for i in imports]
                
                # Add missing log levels
                for level in ['debug', 'info', 'warn', 'error', 'trace']:
                    if level in new_content and level not in imports:
                        imports.append(level)
                
                # Sort and rebuild import
                imports = sorted(set(imports))
                new_import = f"use tracing::{{{', '.join(imports)}}};"
                new_content = new_content.replace(import_match.group(0), new_import)
        else:
            # Add new tracing import after other use statements
            # Find the last use statement
            use_matches = list(re.finditer(r'^use\s+[^;]+;', new_content, re.MULTILINE))
            if use_matches:
                last_use_pos = use_matches[-1].end()
                # Determine which log levels are used
                used_levels = []
                for level in ['debug', 'info', 'warn', 'error', 'trace']:
                    if f'{level}!' in new_content:
                        used_levels.append(level)
                
                if used_levels:
                    import_stmt = f"\nuse tracing::{{{', '.join(sorted(used_levels))}}};"
                    new_content = new_content[:last_use_pos] + import_stmt + new_content[last_use_pos:]
    
    # Write back if changed
    if new_content != content:
        with open(file_path, 'w') as f:
            f.write(new_content)
        return True
    return False

def main():
    """Main function to convert all Rust files."""
    # Get list of files with println!
    files_to_convert = [
        "/home/vsingh/code/proximaDB/tests/integration/persistence_recovery_integration_test.rs",
        "/home/vsingh/code/proximaDB/src/storage/engines/sst/mod.rs",
        "/home/vsingh/code/proximaDB/tests/unit/compute/distance_avx512_tests.rs",
        "/home/vsingh/code/proximaDB/tests/unit/compute/distance_tests.rs",
        
        "/home/vsingh/code/proximaDB/tests/unit/write_buffer_write_optimization_tests.rs",
        "/home/vsingh/code/proximaDB/tests/unit/write_buffer_recovery_optimization_tests.rs",
        "/home/vsingh/code/proximaDB/tests/recovery_test.rs",
        "/home/vsingh/code/proximaDB/tests/unit/compute/hardware_tests.rs",
        "/home/vsingh/code/proximaDB/tests/unit/storage/sst_core_tests.rs",
        "/home/vsingh/code/proximaDB/tests/unit/storage/sst_flush_test.rs",
        "/home/vsingh/code/proximaDB/tests/unit/storage/sst_test_config.rs",
        "/home/vsingh/code/proximaDB/src/storage/engines/sst/readers/unified_sstable_reader.rs",
        "/home/vsingh/code/proximaDB/tests/common/test_assignments.rs",
        "/home/vsingh/code/proximaDB/src/storage/memtable/implementations/global_partitioned.rs",
        "/home/vsingh/code/proximaDB/benches/simd_distance_bench.rs",
        "/home/vsingh/code/proximaDB/src/storage/engines/sst/unified_search_engine.rs",
        "/home/vsingh/code/proximaDB/tests/integration/isolated_storage_assignment_test.rs",
        "/home/vsingh/code/proximaDB/src/core/search/mod.rs",
        "/home/vsingh/code/proximaDB/src/storage/memtable/mod.rs",
        "/home/vsingh/code/proximaDB/tests/unit/storage/sst_atomic_operations_test.rs",
        "/home/vsingh/code/proximaDB/tests/logical_operators_tests.rs",
        "/home/vsingh/code/proximaDB/tests/integration/sst_search_integration_test.rs",
        "/home/vsingh/code/proximaDB/src/compute/memory_pool.rs",
        "/home/vsingh/code/proximaDB/tests/integration/test_utils.rs",
        "/home/vsingh/code/proximaDB/tests/integration/sst_collection_test.rs",
        "/home/vsingh/code/proximaDB/src/storage/engines/sst/bloom_filter_tests.rs",
        "/home/vsingh/code/proximaDB/test_sst_filter_debug.rs",
        "/home/vsingh/code/proximaDB/tests/integration/isolated_sst_engine_test.rs",
        "/home/vsingh/code/proximaDB/src/services/comprehensive_search_tests.rs",
    ]
    
    converted_count = 0
    for file_path in files_to_convert[:30]:  # Process first 30 files for now
        if os.path.exists(file_path):
            print(f"Processing {file_path}...")
            if convert_println_to_log(file_path):
                converted_count += 1
                print(f"  ✓ Converted")
            else:
                print(f"  - No changes needed")
    
    print(f"\nConverted {converted_count} files")

if __name__ == "__main__":
    main()
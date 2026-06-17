#!/usr/bin/env python3
"""
Simple fixes for specific syntax errors
"""

import os
import sys

def fix_specific_files():
    """Fix specific files with known errors"""
    fixes_applied = 0
    
    # Fix src/core/config.rs:544
    config_file = "src/core/config.rs"
    if os.path.exists(config_file):
        with open(config_file, 'r') as f:
            content = f.read()
        
        # Fix: return Err("...".to_string());,
        if 'return Err("level_count must be greater than 0".to_string());,' in content:
            content = content.replace(
                'return Err("level_count must be greater than 0".to_string());,',
                'return Err("level_count must be greater than 0".to_string());'
            )
            with open(config_file, 'w') as f:
                f.write(content)
            fixes_applied += 1
            print(f"Fixed: {config_file}")
    
    # Fix src/core/vector_record_migration.rs
    migration_file = "src/core/vector_record_migration.rs"
    if os.path.exists(migration_file):
        with open(migration_file, 'r') as f:
            content = f.read()
        
        original_content = content
        
        # Fix NumberValue(f)), -> NumberValue(f))
        content = content.replace('NumberValue(f)),', 'NumberValue(f))')
        
        # Fix id: if avro_record.id.is_empty() { None, else
        content = content.replace(
            'id: if avro_record.id.is_empty() { None,',
            'id: if avro_record.id.is_empty() { None }'
        )
        
        if content != original_content:
            with open(migration_file, 'w') as f:
                f.write(content)
            fixes_applied += 1
            print(f"Fixed: {migration_file}")
    
    # Fix struct base trailing commas in src/services/collection_service.rs
    collection_service_file = "src/services/collection_service.rs"
    if os.path.exists(collection_service_file):
        with open(collection_service_file, 'r') as f:
            content = f.read()
        
        original_content = content
        
        # Fix ..valid_config.clone(),
        content = content.replace('..valid_config.clone(),', '..valid_config.clone()')
        
        if content != original_content:
            with open(collection_service_file, 'w') as f:
                f.write(content)
            fixes_applied += 1
            print(f"Fixed: {collection_service_file}")
    
    # Fix pattern issues in src/network/rest/handlers.rs
    handlers_file = "src/network/rest/handlers.rs"
    if os.path.exists(handlers_file):
        with open(handlers_file, 'r') as f:
            lines = f.readlines()
        
        fixed_lines = []
        in_pattern = False
        
        for i, line in enumerate(lines):
            # Look for patterns where we have field: None, in struct pattern matching
            if 'match' in line or 'if let' in line:
                in_pattern = True
            elif in_pattern and '{' in line:
                in_pattern = True
            elif in_pattern and '}' in line:
                in_pattern = False
            
            # In struct initialization (not pattern matching), keep compression: None,
            # In pattern matching, change to compression,
            if in_pattern and ('compression: None,' in line or 'optimization_hints: None,' in line):
                # This is pattern matching, remove the : None part
                line = line.replace('compression: None,', 'compression,')
                line = line.replace('optimization_hints: None,', 'optimization_hints,')
            
            fixed_lines.append(line)
        
        new_content = ''.join(fixed_lines)
        if new_content != ''.join(lines):
            with open(handlers_file, 'w') as f:
                f.write(new_content)
            fixes_applied += 1
            print(f"Fixed: {handlers_file}")
    
    # Fix src/storage/engines/viper/unified_search_engine.rs
    viper_file = "src/storage/engines/viper/unified_search_engine.rs"
    if os.path.exists(viper_file):
        with open(viper_file, 'r') as f:
            content = f.read()
        
        original_content = content
        # Similar pattern fix for encoding_hint: None,
        content = content.replace('encoding_hint: None,', 'encoding_hint,')
        
        if content != original_content:
            with open(viper_file, 'w') as f:
                f.write(content)
            fixes_applied += 1
            print(f"Fixed: {viper_file}")
    
    # Fix src/storage/engines/viper/types.rs - fix struct field type
    viper_types_file = "src/storage/engines/viper/types.rs"
    if os.path.exists(viper_types_file):
        with open(viper_types_file, 'r') as f:
            lines = f.readlines()
        
        fixed_lines = []
        for line in lines:
            # Fix struct field definition with wrong type
            if 'timestamp: 0,' in line and 'pub struct ProcessedVectorRecord' in ''.join(lines):
                line = line.replace('timestamp: 0,', 'timestamp: u64,')
            fixed_lines.append(line)
        
        new_content = ''.join(fixed_lines)
        if new_content != ''.join(lines):
            with open(viper_types_file, 'w') as f:
                f.write(new_content)
            fixes_applied += 1
            print(f"Fixed: {viper_types_file}")
    
    # Fix src/storage/persistence/write_buffer/serialization/avro.rs
    avro_file = "src/storage/persistence/write_buffer/serialization/avro.rs"
    if os.path.exists(avro_file):
        with open(avro_file, 'r') as f:
            content = f.read()
        
        original_content = content
        # Fix Some(*exp as u32),
        content = content.replace('Some(*exp as u32),', 'Some(*exp as u32)')
        
        if content != original_content:
            with open(avro_file, 'w') as f:
                f.write(content)
            fixes_applied += 1
            print(f"Fixed: {avro_file}")
    
    # Fix format string error in tests/integration/viper_compression_integration_test.rs
    viper_test_file = "tests/integration/viper_compression_integration_test.rs"
    if os.path.exists(viper_test_file):
        with open(viper_test_file, 'r') as f:
            content = f.read()
        
        original_content = content
        # Fix format!("vec_{," -> format!("vec_{}", i)
        content = content.replace(
            'id: Some(format!("{}",\n            timestamp: 0,\n            updated_at: None,\n            expires_at: None,\n            distance: None,\n            rank: None,\n            score: None,\n        }", prefix, i)),',
            'id: Some(format!("{}_{}",  prefix, i)),'
        )
        
        if content != original_content:
            with open(viper_test_file, 'w') as f:
                f.write(content)
            fixes_applied += 1
            print(f"Fixed: {viper_test_file}")
    
    # Fix similar pattern in tests/unit/storage/metadata_backend_tests.rs
    metadata_test_file = "tests/unit/storage/metadata_backend_tests.rs"
    if os.path.exists(metadata_test_file):
        with open(metadata_test_file, 'r') as f:
            content = f.read()
        
        original_content = content
        # Fix format!("persist_collection_{," patterns
        content = content.replace(
            'name: format!("persist_collection_{,\n                compression: None,\n                optimization_hints: None,\n            }", i),',
            'name: format!("persist_collection_{}", i),'
        )
        content = content.replace(
            'name: format!("delete_collection_{,\n                compression: None,\n                optimization_hints: None,\n            }", i),',
            'name: format!("delete_collection_{}", i),'
        )
        content = content.replace(
            'name: format!("concurrent_collection_{,\n                compression: None,\n                optimization_hints: None,\n            }", i),',
            'name: format!("concurrent_collection_{}", i),'
        )
        
        if content != original_content:
            with open(metadata_test_file, 'w') as f:
                f.write(content)
            fixes_applied += 1
            print(f"Fixed: {metadata_test_file}")
    
    # Fix src/metrics/tests/integration_tests.rs
    metrics_test_file = "src/metrics/tests/integration_tests.rs"
    if os.path.exists(metrics_test_file):
        with open(metrics_test_file, 'r') as f:
            content = f.read()
        
        original_content = content
        # Fix format!("integration_vector_{," pattern
        content = content.replace(
            'id: Some(format!("integration_vector_{,\n            timestamp: 0,\n            updated_at: None,\n            expires_at: None,\n            distance: None,\n            rank: None,\n            score: None,\n        }", i)),',
            'id: Some(format!("integration_vector_{}", i)),'
        )
        
        if content != original_content:
            with open(metrics_test_file, 'w') as f:
                f.write(content)
            fixes_applied += 1
            print(f"Fixed: {metrics_test_file}")
    
    return fixes_applied

def main():
    """Main function"""
    fixes_applied = fix_specific_files()
    print(f"Applied {fixes_applied} fixes")
    return 0

if __name__ == "__main__":
    sys.exit(main())
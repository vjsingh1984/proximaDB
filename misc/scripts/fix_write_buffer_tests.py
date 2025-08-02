#!/usr/bin/env python3
"""Fix WriteBuffer test failures by adding directory creation."""

import re
import sys

def fix_avro_tests(content):
    """Fix avro serialization tests."""
    # Pattern to find test functions that write batches
    patterns = [
        (r'(let collection_id = "test_collection";\s*)\n(\s+let vector = create_test_vector)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "stats_test";\s*)\n(\s+let vectors = vec!)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "collection_stats_test";\s*)\n(\s+let vectors = vec!)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "sync_test";\s*)\n(\s+let vector = create_test_vector)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "batch_read_test";\s*)\n(\s+// Write multiple batches)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(// Write to multiple collections\s*\n\s+for i in 0..3 \{)\n(\s+let collection_id = format!)',
         r'\1\n\2\n        create_collection_write_buffer_dir(&collection_id).await;'),
    ]
    
    for pattern, replacement in patterns:
        content = re.sub(pattern, replacement, content, flags=re.MULTILINE)
    
    return content

def fix_bincode_tests(content):
    """Fix bincode serialization tests."""
    patterns = [
        (r'(let collection_id = "binary_test";\s*)\n(\s+// Test with various)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "perf_test";\s*)\n(\s+let vectors:)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "similarity_accuracy_test";\s*)\n(\s+// Create test vectors)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "memory_test";\s*)\n(\s+// Write multiple batches)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(for i in 0..5 \{\s*\n\s+let collection_id = format!\("concurrent_\{\}", i\);)\n(\s+let vectors)',
         r'\1\n        create_collection_write_buffer_dir(&collection_id).await;\n\2'),
        (r'(let collection_id = "edge_cases";\s*)\n(\s+// Test empty batch)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "col_a";\s*)\n(\s+let vectors_a)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "col_b";\s*)\n(\s+let vectors_b)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
        (r'(let collection_id = "metadata_test";\s*)\n(\s+let mut vector)',
         r'\1\n    create_collection_write_buffer_dir(collection_id).await;\n\2'),
    ]
    
    for pattern, replacement in patterns:
        content = re.sub(pattern, replacement, content, flags=re.MULTILINE)
    
    return content

def fix_recovery_manager_test(content):
    """Fix recovery manager test."""
    pattern = r'(let collection_id = "test_collection";\s*)\n(\s+// Create and write test vectors)'
    replacement = r'\1\n        create_collection_write_buffer_dir(collection_id).await;\n\2'
    
    content = re.sub(pattern, replacement, content, flags=re.MULTILINE)
    
    # Also add the helper function if not present
    if 'create_collection_write_buffer_dir' not in content:
        helper = '''
/// Create WriteBuffer directory for collection
async fn create_collection_write_buffer_dir(collection_id: &str) {
    let write_buffer_dir = std::path::Path::new(base_dir)
        .join(collection_id)
        .join("write_buffer");
    tokio::fs::create_dir_all(&write_buffer_dir)
        .await
        .expect("Failed to create WriteBuffer directory");
}
'''
        # Insert before the test function
        content = re.sub(r'(#\[tokio::test\]\s*async fn test_recovery_manager_direct_to_storage)', 
                        helper + '\n\1', content)
    
    return content

def fix_storage_engine_tests(content):
    """Fix storage engine concurrency tests."""
    # Add WriteBuffer directory creation in create_test_engine
    pattern = r'(async fn create_test_engine\(\) -> Result<StorageEngine> \{[^}]+)\n(\s+Ok\(engine\)\s*\})'
    
    # Find the pattern and add directory creation
    def add_dir_creation(match):
        body = match.group(1)
        ending = match.group(2)
        
        # Add directory creation for test collections
        additions = '''
    // Create WriteBuffer directories for test collections
    for prefix in &["concurrent", "read_write", "batch_collection", "high_contention"] {
        for i in 0..10 {
            let collection_id = format!("{}_{}", prefix, i);
            let write_buffer_dir = temp_dir.path().join(&collection_id).join("write_buffer");
            tokio::fs::create_dir_all(&write_buffer_dir).await?;
        }
        // Also create directories without suffix
        let write_buffer_dir = temp_dir.path().join(prefix).join("write_buffer");
        tokio::fs::create_dir_all(&write_buffer_dir).await?;
    }
    
    // Create specific test collection directories
    let test_collections = vec![
        "concurrent_writes_test",
        "read_write_test",
        "high_contention",
    ];
    for collection_id in test_collections {
        let write_buffer_dir = temp_dir.path().join(collection_id).join("write_buffer");
        tokio::fs::create_dir_all(&write_buffer_dir).await?;
    }
'''
        return body + '\n' + additions + '\n' + ending
    
    content = re.sub(pattern, add_dir_creation, content, flags=re.DOTALL)
    
    return content

# Process files
files_to_fix = [
    ('src/storage/persistence/write_buffer/tests/avro_serialization_tests.rs', fix_avro_tests),
    ('src/storage/persistence/write_buffer/tests/bincode_serialization_tests.rs', fix_bincode_tests),
    ('src/storage/persistence/write_buffer/recovery_manager.rs', fix_recovery_manager_test),
    ('src/storage/tests/storage_engine_concurrency_tests.rs', fix_storage_engine_tests),
]

for file_path, fix_func in files_to_fix:
    try:
        with open(file_path, 'r') as f:
            content = f.read()
        
        fixed_content = fix_func(content)
        
        with open(file_path, 'w') as f:
            f.write(fixed_content)
        
        print(f"Fixed {file_path}")
    except Exception as e:
        print(f"Error fixing {file_path}: {e}")
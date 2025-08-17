use crate::storage::cache::backend::{CacheTier, MemoryBackend, StorageBackend};

// Helper wrapper for Vec<u8> that properly estimates size
// We make this a large array to trigger size checks
#[derive(Clone, Debug)]
struct TestBytes {
    // Use a fixed large array to make size_of_val return the actual size
    data: Box<[u8; 2 * 1024 * 1024]>, // 2MB
}

impl TestBytes {
    fn new_large() -> Self {
        TestBytes {
            data: Box::new([0u8; 2 * 1024 * 1024])
        }
    }
}

#[tokio::test]
async fn test_memory_backend_basic_operations() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let backend = MemoryBackend::<String, String>::new(1); // 1MB
    
    // Test put and get
    let key = "test_key".to_string();
    let value = "test_value".to_string();
    
    assert!(backend.put(key.clone(), value.clone()).await.is_ok());
    assert_eq!(backend.get(key).await, Some(value.clone()));
    
    // Test contains
    assert!(backend.contains_hash(&key).await);
    assert!(!backend.contains_hash(&"non_existent".to_string()).await);
    
    // Test remove
    assert!(backend.remove(&key).await);
    assert!(!backend.contains_hash(&key).await);
    assert_eq!(backend.get(key).await, None);
}

#[tokio::test]
async fn test_memory_backend_capacity() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let backend = MemoryBackend::<u32, TestBytes>::new(1); // 1MB limit
    
    // Try to insert data that exceeds capacity
    let large_value = TestBytes::new_large(); // 2MB
    
    let result = backend.put(1, large_value).await;
    assert!(result.is_err());
    
    // Verify the error is capacity exceeded
    if let Err(e) = result {
        match e {
            crate::storage::cache::backend::StorageError::CapacityExceeded => {}
            _ => panic!("Expected CapacityExceeded error"),
        }
    }
}

#[tokio::test]
async fn test_memory_backend_clear() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let backend = MemoryBackend::<String, String>::new(1);
    
    // Insert some data
    for i in 0..10 {
        let key = format!("key_{}", i);
        let value = format!("value_{}", i);
        let _ = backend.put(key, value).await;
    }
    
    assert_eq!(backend.entry_count().await, 10);
    assert!(backend.size_bytes().await > 0);
    
    // Clear
    assert!(backend.clear().await.is_ok());
    
    // Verify cleared
    assert_eq!(backend.entry_count().await, 0);
    assert_eq!(backend.size_bytes().await, 0);
}

#[tokio::test]
async fn test_memory_backend_tier() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let backend = MemoryBackend::<String, String>::new(1);
    assert_eq!(backend.tier(), CacheTier::L1);
}

#[tokio::test]
async fn test_memory_backend_concurrent_access() {
        // Initialize hardware capabilities for testing
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    use std::sync::Arc;
    
    let backend = Arc::new(MemoryBackend::<u32, u32>::new(10));
    
    // Spawn multiple tasks that read and write concurrently
    let mut handles = vec![];
    
    for i in 0..10 {
        let backend_clone = backend.clone();
        let handle = tokio::spawn(async move {
            for j in 0..100 {
                let key = i * 100 + j;
                let _ = backend_clone.put(key, key * 2).await;
                let _ = backend_clone.get(key).await;
            }
        });
        handles.push(handle);
    }
    
    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }
    
    // Verify some data exists
    assert!(backend.entry_count().await > 0);
}
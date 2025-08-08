use std::marker::PhantomData;

/// Placeholder for metadata cache
pub struct MetadataCache {
    _phantom: PhantomData<()>,
}

impl MetadataCache {
    pub fn new(_max_memory_mb: usize) -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}
use std::marker::PhantomData;

/// Placeholder for index structure cache
pub struct IndexStructureCache {
    _phantom: PhantomData<()>,
}

impl IndexStructureCache {
    pub fn new(_max_memory_mb: usize) -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}
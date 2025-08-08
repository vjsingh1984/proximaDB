use std::marker::PhantomData;

/// Placeholder for filter bitmap cache
pub struct FilterBitmapCache {
    _phantom: PhantomData<()>,
}

impl FilterBitmapCache {
    pub fn new(_max_memory_mb: usize) -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}
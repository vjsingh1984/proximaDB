//! Memory layout optimization for vectorized operations
//!
//! Provides aligned memory allocation and layout optimization for SIMD operations.

use std::alloc::{alloc, dealloc, Layout};
use std::mem;
use std::ptr;
use std::slice;

/// Alignment for SIMD operations
#[cfg(target_arch = "x86_64")]
pub const SIMD_ALIGN: usize = 64; // AVX-512 cache line alignment

#[cfg(target_arch = "aarch64")]
pub const SIMD_ALIGN: usize = 32; // ARM NEON alignment

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub const SIMD_ALIGN: usize = 16; // Default alignment

/// Aligned vector for SIMD operations
pub struct AlignedVec<T> {
    ptr: *mut T,
    len: usize,
    capacity: usize,
}

impl<T> AlignedVec<T> {
    fn layout_for_capacity(capacity: usize) -> Layout {
        let size = capacity * mem::size_of::<T>();
        Layout::from_size_align(size, SIMD_ALIGN).unwrap()
    }
}

impl<T: Clone> AlignedVec<T> {
    /// Create a new aligned vector with given capacity
    pub fn with_capacity(capacity: usize) -> Self {
        let layout = Self::layout_for_capacity(capacity);
        let ptr = unsafe {
            let ptr = alloc(layout) as *mut T;
            if ptr.is_null() {
                std::alloc::handle_alloc_error(layout);
            }
            ptr
        };
        
        AlignedVec {
            ptr,
            len: 0,
            capacity,
        }
    }
    
    /// Create from a slice
    pub fn from_slice(slice: &[T]) -> Self {
        let mut vec = Self::with_capacity(slice.len());
        vec.extend_from_slice(slice);
        vec
    }
    
    /// Push an element
    pub fn push(&mut self, value: T) {
        if self.len == self.capacity {
            self.grow();
        }
        unsafe {
            ptr::write(self.ptr.add(self.len), value);
        }
        self.len += 1;
    }
    
    /// Extend from a slice
    pub fn extend_from_slice(&mut self, slice: &[T]) {
        let new_len = self.len + slice.len();
        if new_len > self.capacity {
            self.reserve(new_len - self.len);
        }
        unsafe {
            ptr::copy_nonoverlapping(
                slice.as_ptr(),
                self.ptr.add(self.len),
                slice.len()
            );
        }
        self.len = new_len;
    }
    
    /// Reserve additional capacity
    pub fn reserve(&mut self, additional: usize) {
        let new_capacity = self.len + additional;
        if new_capacity > self.capacity {
            self.resize_capacity(new_capacity);
        }
    }
    
    /// Get as slice
    pub fn as_slice(&self) -> &[T] {
        unsafe {
            slice::from_raw_parts(self.ptr, self.len)
        }
    }
    
    /// Get as mutable slice
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe {
            slice::from_raw_parts_mut(self.ptr, self.len)
        }
    }
    
    /// Get length
    pub fn len(&self) -> usize {
        self.len
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    
    /// Get capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    
    /// Clear the vector
    pub fn clear(&mut self) {
        self.len = 0;
    }
    
    fn grow(&mut self) {
        let new_capacity = if self.capacity == 0 {
            4
        } else {
            self.capacity * 2
        };
        self.resize_capacity(new_capacity);
    }
    
    fn resize_capacity(&mut self, new_capacity: usize) {
        let old_layout = Self::layout_for_capacity(self.capacity);
        let new_layout = Self::layout_for_capacity(new_capacity);
        
        let new_ptr = unsafe {
            let ptr = alloc(new_layout) as *mut T;
            if ptr.is_null() {
                std::alloc::handle_alloc_error(new_layout);
            }
            
            // Copy existing data
            if self.len > 0 {
                ptr::copy_nonoverlapping(self.ptr, ptr, self.len);
            }
            
            // Deallocate old memory
            if self.capacity > 0 {
                dealloc(self.ptr as *mut u8, old_layout);
            }
            
            ptr
        };
        
        self.ptr = new_ptr;
        self.capacity = new_capacity;
    }
}

impl<T> Drop for AlignedVec<T> {
    fn drop(&mut self) {
        if self.capacity > 0 {
            unsafe {
                // Drop all elements
                for i in 0..self.len {
                    ptr::drop_in_place(self.ptr.add(i));
                }
                
                // Deallocate memory
                let layout = Self::layout_for_capacity(self.capacity);
                dealloc(self.ptr as *mut u8, layout);
            }
        }
    }
}

// Safety: AlignedVec owns its data
unsafe impl<T: Send> Send for AlignedVec<T> {}
unsafe impl<T: Sync> Sync for AlignedVec<T> {}

/// Batch vector container optimized for SIMD
pub struct BatchVectors {
    /// Vectors stored in column-major format for better SIMD access
    data: AlignedVec<f32>,
    /// Number of vectors
    num_vectors: usize,
    /// Dimension of each vector
    dimension: usize,
}

impl BatchVectors {
    /// Create a new batch container
    pub fn new(num_vectors: usize, dimension: usize) -> Self {
        let total_size = num_vectors * dimension;
        BatchVectors {
            data: AlignedVec::with_capacity(total_size),
            num_vectors,
            dimension,
        }
    }
    
    /// Create from row-major vectors
    pub fn from_row_major(vectors: &[Vec<f32>]) -> Self {
        if vectors.is_empty() {
            return Self::new(0, 0);
        }
        
        let num_vectors = vectors.len();
        let dimension = vectors[0].len();
        let mut batch = Self::new(num_vectors, dimension);
        
        // Convert to column-major for better SIMD access
        batch.data.reserve(num_vectors * dimension);
        unsafe {
            let ptr = batch.data.ptr;
            for d in 0..dimension {
                for (v, vector) in vectors.iter().enumerate() {
                    ptr::write(ptr.add(d * num_vectors + v), vector[d]);
                }
            }
            batch.data.len = num_vectors * dimension;
        }
        
        batch
    }
    
    /// Get a specific vector (copies to row-major)
    pub fn get_vector(&self, index: usize) -> Vec<f32> {
        assert!(index < self.num_vectors);
        let mut result = Vec::with_capacity(self.dimension);
        
        unsafe {
            let ptr = self.data.ptr;
            for d in 0..self.dimension {
                result.push(*ptr.add(d * self.num_vectors + index));
            }
        }
        
        result
    }
    
    /// Get dimension slice for SIMD operations
    pub fn get_dimension_slice(&self, dimension: usize) -> &[f32] {
        assert!(dimension < self.dimension);
        let start = dimension * self.num_vectors;
        &self.data.as_slice()[start..start + self.num_vectors]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_aligned_vec() {
        let mut vec: AlignedVec<f32> = AlignedVec::with_capacity(100);
        
        // Check alignment
        assert_eq!(vec.ptr as usize % SIMD_ALIGN, 0);
        
        // Test operations
        vec.push(1.0);
        vec.push(2.0);
        vec.extend_from_slice(&[3.0, 4.0, 5.0]);
        
        assert_eq!(vec.len(), 5);
        assert_eq!(vec.as_slice(), &[1.0, 2.0, 3.0, 4.0, 5.0]);
    }
    
    #[test]
    fn test_batch_vectors() {
        let vectors = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
            vec![7.0, 8.0, 9.0],
        ];
        
        let batch = BatchVectors::from_row_major(&vectors);
        
        // Check dimension slices (column-major)
        assert_eq!(batch.get_dimension_slice(0), &[1.0, 4.0, 7.0]);
        assert_eq!(batch.get_dimension_slice(1), &[2.0, 5.0, 8.0]);
        assert_eq!(batch.get_dimension_slice(2), &[3.0, 6.0, 9.0]);
        
        // Check vector retrieval
        assert_eq!(batch.get_vector(0), vec![1.0, 2.0, 3.0]);
        assert_eq!(batch.get_vector(1), vec![4.0, 5.0, 6.0]);
    }
}
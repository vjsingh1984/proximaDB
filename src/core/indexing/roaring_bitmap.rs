//! Roaring Bitmap Index Implementation for ProximaDB
//!
//! This module provides a compressed bitmap index using the RoaringBitmap
//! data structure for efficient categorical metadata filtering in VIPER storage.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Roaring bitmap index for categorical metadata filtering
/// 
/// Provides fast intersection operations for metadata predicates like:
/// - category = "electronics"
/// - status IN ["active", "pending"]
/// - user_type = "premium"
pub struct RoaringBitmapIndex {
    /// Maps metadata field values to bitmap of matching row IDs
    indexes: HashMap<String, HashMap<String, RoaringBitmap>>,
    
    /// Total number of rows indexed
    total_rows: u32,
    
    /// Statistics for monitoring performance
    stats: BitmapIndexStats,
}

impl RoaringBitmapIndex {
    /// Create a new roaring bitmap index
    pub fn new() -> Self {
        Self {
            indexes: HashMap::new(),
            total_rows: 0,
            stats: BitmapIndexStats::default(),
        }
    }
    
    /// Add a document to the index
    /// 
    /// # Arguments
    /// * `row_id` - Unique row identifier (position in storage)
    /// * `metadata` - Key-value pairs of metadata fields
    pub fn insert(&mut self, row_id: u32, metadata: &HashMap<String, String>) {
        for (field, value) in metadata {
            let field_index = self.indexes.entry(field.clone()).or_insert_with(HashMap::new);
            let bitmap = field_index.entry(value.clone()).or_insert_with(RoaringBitmap::new);
            bitmap.insert(row_id);
        }
        
        self.total_rows = self.total_rows.max(row_id + 1);
        self.stats.total_insertions += 1;
        self.update_stats();
    }
    
    /// Remove a document from the index
    pub fn remove(&mut self, row_id: u32, metadata: &HashMap<String, String>) {
        for (field, value) in metadata {
            if let Some(field_index) = self.indexes.get_mut(field) {
                if let Some(bitmap) = field_index.get_mut(value) {
                    bitmap.remove(row_id);
                    
                    // Clean up empty bitmaps
                    if bitmap.is_empty() {
                        field_index.remove(value);
                    }
                }
                
                // Clean up empty field indexes
                if field_index.is_empty() {
                    self.indexes.remove(field);
                }
            }
        }
        
        self.stats.total_removals += 1;
        self.update_stats();
    }
    
    /// Query for rows matching a specific field value
    /// 
    /// # Arguments
    /// * `field` - Metadata field name
    /// * `value` - Field value to match
    /// 
    /// # Returns
    /// Bitmap of row IDs that match the condition
    pub fn query_equals(&self, field: &str, value: &str) -> Option<&RoaringBitmap> {
        self.indexes
            .get(field)?
            .get(value)
    }
    
    /// Query for rows where field value is in a set of values
    /// 
    /// # Arguments
    /// * `field` - Metadata field name  
    /// * `values` - Set of values to match (OR operation)
    /// 
    /// # Returns
    /// Bitmap of row IDs that match any of the values
    pub fn query_in(&self, field: &str, values: &[&str]) -> Option<RoaringBitmap> {
        let field_index = self.indexes.get(field)?;
        
        let mut result = RoaringBitmap::new();
        for value in values {
            if let Some(bitmap) = field_index.get(*value) {
                result = result.union(bitmap);
            }
        }
        
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }
    
    /// Intersect multiple query results (AND operation)
    /// 
    /// # Arguments
    /// * `bitmaps` - Vector of bitmaps to intersect
    /// 
    /// # Returns
    /// Bitmap of row IDs that match all conditions
    pub fn intersect_all(&self, bitmaps: &[&RoaringBitmap]) -> RoaringBitmap {
        if bitmaps.is_empty() {
            return RoaringBitmap::new();
        }
        
        let mut result = bitmaps[0].clone();
        for bitmap in &bitmaps[1..] {
            result = result.intersection(bitmap);
            if result.is_empty() {
                break; // Early termination for empty intersection
            }
        }
        
        result
    }
    
    /// Union multiple query results (OR operation)
    pub fn union_all(&self, bitmaps: &[&RoaringBitmap]) -> RoaringBitmap {
        let mut result = RoaringBitmap::new();
        for bitmap in bitmaps {
            result = result.union(bitmap);
        }
        result
    }
    
    /// Get all unique values for a field
    pub fn get_field_values(&self, field: &str) -> Vec<String> {
        self.indexes
            .get(field)
            .map(|field_index| field_index.keys().cloned().collect())
            .unwrap_or_default()
    }
    
    /// Get cardinality (unique value count) for a field
    pub fn get_field_cardinality(&self, field: &str) -> usize {
        self.indexes
            .get(field)
            .map(|field_index| field_index.len())
            .unwrap_or(0)
    }
    
    /// Get selectivity for a field value (percentage of rows that match)
    pub fn get_selectivity(&self, field: &str, value: &str) -> f64 {
        if self.total_rows == 0 {
            return 0.0;
        }
        
        let matching_rows = self.query_equals(field, value)
            .map(|bitmap| bitmap.cardinality())
            .unwrap_or(0);
            
        matching_rows as f64 / self.total_rows as f64
    }
    
    /// Get comprehensive statistics about the index
    pub fn get_stats(&self) -> &BitmapIndexStats {
        &self.stats
    }
    
    /// Optimize all bitmaps for memory usage and query performance
    pub fn optimize(&mut self) {
        for field_index in self.indexes.values_mut() {
            for bitmap in field_index.values_mut() {
                bitmap.run_optimize();
            }
        }
        self.update_stats();
    }
    
    /// Get memory usage in bytes
    pub fn memory_usage(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        
        for (field, field_index) in &self.indexes {
            total += field.len();
            total += std::mem::size_of::<HashMap<String, RoaringBitmap>>();
            
            for (value, bitmap) in field_index {
                total += value.len();
                total += bitmap.serialized_size();
            }
        }
        
        total
    }
    
    // Private helper methods
    
    fn update_stats(&mut self) {
        self.stats.total_fields = self.indexes.len();
        self.stats.total_values = self.indexes.values()
            .map(|field_index| field_index.len())
            .sum();
        self.stats.total_rows = self.total_rows;
        self.stats.memory_usage_bytes = self.memory_usage();
        
        // Calculate average bitmap density
        let total_bits: u64 = self.indexes.values()
            .flat_map(|field_index| field_index.values())
            .map(|bitmap| bitmap.cardinality())
            .sum();
        
        self.stats.average_bitmap_density = if self.stats.total_values > 0 {
            total_bits as f64 / (self.stats.total_values as f64 * self.total_rows as f64)
        } else {
            0.0
        };
    }
}

impl Default for RoaringBitmapIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics for monitoring bitmap index performance
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BitmapIndexStats {
    pub total_fields: usize,
    pub total_values: usize,
    pub total_rows: u32,
    pub total_insertions: u64,
    pub total_removals: u64,
    pub memory_usage_bytes: usize,
    pub average_bitmap_density: f64,
}

/// Simple RoaringBitmap implementation
/// 
/// This is a simplified implementation for ProximaDB's needs.
/// In production, you might want to use the `roaring` crate for
/// better performance and more features.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoaringBitmap {
    /// Sorted vector of integers for simple implementation
    /// In production, this would use the actual roaring bitmap format
    bits: Vec<u32>,
}

impl RoaringBitmap {
    /// Create a new empty bitmap
    pub fn new() -> Self {
        Self { bits: Vec::new() }
    }
    
    /// Insert a value into the bitmap
    pub fn insert(&mut self, value: u32) {
        if let Err(pos) = self.bits.binary_search(&value) {
            self.bits.insert(pos, value);
        }
    }
    
    /// Remove a value from the bitmap
    pub fn remove(&mut self, value: u32) {
        if let Ok(pos) = self.bits.binary_search(&value) {
            self.bits.remove(pos);
        }
    }
    
    /// Check if bitmap contains a value
    pub fn contains(&self, value: u32) -> bool {
        self.bits.binary_search(&value).is_ok()
    }
    
    /// Check if bitmap is empty
    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }
    
    /// Get number of bits set
    pub fn cardinality(&self) -> u64 {
        self.bits.len() as u64
    }
    
    /// Union with another bitmap
    pub fn union(&self, other: &RoaringBitmap) -> RoaringBitmap {
        let mut result = Vec::new();
        let mut i = 0;
        let mut j = 0;
        
        while i < self.bits.len() && j < other.bits.len() {
            if self.bits[i] < other.bits[j] {
                result.push(self.bits[i]);
                i += 1;
            } else if self.bits[i] > other.bits[j] {
                result.push(other.bits[j]);
                j += 1;
            } else {
                result.push(self.bits[i]);
                i += 1;
                j += 1;
            }
        }
        
        // Add remaining elements
        result.extend_from_slice(&self.bits[i..]);
        result.extend_from_slice(&other.bits[j..]);
        
        RoaringBitmap { bits: result }
    }
    
    /// Intersection with another bitmap
    pub fn intersection(&self, other: &RoaringBitmap) -> RoaringBitmap {
        let mut result = Vec::new();
        let mut i = 0;
        let mut j = 0;
        
        while i < self.bits.len() && j < other.bits.len() {
            if self.bits[i] < other.bits[j] {
                i += 1;
            } else if self.bits[i] > other.bits[j] {
                j += 1;
            } else {
                result.push(self.bits[i]);
                i += 1;
                j += 1;
            }
        }
        
        RoaringBitmap { bits: result }
    }
    
    /// Get iterator over set bits
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.bits.iter().copied()
    }
    
    /// Optimize bitmap for memory and performance
    pub fn run_optimize(&mut self) {
        // For our simple implementation, just ensure vector is properly sized
        self.bits.shrink_to_fit();
    }
    
    /// Get serialized size in bytes
    pub fn serialized_size(&self) -> usize {
        std::mem::size_of::<Vec<u32>>() + self.bits.len() * std::mem::size_of::<u32>()
    }
}

impl Default for RoaringBitmap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roaring_bitmap_basic_operations() {
        let mut bitmap = RoaringBitmap::new();
        
        // Insert values
        bitmap.insert(1);
        bitmap.insert(5);
        bitmap.insert(10);
        bitmap.insert(100);
        
        // Test membership
        assert!(bitmap.contains(1));
        assert!(bitmap.contains(5));
        assert!(bitmap.contains(10));
        assert!(bitmap.contains(100));
        assert!(!bitmap.contains(2));
        assert!(!bitmap.contains(50));
        
        // Test cardinality
        assert_eq!(bitmap.cardinality(), 4);
        
        // Test removal
        bitmap.remove(5);
        assert!(!bitmap.contains(5));
        assert_eq!(bitmap.cardinality(), 3);
    }
    
    #[test]
    fn test_roaring_bitmap_union() {
        let mut bitmap1 = RoaringBitmap::new();
        bitmap1.insert(1);
        bitmap1.insert(3);
        bitmap1.insert(5);
        
        let mut bitmap2 = RoaringBitmap::new();
        bitmap2.insert(2);
        bitmap2.insert(3);
        bitmap2.insert(6);
        
        let union = bitmap1.union(&bitmap2);
        
        assert_eq!(union.cardinality(), 5);
        assert!(union.contains(1));
        assert!(union.contains(2));
        assert!(union.contains(3));
        assert!(union.contains(5));
        assert!(union.contains(6));
    }
    
    #[test]
    fn test_roaring_bitmap_intersection() {
        let mut bitmap1 = RoaringBitmap::new();
        bitmap1.insert(1);
        bitmap1.insert(3);
        bitmap1.insert(5);
        bitmap1.insert(7);
        
        let mut bitmap2 = RoaringBitmap::new();
        bitmap2.insert(3);
        bitmap2.insert(5);
        bitmap2.insert(9);
        
        let intersection = bitmap1.intersection(&bitmap2);
        
        assert_eq!(intersection.cardinality(), 2);
        assert!(intersection.contains(3));
        assert!(intersection.contains(5));
        assert!(!intersection.contains(1));
        assert!(!intersection.contains(7));
        assert!(!intersection.contains(9));
    }
    
    #[test]
    fn test_bitmap_index_operations() {
        let mut index = RoaringBitmapIndex::new();
        
        // Insert documents
        let mut metadata1 = HashMap::new();
        metadata1.insert("category".to_string(), "electronics".to_string());
        metadata1.insert("status".to_string(), "active".to_string());
        index.insert(0, &metadata1);
        
        let mut metadata2 = HashMap::new();
        metadata2.insert("category".to_string(), "books".to_string());
        metadata2.insert("status".to_string(), "active".to_string());
        index.insert(1, &metadata2);
        
        let mut metadata3 = HashMap::new();
        metadata3.insert("category".to_string(), "electronics".to_string());
        metadata3.insert("status".to_string(), "inactive".to_string());
        index.insert(2, &metadata3);
        
        // Test single field queries
        let electronics = index.query_equals("category", "electronics").unwrap();
        assert_eq!(electronics.cardinality(), 2);
        assert!(electronics.contains(0));
        assert!(electronics.contains(2));
        
        let active = index.query_equals("status", "active").unwrap();
        assert_eq!(active.cardinality(), 2);
        assert!(active.contains(0));
        assert!(active.contains(1));
        
        // Test intersection (AND query)
        let active_electronics = index.intersect_all(&[electronics, active]);
        assert_eq!(active_electronics.cardinality(), 1);
        assert!(active_electronics.contains(0));
        
        // Test IN query
        let categories = index.query_in("category", &["electronics", "books"]).unwrap();
        assert_eq!(categories.cardinality(), 3);
    }
    
    #[test]
    fn test_bitmap_index_statistics() {
        let mut index = RoaringBitmapIndex::new();
        
        for i in 0..100 {
            let mut metadata = HashMap::new();
            metadata.insert("category".to_string(), format!("cat_{}", i % 5));
            metadata.insert("status".to_string(), if i % 2 == 0 { "active" } else { "inactive" }.to_string());
            index.insert(i, &metadata);
        }
        
        let stats = index.get_stats();
        assert_eq!(stats.total_fields, 2);
        assert_eq!(stats.total_rows, 100);
        assert_eq!(stats.total_insertions, 100);
        
        // Test selectivity
        let active_selectivity = index.get_selectivity("status", "active");
        assert!((active_selectivity - 0.5).abs() < 0.01); // Should be ~50%
        
        let cat_0_selectivity = index.get_selectivity("category", "cat_0");
        assert!((cat_0_selectivity - 0.2).abs() < 0.01); // Should be ~20%
    }
}
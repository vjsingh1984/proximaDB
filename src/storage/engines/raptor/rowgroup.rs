use arrow_array::{ArrayRef, Float32Array, StringArray, StructArray, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use anyhow::Result;

// Import common types - no more duplication!
use super::common::{
    RowGroup, VectorStats, ColumnStats, Predicate, PredicateOp, 
    MetadataValue, VectorEncoding
};

/// RowGroup manager handles operations on row groups
pub struct RowGroupManager {
    rowgroups: Vec<RowGroup>,
    index: HashMap<u32, usize>, // id -> index mapping
}

impl RowGroupManager {
    pub fn new() -> Self {
        Self {
            rowgroups: Vec::new(),
            index: HashMap::new(),
        }
    }
    
    pub fn add(&mut self, rowgroup: RowGroup) {
        let id = rowgroup.id;
        let idx = self.rowgroups.len();
        self.rowgroups.push(rowgroup);
        self.index.insert(id, idx);
    }
    
    pub fn get(&self, id: u32) -> Option<&RowGroup> {
        self.index.get(&id).and_then(|idx| self.rowgroups.get(*idx))
    }
    
    pub fn get_mut(&mut self, id: u32) -> Option<&mut RowGroup> {
        self.index.get(&id).and_then(|idx| self.rowgroups.get_mut(*idx))
    }
    
    pub fn filter_by_predicate(&self, predicate: &Predicate) -> Vec<u32> {
        let mut matching = Vec::new();
        
        for rg in &self.rowgroups {
            // Check if predicate could match based on stats
            if let Some(stats) = rg.metadata_stats.get(&predicate.field) {
                let could_match = match &predicate.op {
                    PredicateOp::Eq => {
                        // Check if value is within min/max range
                        if let (Some(min), Some(max)) = (&stats.min_value, &stats.max_value) {
                            Self::value_in_range(&predicate.value, min, max)
                        } else {
                            true // No stats, could match
                        }
                    }
                    PredicateOp::Lt | PredicateOp::Lte => {
                        if let Some(min) = &stats.min_value {
                            Self::compare_values(min, &predicate.value) <= 0
                        } else {
                            true
                        }
                    }
                    PredicateOp::Gt | PredicateOp::Gte => {
                        if let Some(max) = &stats.max_value {
                            Self::compare_values(max, &predicate.value) >= 0
                        } else {
                            true
                        }
                    }
                    _ => true, // Conservative: could match
                };
                
                if could_match {
                    matching.push(rg.id);
                }
            } else {
                // No stats for this field, conservatively include
                matching.push(rg.id);
            }
        }
        
        matching
    }
    
    pub fn get_overlapping_by_distance(&self, centroid: &[f32], radius: f32) -> Vec<u32> {
        let mut overlapping = Vec::new();
        
        for rg in &self.rowgroups {
            if let Some(rg_centroid) = &rg.centroid {
                let dist = Self::euclidean_distance(centroid, rg_centroid);
                
                // Check if this rowgroup could contain vectors within radius
                // Conservative: include if centroid is within radius + max_norm
                let max_possible_dist = dist + rg.vector_stats.max_norm;
                if max_possible_dist <= radius {
                    overlapping.push(rg.id);
                }
            } else {
                // No centroid, conservatively include
                overlapping.push(rg.id);
            }
        }
        
        overlapping
    }
    
    fn value_in_range(value: &MetadataValue, min: &MetadataValue, max: &MetadataValue) -> bool {
        Self::compare_values(value, min) >= 0 && Self::compare_values(value, max) <= 0
    }
    
    fn compare_values(a: &MetadataValue, b: &MetadataValue) -> i32 {
        match (a, b) {
            (MetadataValue::Integer(x), MetadataValue::Integer(y)) => {
                if x < y { -1 } else if x > y { 1 } else { 0 }
            }
            (MetadataValue::Float(x), MetadataValue::Float(y)) => {
                if x < y { -1 } else if x > y { 1 } else { 0 }
            }
            (MetadataValue::String(x), MetadataValue::String(y)) => {
                x.cmp(y) as i32
            }
            _ => 0, // Type mismatch, treat as equal
        }
    }
    
    fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>()
            .sqrt()
    }
    
    pub fn optimize_for_sequential_access(&mut self) {
        // Sort rowgroups by offset for sequential I/O
        self.rowgroups.sort_by_key(|rg| rg.offset);
        
        // Rebuild index
        self.index.clear();
        for (idx, rg) in self.rowgroups.iter().enumerate() {
            self.index.insert(rg.id, idx);
        }
    }
    
    pub fn get_total_size(&self) -> u64 {
        self.rowgroups.iter().map(|rg| rg.compressed_size).sum()
    }
    
    pub fn get_total_rows(&self) -> usize {
        self.rowgroups.iter().map(|rg| rg.row_count).sum()
    }
}
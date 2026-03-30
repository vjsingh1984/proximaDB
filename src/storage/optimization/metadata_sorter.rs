/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Metadata-aware sorting for optimal Parquet encoding
//!
//! Sorts vector records by metadata keys to improve compression ratios
//! and enable efficient predicate pushdown in columnar storage.

use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, info};

use crate::proto::proximadb_v1::{FilterableColumnSpec, VectorRecord};

/// Configuration for metadata-based sorting
#[derive(Debug, Clone)]
pub struct MetadataSortConfig {
    /// Primary sort keys in order of priority
    pub primary_sort_keys: Vec<String>,
    /// Secondary sort by vector ID for stable ordering
    pub stable_sort_by_id: bool,
    /// Maximum number of distinct values before switching to hash-based sorting
    pub cardinality_threshold: usize,
}

impl Default for MetadataSortConfig {
    fn default() -> Self {
        Self {
            primary_sort_keys: Vec::new(),
            stable_sort_by_id: true,
            cardinality_threshold: 10000,
        }
    }
}

/// Statistics about the sorting operation
#[derive(Debug, Default)]
pub struct SortingStats {
    pub records_sorted: usize,
    pub sort_keys_used: Vec<String>,
    pub distinct_values_per_key: HashMap<String, usize>,
    pub compression_estimate: f64, // Estimated compression improvement (0.0-1.0)
    pub sort_time_us: u64,
}

/// Metadata-aware sorter for vector records
pub struct MetadataSorter {
    config: MetadataSortConfig,
}

impl MetadataSorter {
    /// Create a new metadata sorter
    pub fn new(config: MetadataSortConfig) -> Self {
        Self { config }
    }

    /// Create sorter from filterable column specifications
    pub fn from_filterable_specs(filterable_columns: &[FilterableColumnSpec]) -> Self {
        let mut sort_keys = Vec::new();

        // Sort by estimated cardinality (low cardinality first for better compression)
        let mut columns_by_cardinality: Vec<_> = filterable_columns.iter().collect();
        columns_by_cardinality.sort_by_key(|col| {
            col.estimated_cardinality // Default cardinality if not specified
        });

        for column in columns_by_cardinality {
            sort_keys.push(column.name.clone());
        }

        let config = MetadataSortConfig {
            primary_sort_keys: sort_keys,
            stable_sort_by_id: true,
            cardinality_threshold: 10000,
        };

        Self::new(config)
    }

    /// Sort vector records for optimal encoding
    pub fn sort_for_encoding(
        &self,
        mut records: Vec<VectorRecord>,
    ) -> Result<(Vec<VectorRecord>, SortingStats)> {
        let start_time = std::time::Instant::now();
        let original_count = records.len();

        if records.is_empty() {
            return Ok((records, SortingStats::default()));
        }

        let mut stats = SortingStats {
            records_sorted: original_count,
            sort_keys_used: self.config.primary_sort_keys.clone(),
            ..Default::default()
        };

        // Analyze metadata distribution for compression estimation
        self.analyze_metadata_distribution(&records, &mut stats);

        // Sort by multiple metadata keys
        records.sort_by(|a, b| {
            // Primary sort: metadata keys in order
            for sort_key in &self.config.primary_sort_keys {
                let a_value = self.extract_metadata_value(a, sort_key);
                let b_value = self.extract_metadata_value(b, sort_key);

                match a_value.cmp(&b_value) {
                    std::cmp::Ordering::Equal => continue,
                    other => return other,
                }
            }

            // Secondary sort: vector ID for stable ordering
            if self.config.stable_sort_by_id {
                let a_id = a.id.as_str();
                let b_id = b.id.as_str();
                a_id.cmp(b_id)
            } else {
                std::cmp::Ordering::Equal
            }
        });

        stats.sort_time_us = start_time.elapsed().as_micros() as u64;

        info!(
            "📊 Metadata Sort: Sorted {} records by {} keys in {}μs (estimated compression improvement: {:.1}%)",
            stats.records_sorted,
            stats.sort_keys_used.len(),
            stats.sort_time_us,
            stats.compression_estimate * 100.0
        );

        Ok((records, stats))
    }

    /// Extract metadata value for sorting (handles different data types)
    fn extract_metadata_value(&self, record: &VectorRecord, key: &str) -> SortableValue {
        // Find the metadata item in the HashMap
        if let Some(sql_value) = record.metadata.get(key)
            && let Some(value) = &sql_value.value {
                match value {
                    crate::proto::proximadb_v1::sql_value::Value::StringValue(s) => {
                        return SortableValue::from_string(s);
                    }
                    crate::proto::proximadb_v1::sql_value::Value::NumberValue(n) => {
                        // Check if it's an integer
                        if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 {
                            return SortableValue::Number(*n as i64);
                        } else {
                            // Store floats as string for consistent ordering
                            return SortableValue::Float(n.to_string());
                        }
                    }
                    crate::proto::proximadb_v1::sql_value::Value::BoolValue(b) => {
                        // Convert bool to string for sorting
                        return SortableValue::String(b.to_string());
                    }
                    _ => {
                        return SortableValue::Null;
                    }
                }
            }

        // Default value if key not found
        SortableValue::Null
    }

    /// Analyze metadata distribution for compression estimation
    fn analyze_metadata_distribution(&self, records: &[VectorRecord], stats: &mut SortingStats) {
        for sort_key in &self.config.primary_sort_keys {
            let mut distinct_values = std::collections::HashSet::new();

            for record in records {
                let value = self.extract_metadata_value(record, sort_key);
                distinct_values.insert(value);
            }

            let cardinality = distinct_values.len();
            stats
                .distinct_values_per_key
                .insert(sort_key.clone(), cardinality);

            debug!(
                "📈 Metadata key '{}': {} distinct values (cardinality: {:.2}%)",
                sort_key,
                cardinality,
                (cardinality as f64 / records.len() as f64) * 100.0
            );
        }

        // Estimate compression improvement based on cardinality
        stats.compression_estimate = self.estimate_compression_improvement(records, stats);
    }

    /// Estimate compression improvement from sorting
    fn estimate_compression_improvement(
        &self,
        records: &[VectorRecord],
        stats: &SortingStats,
    ) -> f64 {
        if records.is_empty() || self.config.primary_sort_keys.is_empty() {
            return 0.0;
        }

        let mut total_improvement = 0.0;
        let mut key_count = 0;

        for sort_key in &self.config.primary_sort_keys {
            if let Some(&cardinality) = stats.distinct_values_per_key.get(sort_key) {
                // Lower cardinality = better compression potential
                let cardinality_ratio = cardinality as f64 / records.len() as f64;
                let improvement = 1.0 - cardinality_ratio;

                // Weight first keys more heavily
                let weight = 1.0 / (key_count as f64 + 1.0);
                total_improvement += improvement * weight;
                key_count += 1;
            }
        }

        if key_count > 0 {
            total_improvement / key_count as f64
        } else {
            0.0
        }
    }
}

/// Sortable value that handles different data types uniformly
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SortableValue {
    Null,
    String(String),
    Number(i64),   // For integer-like strings
    Float(String), // For float-like strings (stored as string for ordering)
}

impl PartialOrd for SortableValue {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortableValue {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (SortableValue::Null, SortableValue::Null) => Ordering::Equal,
            (SortableValue::Null, _) => Ordering::Less,
            (_, SortableValue::Null) => Ordering::Greater,

            (SortableValue::Number(a), SortableValue::Number(b)) => a.cmp(b),
            (SortableValue::Number(_), SortableValue::Float(_)) => Ordering::Less,
            (SortableValue::Number(_), SortableValue::String(_)) => Ordering::Less,

            (SortableValue::Float(a), SortableValue::Float(b)) => a.cmp(b),
            (SortableValue::Float(_), SortableValue::Number(_)) => Ordering::Greater,
            (SortableValue::Float(_), SortableValue::String(_)) => Ordering::Less,

            (SortableValue::String(a), SortableValue::String(b)) => a.cmp(b),
            (SortableValue::String(_), SortableValue::Number(_)) => Ordering::Greater,
            (SortableValue::String(_), SortableValue::Float(_)) => Ordering::Greater,
        }
    }
}

impl SortableValue {
    fn from_string(value: &str) -> Self {
        if value.is_empty() {
            return Self::Null;
        }

        // Try to parse as integer
        if let Ok(num) = value.parse::<i64>() {
            return Self::Number(num);
        }

        // Try to parse as float
        if let Ok(_) = value.parse::<f64>() {
            return Self::Float(value.to_string());
        }

        // Default to string
        Self::String(value.to_string())
    }
}

/// Builder for creating optimized sort configurations
pub struct SortConfigBuilder {
    config: MetadataSortConfig,
}

impl SortConfigBuilder {
    pub fn new() -> Self {
        Self {
            config: MetadataSortConfig::default(),
        }
    }

    pub fn add_sort_key(mut self, key: String) -> Self {
        self.config.primary_sort_keys.push(key);
        self
    }

    pub fn add_sort_keys<I>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        self.config.primary_sort_keys.extend(keys);
        self
    }

    pub fn stable_sort_by_id(mut self, enabled: bool) -> Self {
        self.config.stable_sort_by_id = enabled;
        self
    }

    pub fn cardinality_threshold(mut self, threshold: usize) -> Self {
        self.config.cardinality_threshold = threshold;
        self
    }

    pub fn build(self) -> MetadataSortConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::SqlValue;
    // Import removed - using HashMap metadata now

    fn create_test_record(id: &str, category: &str, priority: &str) -> VectorRecord {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            "category".to_string(),
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    category.to_string(),
                )),
            },
        );
        metadata.insert(
            "priority".to_string(),
            SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    priority.to_string(),
                )),
            },
        );

        VectorRecord {
            id: id.to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            source: None,
        }
    }

    #[test]
    fn test_metadata_sorting() {
        let config = SortConfigBuilder::new()
            .add_sort_key("category".to_string())
            .add_sort_key("priority".to_string())
            .build();

        let sorter = MetadataSorter::new(config);

        let records = vec![
            create_test_record("3", "B", "high"),
            create_test_record("1", "A", "low"),
            create_test_record("2", "A", "high"),
            create_test_record("4", "B", "low"),
        ];

        let (sorted_records, stats) = sorter.sort_for_encoding(records).unwrap();

        // Should be sorted by category first, then priority, then ID
        assert_eq!(sorted_records[0].id, "2"); // A, high
        assert_eq!(sorted_records[1].id, "1"); // A, low
        assert_eq!(sorted_records[2].id, "3"); // B, high
        assert_eq!(sorted_records[3].id, "4"); // B, low

        assert_eq!(stats.records_sorted, 4);
        assert_eq!(stats.sort_keys_used, vec!["category", "priority"]);
    }

    #[test]
    fn test_sortable_value_ordering() {
        let mut values = vec![
            SortableValue::String("zebra".to_string()),
            SortableValue::Number(10),
            SortableValue::Null,
            SortableValue::Number(2),
            SortableValue::String("apple".to_string()),
            SortableValue::Float("3.14".to_string()),
        ];

        values.sort();

        // Expected order: Null, Numbers (2, 10), Float (3.14), Strings (apple, zebra)
        assert!(matches!(values[0], SortableValue::Null));
        assert!(matches!(values[1], SortableValue::Number(2)));
        assert!(matches!(values[2], SortableValue::Number(10)));
        assert!(matches!(values[3], SortableValue::Float(_)));
        assert!(matches!(values[4], SortableValue::String(_)));
        assert!(matches!(values[5], SortableValue::String(_)));
    }
}

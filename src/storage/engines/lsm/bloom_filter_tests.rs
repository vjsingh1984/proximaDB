//! Unit tests for enhanced bloom filter system
//!
//! Tests metadata bloom filters and combined SSTable bloom filters

use super::bloom_filter::*;
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_filter_creation() {
        let config = BloomFilterConfig::default();
        let filter = BloomFilter::new(1000, &config);
        
        assert_eq!(filter.num_elements, 1000);
        assert!(filter.num_bits > 0);
        assert!(filter.num_hashes > 0);
        assert!(!filter.bits.is_empty());
    }

    #[test]
    fn test_bloom_filter_config_default() {
        let config = BloomFilterConfig::default();
        
        assert_eq!(config.false_positive_rate, 0.01);
        assert_eq!(config.min_elements, 100);
    }

    #[test]
    fn test_bloom_filter_insert_and_check() {
        let config = BloomFilterConfig::default();
        let mut filter = BloomFilter::new(1000, &config);
        
        // Insert keys
        filter.insert("key1");
        filter.insert("key2");
        filter.insert("key3");
        
        // Check inserted keys
        assert!(filter.might_contain("key1"));
        assert!(filter.might_contain("key2"));
        assert!(filter.might_contain("key3"));
        
        // Check non-inserted key (should be false with high probability)
        assert!(!filter.might_contain("nonexistent_key_12345"));
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        let config = BloomFilterConfig {
            false_positive_rate: 0.01,
            min_elements: 100,
        };
        let filter = BloomFilter::new(1000, &config);
        
        let calculated_rate = filter.false_positive_rate();
        assert!(calculated_rate <= 0.02); // Should be close to configured rate
    }

    #[test]
    fn test_bloom_filter_size_calculation() {
        let config = BloomFilterConfig::default();
        let filter = BloomFilter::new(1000, &config);
        
        let size = filter.size_bytes();
        assert!(size > 0);
        assert_eq!(size, filter.bits.len());
    }

    #[test]
    fn test_bloom_filter_builder() {
        let config = BloomFilterConfig::default();
        let mut builder = BloomFilterBuilder::new(config);
        
        builder.add_key("key1".to_string());
        builder.add_key("key2".to_string());
        builder.add_keys(vec!["key3", "key4", "key5"]);
        
        let filter = builder.build();
        
        assert!(filter.might_contain("key1"));
        assert!(filter.might_contain("key2"));
        assert!(filter.might_contain("key3"));
        assert!(filter.might_contain("key4"));
        assert!(filter.might_contain("key5"));
    }

    #[test]
    fn test_metadata_bloom_filter_creation() {
        let config = BloomFilterConfig::default();
        let filter = MetadataBloomFilter::new(config);
        
        assert_eq!(filter.column_filters.len(), 0);
        assert_eq!(filter.config.false_positive_rate, 0.01);
    }

    #[test]
    fn test_metadata_bloom_filter_add_column() {
        let config = BloomFilterConfig::default();
        let mut filter = MetadataBloomFilter::new(config);
        
        let values = vec!["electronics".to_string(), "books".to_string(), "clothing".to_string()];
        filter.add_column("category".to_string(), values);
        
        assert_eq!(filter.column_filters.len(), 1);
        assert!(filter.column_filters.contains_key("category"));
        assert_eq!(filter.num_columns(), 1);
    }

    #[test]
    fn test_metadata_bloom_filter_might_match() {
        let config = BloomFilterConfig::default();
        let mut filter = MetadataBloomFilter::new(config);
        
        let values = vec!["electronics".to_string(), "books".to_string(), "clothing".to_string()];
        filter.add_column("category".to_string(), values);
        
        // Should match existing values
        assert!(filter.might_match_metadata("category", "electronics"));
        assert!(filter.might_match_metadata("category", "books"));
        assert!(filter.might_match_metadata("category", "clothing"));
        
        // Should not match non-existent values
        assert!(!filter.might_match_metadata("category", "nonexistent"));
        
        // Should return true for non-existent column
        assert!(filter.might_match_metadata("nonexistent_column", "value"));
    }

    #[test]
    fn test_metadata_bloom_filter_multiple_conditions() {
        let config = BloomFilterConfig::default();
        let mut filter = MetadataBloomFilter::new(config);
        
        // Add multiple columns
        filter.add_column("category".to_string(), vec!["electronics".to_string(), "books".to_string()]);
        filter.add_column("brand".to_string(), vec!["apple".to_string(), "samsung".to_string()]);
        
        // Test conditions that should match
        let conditions1 = HashMap::from([
            ("category".to_string(), "electronics".to_string()),
            ("brand".to_string(), "apple".to_string()),
        ]);
        assert!(filter.might_match_conditions(&conditions1));
        
        // Test conditions that should not match
        let conditions2 = HashMap::from([
            ("category".to_string(), "electronics".to_string()),
            ("brand".to_string(), "nonexistent".to_string()),
        ]);
        assert!(!filter.might_match_conditions(&conditions2));
    }

    #[test]
    fn test_metadata_bloom_filter_builder() {
        let config = BloomFilterConfig::default();
        let mut builder = MetadataBloomFilterBuilder::new(config);
        
        builder.add_column_values("category".to_string(), vec!["electronics".to_string(), "books".to_string()]);
        builder.add_value("brand".to_string(), "apple".to_string());
        builder.add_value("brand".to_string(), "samsung".to_string());
        
        let filter = builder.build();
        
        assert_eq!(filter.num_columns(), 2);
        assert!(filter.might_match_metadata("category", "electronics"));
        assert!(filter.might_match_metadata("brand", "apple"));
        assert!(filter.might_match_metadata("brand", "samsung"));
    }

    #[test]
    fn test_metadata_bloom_filter_size_calculation() {
        let config = BloomFilterConfig::default();
        let mut filter = MetadataBloomFilter::new(config);
        
        filter.add_column("category".to_string(), vec!["electronics".to_string(), "books".to_string()]);
        filter.add_column("brand".to_string(), vec!["apple".to_string(), "samsung".to_string()]);
        
        let total_size = filter.total_size_bytes();
        assert!(total_size > 0);
        
        let individual_sizes: usize = filter.column_filters.values().map(|f| f.size_bytes()).sum();
        assert_eq!(total_size, individual_sizes);
    }

    #[test]
    fn test_sstable_bloom_filter_creation() {
        let config = BloomFilterConfig::default();
        let key_filter = BloomFilter::new(1000, &config);
        let metadata_filter = MetadataBloomFilter::new(config);
        
        let sstable_filter = SstableBloomFilter::new(key_filter, metadata_filter);
        
        assert_eq!(sstable_filter.stats.total_keys, 0);
        assert_eq!(sstable_filter.stats.key_lookups_saved, 0);
        assert_eq!(sstable_filter.stats.metadata_queries_saved, 0);
    }

    #[test]
    fn test_sstable_bloom_filter_key_operations() {
        let config = BloomFilterConfig::default();
        let mut key_filter = BloomFilter::new(1000, &config);
        key_filter.insert("key1");
        key_filter.insert("key2");
        
        let metadata_filter = MetadataBloomFilter::new(config);
        let mut sstable_filter = SstableBloomFilter::new(key_filter, metadata_filter);
        
        // Test key that exists
        assert!(sstable_filter.might_contain_key("key1"));
        assert!(sstable_filter.might_contain_key("key2"));
        
        // Test key that doesn't exist (should increment stats)
        assert!(!sstable_filter.might_contain_key("nonexistent"));
        assert_eq!(sstable_filter.stats.key_lookups_saved, 1);
    }

    #[test]
    fn test_sstable_bloom_filter_metadata_operations() {
        let config = BloomFilterConfig::default();
        let key_filter = BloomFilter::new(1000, &config);
        
        let mut metadata_filter = MetadataBloomFilter::new(config);
        metadata_filter.add_column("category".to_string(), vec!["electronics".to_string()]);
        
        let mut sstable_filter = SstableBloomFilter::new(key_filter, metadata_filter);
        
        // Test metadata that exists
        let conditions1 = HashMap::from([("category".to_string(), "electronics".to_string())]);
        assert!(sstable_filter.might_match_metadata(&conditions1));
        
        // Test metadata that doesn't exist (should increment stats)
        let conditions2 = HashMap::from([("category".to_string(), "nonexistent".to_string())]);
        assert!(!sstable_filter.might_match_metadata(&conditions2));
        assert_eq!(sstable_filter.stats.metadata_queries_saved, 1);
    }

    #[test]
    fn test_sstable_bloom_filter_combined_query() {
        let config = BloomFilterConfig::default();
        let mut key_filter = BloomFilter::new(1000, &config);
        key_filter.insert("key1");
        
        let mut metadata_filter = MetadataBloomFilter::new(config);
        metadata_filter.add_column("category".to_string(), vec!["electronics".to_string()]);
        
        let mut sstable_filter = SstableBloomFilter::new(key_filter, metadata_filter);
        
        // Test query that should match both key and metadata
        let conditions = HashMap::from([("category".to_string(), "electronics".to_string())]);
        assert!(sstable_filter.might_match_query(Some("key1"), Some(&conditions)));
        
        // Test query that should fail on key
        assert!(!sstable_filter.might_match_query(Some("nonexistent_key"), Some(&conditions)));
        
        // Test query that should fail on metadata
        let bad_conditions = HashMap::from([("category".to_string(), "nonexistent".to_string())]);
        assert!(!sstable_filter.might_match_query(Some("key1"), Some(&bad_conditions)));
    }

    #[test]
    fn test_sstable_bloom_filter_size_calculation() {
        let config = BloomFilterConfig::default();
        let key_filter = BloomFilter::new(1000, &config);
        let mut metadata_filter = MetadataBloomFilter::new(config);
        metadata_filter.add_column("category".to_string(), vec!["electronics".to_string()]);
        
        let sstable_filter = SstableBloomFilter::new(key_filter, metadata_filter);
        
        let total_size = sstable_filter.total_size_bytes();
        assert!(total_size > 0);
        
        let expected_size = sstable_filter.key_filter.size_bytes() + sstable_filter.metadata_filter.total_size_bytes();
        assert_eq!(total_size, expected_size);
    }

    #[test]
    fn test_sstable_bloom_filter_efficiency_stats() {
        let config = BloomFilterConfig::default();
        let key_filter = BloomFilter::new(1000, &config);
        let metadata_filter = MetadataBloomFilter::new(config);
        
        let mut sstable_filter = SstableBloomFilter::new(key_filter, metadata_filter);
        
        // Initially no stats
        let (key_eff, meta_eff) = sstable_filter.efficiency_stats();
        assert_eq!(key_eff, 0.0);
        assert_eq!(meta_eff, 0.0);
        
        // Add some stats
        sstable_filter.stats.total_keys = 100;
        sstable_filter.stats.key_lookups_saved = 20;
        sstable_filter.stats.metadata_queries_saved = 30;
        
        let (key_eff, meta_eff) = sstable_filter.efficiency_stats();
        assert_eq!(key_eff, 0.2);
        assert_eq!(meta_eff, 0.3);
    }

    #[test]
    fn test_bloom_filter_stats_creation() {
        let stats = BloomFilterStats {
            total_keys: 1000,
            metadata_queries_saved: 200,
            key_lookups_saved: 150,
        };
        
        assert_eq!(stats.total_keys, 1000);
        assert_eq!(stats.metadata_queries_saved, 200);
        assert_eq!(stats.key_lookups_saved, 150);
    }

    #[test]
    fn test_bloom_filter_serialization() {
        let config = BloomFilterConfig::default();
        let mut filter = BloomFilter::new(100, &config);
        filter.insert("test_key");
        
        let serialized = bincode::serialize(&filter).unwrap();
        let deserialized: BloomFilter = bincode::deserialize(&serialized).unwrap();
        
        assert_eq!(deserialized.num_elements, filter.num_elements);
        assert_eq!(deserialized.num_bits, filter.num_bits);
        assert_eq!(deserialized.num_hashes, filter.num_hashes);
        assert_eq!(deserialized.bits, filter.bits);
        assert!(deserialized.might_contain("test_key"));
    }

    #[test]
    fn test_metadata_bloom_filter_serialization() {
        let config = BloomFilterConfig::default();
        let mut filter = MetadataBloomFilter::new(config);
        filter.add_column("category".to_string(), vec!["electronics".to_string()]);
        
        let serialized = bincode::serialize(&filter).unwrap();
        let deserialized: MetadataBloomFilter = bincode::deserialize(&serialized).unwrap();
        
        assert_eq!(deserialized.num_columns(), filter.num_columns());
        assert!(deserialized.might_match_metadata("category", "electronics"));
    }

    #[test]
    fn test_sstable_bloom_filter_serialization() {
        let config = BloomFilterConfig::default();
        let mut key_filter = BloomFilter::new(100, &config);
        key_filter.insert("test_key");
        
        let mut metadata_filter = MetadataBloomFilter::new(config);
        metadata_filter.add_column("category".to_string(), vec!["electronics".to_string()]);
        
        let sstable_filter = SstableBloomFilter::new(key_filter, metadata_filter);
        
        let serialized = bincode::serialize(&sstable_filter).unwrap();
        let mut deserialized: SstableBloomFilter = bincode::deserialize(&serialized).unwrap();
        
        assert!(deserialized.might_contain_key("test_key"));
        
        let conditions = HashMap::from([("category".to_string(), "electronics".to_string())]);
        assert!(deserialized.might_match_metadata(&conditions));
    }

    #[test]
    fn test_bloom_filter_performance_characteristics() {
        let config = BloomFilterConfig {
            false_positive_rate: 0.001, // Very low false positive rate
            min_elements: 100,
        };
        
        let filter = BloomFilter::new(10000, &config);
        
        // Filter should be larger due to low false positive rate
        assert!(filter.num_bits > 10000);
        assert!(filter.num_hashes > 1);
        
        let actual_fp_rate = filter.false_positive_rate();
        assert!(actual_fp_rate <= 0.01); // Should be very low
    }

    #[test]
    fn test_edge_cases() {
        let config = BloomFilterConfig::default();
        
        // Test with very small number of elements
        let small_filter = BloomFilter::new(1, &config);
        assert!(small_filter.num_elements >= config.min_elements as u32);
        
        // Test empty metadata filter
        let empty_filter = MetadataBloomFilter::new(config);
        assert_eq!(empty_filter.num_columns(), 0);
        assert_eq!(empty_filter.total_size_bytes(), 0);
        
        // Test with empty conditions
        let empty_conditions = HashMap::new();
        assert!(empty_filter.might_match_conditions(&empty_conditions));
    }
}
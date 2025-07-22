//! Tests for LSM bloom filters using the unified design

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::core::bloom::{
        BloomFilterConfig, BloomStrategy, MetadataBloomFilter,
        factory::BloomFilterFactory,
        strategies::CompositeBloomFilter,
    };
    use crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder;
    use std::collections::HashMap;
    
    #[test]
    fn test_bloom_filter_basic_operations() {
        let config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            bits_per_key: 10,
            expected_items: 100,
            enabled: true,
            ..Default::default()
        };
        
        let mut filter = BloomFilterFactory::create(&config);
        
        // Insert some keys
        filter.insert(b"key1");
        filter.insert(b"key2");
        filter.insert(b"key3");
        
        // Check they exist
        assert!(filter.might_contain(b"key1"));
        assert!(filter.might_contain(b"key2"));
        assert!(filter.might_contain(b"key3"));
        
        // Check non-existent key (might have false positives)
        // We can't assert false because bloom filters can have false positives
        let _result = filter.might_contain(b"key4");
    }
    
    #[test]
    fn test_bloom_filter_false_positive_rate() {
        let config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            bits_per_key: 10,
            expected_items: 1000,
            enabled: true,
            ..Default::default()
        };
        
        let filter = BloomFilterFactory::create(&config);
        let calculated_rate = filter.false_positive_rate();
        
        println!("Calculated false positive rate: {}", calculated_rate);
        
        // With 10 bits per key, false positive rate should be approximately 0.0095
        // Note: An empty bloom filter should have 0.0 false positive rate
        assert!(calculated_rate >= 0.0 && calculated_rate < 0.02);
    }
    
    #[test]
    fn test_metadata_bloom_filter() {
        let config = BloomFilterConfig {
            strategy: BloomStrategy::Composite,
            expected_items: 100,
            enabled: true,
            ..Default::default()
        };
        
        let mut builder = CompositeBloomFilterBuilder::new(config);
        
        // Add metadata values
        builder.add_metadata_value("category".to_string(), "electronics".to_string());
        builder.add_metadata_value("category".to_string(), "books".to_string());
        builder.add_metadata_value("price".to_string(), "99.99".to_string());
        
        let filter = builder.build();
        
        // Check metadata exists
        assert!(MetadataBloomFilter::might_match_metadata(&filter, "category", "electronics"));
        assert!(MetadataBloomFilter::might_match_metadata(&filter, "category", "books"));
        assert!(MetadataBloomFilter::might_match_metadata(&filter, "price", "99.99"));
        
        // Check non-existent metadata
        let _result = MetadataBloomFilter::might_match_metadata(&filter, "category", "food");
    }
    
    #[test]
    fn test_sstable_bloom_filter() {
        // Create key filter
        let key_config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            expected_items: 100,
            ..Default::default()
        };
        let mut key_filter = BloomFilterFactory::create(&key_config);
        key_filter.insert(b"key1");
        key_filter.insert(b"key2");
        
        // Create metadata filter
        let meta_config = BloomFilterConfig {
            strategy: BloomStrategy::Composite,
            expected_items: 100,
            ..Default::default()
        };
        let mut meta_builder = CompositeBloomFilterBuilder::new(meta_config);
        meta_builder.add_metadata_value("type".to_string(), "document".to_string());
        let metadata_filter = meta_builder.build();
        
        // Create SSTable bloom filter
        let sstable_filter = SstableBloomFilter::new(
            key_filter.as_ref(),
            &metadata_filter
        ).unwrap();
        
        // Test key lookups
        assert!(sstable_filter.might_contain_key("key1").unwrap());
        assert!(sstable_filter.might_contain_key("key2").unwrap());
        
        // Test metadata lookups
        assert!(sstable_filter.might_match_metadata("type", "document").unwrap());
        
        // Test combined query
        let mut conditions = HashMap::new();
        conditions.insert("type".to_string(), "document".to_string());
        assert!(sstable_filter.might_match_query(
            Some("key1"),
            Some(&conditions)
        ).unwrap());
    }
    
    #[test]
    fn test_bloom_filter_serialization() {
        let config = BloomFilterConfig::default();
        let mut filter = BloomFilterFactory::create(&config);
        
        // Add data
        filter.insert(b"test1");
        filter.insert(b"test2");
        
        // Serialize
        let serialized_data = filter.serialize().unwrap();
        assert!(serialized_data.len() > 0);
        
        // Create SerializedBloomFilter for deserialization
        let serialized = crate::core::bloom::SerializedBloomFilter {
            strategy_type: config.strategy,
            version: crate::core::bloom::SerializedBloomFilter::CURRENT_VERSION,
            config: config.clone(),
            data: serialized_data,
            metadata: HashMap::new(),
        };
        
        // Deserialize
        let restored = BloomFilterFactory::from_serialized(&serialized).unwrap();
        
        // Verify data is preserved
        assert!(restored.might_contain(b"test1"));
        assert!(restored.might_contain(b"test2"));
    }
    
    #[test]
    fn test_bloom_filter_size_estimation() {
        let config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            bits_per_key: 10,
            expected_items: 1000,
            enabled: true,
            ..Default::default()
        };
        
        let filter = BloomFilterFactory::create(&config);
        
        // Expected size: ~10 bits per key * 1000 keys / 8 bits per byte
        let expected_size = (10 * 1000) / 8;
        let actual_size = filter.bit_count() / 8;
        
        // Allow some variance for overhead
        assert!(actual_size >= expected_size);
        assert!(actual_size <= expected_size * 2);
    }
    
    #[test]
    fn test_bloom_filter_with_high_accuracy() {
        let config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            bits_per_key: 20, // Higher bits for very low false positive rate
            expected_items: 100,
            enabled: true,
            ..Default::default()
        };
        
        let mut filter = BloomFilterFactory::create(&config);
        
        // Insert keys
        for i in 0..50 {
            filter.insert(format!("key_{}", i).as_bytes());
        }
        
        // Check all inserted keys exist
        for i in 0..50 {
            assert!(filter.might_contain(format!("key_{}", i).as_bytes()));
        }
        
        // With 20 bits per key, false positive rate should be very low
        assert!(filter.false_positive_rate() < 0.001);
    }
    
    #[test]
    fn test_disabled_bloom_filter() {
        let config = BloomFilterConfig {
            enabled: false,
            ..Default::default()
        };
        
        let filter = BloomFilterFactory::create(&config);
        
        // Disabled filter should always return true (conservative)
        assert!(filter.might_contain(b"anything"));
        assert!(filter.might_contain(b"everything"));
    }
    
    #[test]
    fn test_bloom_filter_stats() {
        // Create filters
        let key_config = BloomFilterConfig::for_sstable(100);
        let key_filter = BloomFilterFactory::create(&key_config);
        
        let meta_filter = CompositeBloomFilter::new(100, &BloomFilterConfig::default());
        
        // Create SSTable filter
        let sstable_filter = SstableBloomFilter::new(
            key_filter.as_ref(),
            &meta_filter
        ).unwrap();
        
        // Check stats
        let stats = sstable_filter.efficiency_stats();
        assert!(stats.contains_key("key_count"));
        assert!(stats.contains_key("metadata_columns"));
        assert!(stats.contains_key("total_keys"));
        assert!(stats.contains_key("key_lookups_saved"));
        assert!(stats.contains_key("metadata_queries_saved"));
    }
}
use proximadb::core::bloom::{BloomFilterConfig, BloomStrategy, BloomFilterStrategy};
use proximadb::core::bloom::factory::BloomFilterFactory;
use proximadb::core::bloom::strategies::composite::CompositeBloomFilterBuilder;
use proximadb::storage::engines::sst::bloom_filter::SstableBloomFilter;

fn main() {
    // Create a key bloom filter
    let bloom_config = BloomFilterConfig {
        strategy: BloomStrategy::ByteAligned,
        expected_items: 3,
        ..Default::default()
    };
    let mut key_bloom_filter = BloomFilterFactory::create(&bloom_config);
    key_bloom_filter.insert(b"vec1");
    key_bloom_filter.insert(b"vec2");
    key_bloom_filter.insert(b"vec3");
    
    // Create metadata bloom filter
    let metadata_config = BloomFilterConfig {
        strategy: BloomStrategy::Composite,
        expected_items: 3,
        ..Default::default()
    };
    let mut metadata_builder = CompositeBloomFilterBuilder::new(metadata_config);
    metadata_builder.add_metadata_value("category".to_string(), "A".to_string());
    metadata_builder.add_metadata_value("category".to_string(), "B".to_string());
    let metadata_bloom_filter = metadata_builder.build();
    
    // Create SSTable bloom filter
    let combined_bloom_filter = SstableBloomFilter::new(key_bloom_filter.as_ref(), &metadata_bloom_filter).unwrap();
    
    // Serialize it
    let bloom_data = bincode::serialize(&combined_bloom_filter).unwrap();
    println!("Total bloom filter size: {} bytes", bloom_data.len());
    println!("First 50 bytes: {:?}", &bloom_data[..50.min(bloom_data.len())]);
    
    // Try to deserialize
    match bincode::deserialize::<SstableBloomFilter>(&bloom_data) {
        Ok(_) => println!("Deserialization successful!"),
        Err(e) => println!("Deserialization failed: {:?}", e),
    }
}
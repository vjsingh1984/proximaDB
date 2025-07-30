use proximadb::storage::engines::sst::bloom_filter::{BloomFilter, MetadataBloomFilter, SstableBloomFilter};
use proximadb::core::bloom::BloomFilterConfig;

fn main() {
    let config = BloomFilterConfig {
        bits_per_key: 10,
        enabled: true,
        ..Default::default()
    };
    
    // Create a simple bloom filter
    let mut key_filter = BloomFilter::new(3, &config);
    key_filter.insert("key1");
    key_filter.insert("key2");
    key_filter.insert("key3");
    
    // Create metadata filter
    let metadata_filter = MetadataBloomFilter::new();
    
    // Create SSTable bloom filter
    let sstable_bloom = SstableBloomFilter::new(key_filter.clone(), metadata_filter);
    
    // Serialize it
    let serialized = bincode::serialize(&sstable_bloom).unwrap();
    println!("Serialized SstableBloomFilter: {} bytes", serialized.len());
    println!("First 20 bytes: {:?}", &serialized[..20.min(serialized.len())]);
    
    // Try to deserialize as SstableBloomFilter
    match bincode::deserialize::<SstableBloomFilter>(&serialized) {
        Ok(_) => println!("✓ Successfully deserialized as SstableBloomFilter"),
        Err(e) => println!("✗ Failed to deserialize as SstableBloomFilter: {:?}", e),
    }
    
    // Try to deserialize as just BloomFilter
    match bincode::deserialize::<BloomFilter>(&serialized) {
        Ok(bf) => println!("✓ Successfully deserialized as BloomFilter! Elements: {}", bf.num_elements),
        Err(e) => println!("✗ Failed to deserialize as BloomFilter: {:?}", e),
    }
    
    // Now serialize just the key filter
    let key_filter_serialized = bincode::serialize(&key_filter).unwrap();
    println!("\nSerialized BloomFilter: {} bytes", key_filter_serialized.len());
    println!("First 20 bytes: {:?}", &key_filter_serialized[..20.min(key_filter_serialized.len())]);
}
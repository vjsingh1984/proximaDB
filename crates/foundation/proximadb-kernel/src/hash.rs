//! Fast non-cryptographic hash functions - replaces blake3 for ID generation
//!
//! Provides xxHash and FNV-1a implementations for fast hashing of vector IDs.
//! These are much faster than cryptographic hashes and suitable for ID generation.

use std::hash::Hasher;

fn read_u64_le(bytes: &[u8]) -> u64 {
    let mut array = [0_u8; 8];
    if bytes.len() >= 8 {
        array.copy_from_slice(&bytes[..8]);
    }
    u64::from_le_bytes(array)
}

fn read_u32_le(bytes: &[u8]) -> u32 {
    let mut array = [0_u8; 4];
    if bytes.len() >= 4 {
        array.copy_from_slice(&bytes[..4]);
    }
    u32::from_le_bytes(array)
}

/// xxHash64 - extremely fast non-cryptographic hash
pub struct XxHash64 {
    /// Initial seed value
    seed: u64,
    /// Accumulator lane 1
    v1: u64,
    /// Accumulator lane 2
    v2: u64,
    /// Accumulator lane 3
    v3: u64,
    /// Accumulator lane 4
    v4: u64,
    /// Pending bytes not yet consumed in a 32-byte block
    buffer: Vec<u8>,
    /// Total number of bytes fed so far
    total_len: usize,
}

impl XxHash64 {
    /// First prime constant used in xxHash64 mixing
    const PRIME1: u64 = 0x9E3779B185EBCA87;
    /// Second prime constant used in xxHash64 mixing
    const PRIME2: u64 = 0xC2B2AE3D27D4EB4F;
    /// Third prime constant used in xxHash64 mixing
    const PRIME3: u64 = 0x165667B19E3779F9;
    /// Fourth prime constant used in xxHash64 mixing
    const PRIME4: u64 = 0x85EBCA77C2B2AE63;
    /// Fifth prime constant used in xxHash64 mixing
    const PRIME5: u64 = 0x27D4EB2F165667C5;

    /// Create a new xxHash64 hasher with the given seed
    pub fn new(seed: u64) -> Self {
        XxHash64 {
            seed,
            v1: seed.wrapping_add(Self::PRIME1).wrapping_add(Self::PRIME2),
            v2: seed.wrapping_add(Self::PRIME2),
            v3: seed,
            v4: seed.wrapping_sub(Self::PRIME1),
            buffer: Vec::new(),
            total_len: 0,
        }
    }

    /// Feed data into the hash, processing complete 32-byte blocks immediately
    pub fn update(&mut self, data: &[u8]) {
        self.total_len += data.len();
        self.buffer.extend_from_slice(data);

        // Process 32-byte blocks
        while self.buffer.len() >= 32 {
            let chunk = &self.buffer[0..32];
            self.v1 = self.round(self.v1, read_u64_le(&chunk[0..8]));
            self.v2 = self.round(self.v2, read_u64_le(&chunk[8..16]));
            self.v3 = self.round(self.v3, read_u64_le(&chunk[16..24]));
            self.v4 = self.round(self.v4, read_u64_le(&chunk[24..32]));
            self.buffer.drain(0..32);
        }
    }

    /// Finalize the hash and return the 64-bit digest
    pub fn finish(&self) -> u64 {
        let mut hash = if self.total_len >= 32 {
            self.v1
                .rotate_left(1)
                .wrapping_add(self.v2.rotate_left(7))
                .wrapping_add(self.v3.rotate_left(12))
                .wrapping_add(self.v4.rotate_left(18))
        } else {
            self.seed.wrapping_add(Self::PRIME5)
        };

        hash = hash.wrapping_add(self.total_len as u64);

        // Process remaining bytes
        let mut remaining = self.buffer.as_slice();
        while remaining.len() >= 8 {
            let k1 = read_u64_le(&remaining[0..8]);
            hash ^= self.round(0, k1);
            hash = hash
                .rotate_left(27)
                .wrapping_mul(Self::PRIME1)
                .wrapping_add(Self::PRIME4);
            remaining = &remaining[8..];
        }

        if remaining.len() >= 4 {
            let k1 = read_u32_le(&remaining[0..4]) as u64;
            hash ^= k1.wrapping_mul(Self::PRIME1);
            hash = hash
                .rotate_left(23)
                .wrapping_mul(Self::PRIME2)
                .wrapping_add(Self::PRIME3);
            remaining = &remaining[4..];
        }

        for &byte in remaining {
            hash ^= (byte as u64).wrapping_mul(Self::PRIME5);
            hash = hash.rotate_left(11).wrapping_mul(Self::PRIME1);
        }

        // Final mix
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(Self::PRIME2);
        hash ^= hash >> 29;
        hash = hash.wrapping_mul(Self::PRIME3);
        hash ^= hash >> 32;

        hash
    }

    /// Perform one round of xxHash64 accumulation
    fn round(&self, acc: u64, input: u64) -> u64 {
        acc.wrapping_add(input.wrapping_mul(Self::PRIME2))
            .rotate_left(31)
            .wrapping_mul(Self::PRIME1)
    }
}

/// FNV-1a hash - simple and fast
pub struct Fnv1a64 {
    /// Running hash state
    hash: u64,
}

impl Fnv1a64 {
    /// FNV-1a 64-bit offset basis
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    /// FNV-1a 64-bit prime multiplier
    const FNV_PRIME: u64 = 0x100000001b3;

    /// Create a new FNV-1a hasher initialized with the offset basis
    pub fn new() -> Self {
        Fnv1a64 {
            hash: Self::OFFSET_BASIS,
        }
    }

    /// Feed data byte-by-byte into the FNV-1a hash
    pub fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.hash ^= byte as u64;
            self.hash = self.hash.wrapping_mul(Self::FNV_PRIME);
        }
    }

    /// Return the current 64-bit hash value
    pub fn finish(&self) -> u64 {
        self.hash
    }
}

/// Fast hash trait for generic usage
pub trait FastHash {
    /// Compute a 64-bit hash of the given byte slice.
    fn hash_bytes(data: &[u8]) -> u64;
    /// Compute a 64-bit hash of a UTF-8 string.
    fn hash_string(s: &str) -> u64 {
        Self::hash_bytes(s.as_bytes())
    }
}

/// xxHash implementation of FastHash
pub struct XxHasher;

impl FastHash for XxHasher {
    fn hash_bytes(data: &[u8]) -> u64 {
        let mut hasher = XxHash64::new(0);
        hasher.update(data);
        hasher.finish()
    }
}

/// FNV implementation of FastHash
pub struct FnvHasher;

impl FastHash for FnvHasher {
    fn hash_bytes(data: &[u8]) -> u64 {
        let mut hasher = Fnv1a64::new();
        hasher.update(data);
        hasher.finish()
    }
}

/// Builder for creating hashers
pub struct HashBuilder {
    /// Seed value passed to hashers that support seeding
    seed: u64,
}

impl HashBuilder {
    /// Create a new hash builder with a default seed of 0
    pub fn new() -> Self {
        HashBuilder { seed: 0 }
    }

    /// Create a new hash builder with the given seed
    pub fn with_seed(seed: u64) -> Self {
        HashBuilder { seed }
    }

    /// Build an xxHash64 hasher using this builder's seed
    pub fn build_xxhash(&self) -> XxHash64 {
        XxHash64::new(self.seed)
    }

    /// Build an FNV-1a hasher (seed is not used for FNV)
    pub fn build_fnv(&self) -> Fnv1a64 {
        Fnv1a64::new()
    }
}

impl Default for HashBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Default implementations for use with BuildHasherDefault
impl Default for XxHash64 {
    fn default() -> Self {
        XxHash64::new(0)
    }
}

impl Default for Fnv1a64 {
    fn default() -> Self {
        Fnv1a64::new()
    }
}

/// Standard Hasher implementation for use with HashMap
impl Hasher for XxHash64 {
    fn write(&mut self, bytes: &[u8]) {
        self.update(bytes);
    }

    fn finish(&self) -> u64 {
        XxHash64::finish(self)
    }
}

impl Hasher for Fnv1a64 {
    fn write(&mut self, bytes: &[u8]) {
        self.update(bytes);
    }

    fn finish(&self) -> u64 {
        Fnv1a64::finish(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hasher as StdHasher;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_xxhash() {
        let data = b"Hello, World!";
        let hash1 = XxHasher::hash_bytes(data);
        let hash2 = XxHasher::hash_bytes(b"Hello, World!");
        let hash3 = XxHasher::hash_bytes(b"Different data");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_fnv() {
        let data = b"Test data";
        let hash1 = FnvHasher::hash_bytes(data);
        let hash2 = FnvHasher::hash_bytes(b"Test data");
        let hash3 = FnvHasher::hash_bytes(b"Other data");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_hash_builder() {
        let builder = HashBuilder::with_seed(42);
        let mut hasher = builder.build_xxhash();
        hasher.update(b"test");
        let hash1 = hasher.finish();

        let mut hasher2 = XxHash64::new(42);
        hasher2.update(b"test");
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2);
    }

    // Comprehensive test suite additions

    #[test]
    fn test_xxhash_empty_input() {
        let hash1 = XxHasher::hash_bytes(b"");
        let hash2 = XxHasher::hash_bytes(&[]);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, 0); // Should not be zero
    }

    #[test]
    fn test_xxhash_single_byte() {
        let hash1 = XxHasher::hash_bytes(b"a");
        let hash2 = XxHasher::hash_bytes(b"b");

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_xxhash_incremental() {
        let mut hasher1 = XxHash64::new(0);
        hasher1.update(b"Hello");
        hasher1.update(b", ");
        hasher1.update(b"World!");
        let hash1 = hasher1.finish();

        let hash2 = XxHasher::hash_bytes(b"Hello, World!");

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_xxhash_different_seeds() {
        let data = b"test data";
        let hash1 = {
            let mut hasher = XxHash64::new(0);
            hasher.update(data);
            hasher.finish()
        };
        let hash2 = {
            let mut hasher = XxHash64::new(42);
            hasher.update(data);
            hasher.finish()
        };
        let hash3 = {
            let mut hasher = XxHash64::new(0xDEADBEEF);
            hasher.update(data);
            hasher.finish()
        };

        assert_ne!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_ne!(hash2, hash3);
    }

    #[test]
    fn test_xxhash_large_data() {
        let large_data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();

        let hash1 = XxHasher::hash_bytes(&large_data);
        let hash2 = XxHasher::hash_bytes(&large_data);

        assert_eq!(hash1, hash2);

        // Test incremental hashing with large data
        let mut hasher = XxHash64::new(0);
        for chunk in large_data.chunks(1000) {
            hasher.update(chunk);
        }
        let incremental_hash = hasher.finish();

        assert_eq!(hash1, incremental_hash);
    }

    #[test]
    fn test_xxhash_chunk_boundaries() {
        // Test data that crosses chunk boundaries (32 bytes)
        let data: Vec<u8> = (0..100).collect();

        let hash1 = XxHasher::hash_bytes(&data);

        // Hash in different chunk sizes
        let mut hasher2 = XxHash64::new(0);
        hasher2.update(&data[0..31]);
        hasher2.update(&data[31..64]);
        hasher2.update(&data[64..]);
        let hash2 = hasher2.finish();

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_fnv_empty_input() {
        let hash1 = FnvHasher::hash_bytes(b"");
        let hash2 = FnvHasher::hash_bytes(&[]);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1, Fnv1a64::OFFSET_BASIS); // FNV-1a offset basis for empty input
    }

    #[test]
    fn test_fnv_incremental() {
        let mut hasher1 = Fnv1a64::new();
        hasher1.update(b"Hello");
        hasher1.update(b", ");
        hasher1.update(b"World!");
        let hash1 = hasher1.finish();

        let hash2 = FnvHasher::hash_bytes(b"Hello, World!");

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_fnv_avalanche() {
        // Test that small changes in input cause large changes in output
        let base = b"test";
        let hash_base = FnvHasher::hash_bytes(base);

        let modified = b"Test"; // Capital T
        let hash_modified = FnvHasher::hash_bytes(modified);

        assert_ne!(hash_base, hash_modified);

        // Count different bits
        let xor_result = hash_base ^ hash_modified;
        let different_bits = xor_result.count_ones();

        // Should have good avalanche (at least 25% of bits different)
        assert!(
            different_bits >= 16,
            "Poor avalanche effect: only {} bits differ",
            different_bits
        );
    }

    #[test]
    fn test_fnv_known_vectors() {
        // Test against known FNV-1a test vectors
        assert_eq!(FnvHasher::hash_bytes(b""), 0xcbf29ce484222325);
        assert_eq!(FnvHasher::hash_bytes(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(FnvHasher::hash_bytes(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn test_hash_string_convenience() {
        let str_data = "Hello, World!";
        let bytes_data = str_data.as_bytes();

        assert_eq!(
            XxHasher::hash_string(str_data),
            XxHasher::hash_bytes(bytes_data)
        );
        assert_eq!(
            FnvHasher::hash_string(str_data),
            FnvHasher::hash_bytes(bytes_data)
        );
    }

    #[test]
    fn test_hash_builder_default() {
        let builder1 = HashBuilder::default();
        let builder2 = HashBuilder::new();

        let mut hasher1 = builder1.build_xxhash();
        let mut hasher2 = builder2.build_xxhash();

        hasher1.update(b"test");
        hasher2.update(b"test");

        assert_eq!(hasher1.finish(), hasher2.finish());
    }

    #[test]
    fn test_hash_builder_fnv() {
        let builder = HashBuilder::new();
        let mut hasher = builder.build_fnv();

        hasher.update(b"test");
        let hash = hasher.finish();

        assert_eq!(hash, FnvHasher::hash_bytes(b"test"));
    }

    #[test]
    fn test_std_hasher_trait_xxhash() {
        let mut hasher = XxHash64::new(0);

        // Test write method
        hasher.write(b"Hello");
        hasher.write(b", World!");
        let hash1 = hasher.finish();

        let hash2 = XxHasher::hash_bytes(b"Hello, World!");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_std_hasher_trait_fnv() {
        let mut hasher = Fnv1a64::new();

        // Test write method
        hasher.write(b"Hello");
        hasher.write(b", World!");
        let hash1 = hasher.finish();

        let hash2 = FnvHasher::hash_bytes(b"Hello, World!");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_consistency_across_calls() {
        let data = b"consistency test";

        // Multiple calls should give same result
        for _ in 0..100 {
            let hash1 = XxHasher::hash_bytes(data);
            let hash2 = XxHasher::hash_bytes(data);
            assert_eq!(hash1, hash2);

            let hash3 = FnvHasher::hash_bytes(data);
            let hash4 = FnvHasher::hash_bytes(data);
            assert_eq!(hash3, hash4);
        }
    }

    #[test]
    fn test_hash_collision_resistance() {
        use std::collections::HashSet;

        let mut xxhash_results = HashSet::new();
        let mut fnv_results = HashSet::new();

        // Test with various inputs
        for i in 0..10000 {
            let data = format!("test_data_{}", i);
            let bytes = data.as_bytes();

            let xx_hash = XxHasher::hash_bytes(bytes);
            let fnv_hash = FnvHasher::hash_bytes(bytes);

            // Should be no collisions in this test set
            assert!(xxhash_results.insert(xx_hash), "XXHash collision found!");
            assert!(fnv_results.insert(fnv_hash), "FNV collision found!");
        }

        assert_eq!(xxhash_results.len(), 10000);
        assert_eq!(fnv_results.len(), 10000);
    }

    #[test]
    fn test_concurrent_hashing() {
        let data = Arc::new(b"concurrent test data".to_vec());
        let mut handles = vec![];
        let results = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Spawn multiple threads
        for _ in 0..10 {
            let data = Arc::clone(&data);
            let results = Arc::clone(&results);

            let handle = thread::spawn(move || {
                let mut local_results = Vec::new();

                for _ in 0..1000 {
                    let xx_hash = XxHasher::hash_bytes(&data);
                    let fnv_hash = FnvHasher::hash_bytes(&data);
                    local_results.push((xx_hash, fnv_hash));
                }

                results.lock().unwrap().extend(local_results);
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let all_results = results.lock().unwrap();
        let first_result = all_results[0];

        // All results should be identical
        for &result in all_results.iter() {
            assert_eq!(result, first_result);
        }
    }

    #[test]
    fn test_hash_performance_patterns() {
        // Test with different data patterns

        // All zeros
        let zeros = vec![0u8; 1000];
        let hash1 = XxHasher::hash_bytes(&zeros);
        assert_ne!(hash1, 0);

        // All ones
        let ones = vec![0xFFu8; 1000];
        let hash2 = XxHasher::hash_bytes(&ones);
        assert_ne!(hash2, 0);
        assert_ne!(hash1, hash2);

        // Sequential pattern
        let sequential: Vec<u8> = (0..255).cycle().take(1000).collect();
        let hash3 = XxHasher::hash_bytes(&sequential);
        assert_ne!(hash3, hash1);
        assert_ne!(hash3, hash2);

        // Random-like pattern
        let mut random_like = Vec::new();
        let mut state = 1u64;
        for _ in 0..1000 {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            random_like.push((state >> 16) as u8);
        }
        let hash4 = XxHasher::hash_bytes(&random_like);
        assert_ne!(hash4, hash1);
        assert_ne!(hash4, hash2);
        assert_ne!(hash4, hash3);
    }

    #[test]
    fn test_hash_with_std_collections() {
        // Test using our hashers with standard collections
        use std::collections::HashMap;
        use std::hash::BuildHasherDefault;

        type XxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<XxHash64>>;

        let mut map: XxHashMap<String, i32> = HashMap::default();

        map.insert("key1".to_string(), 1);
        map.insert("key2".to_string(), 2);
        map.insert("key3".to_string(), 3);

        assert_eq!(map.get("key1"), Some(&1));
        assert_eq!(map.get("key2"), Some(&2));
        assert_eq!(map.get("key3"), Some(&3));
        assert_eq!(map.get("key4"), None);
    }

    #[test]
    fn test_hash_endianness() {
        // Test that hashing works correctly regardless of endianness
        let data = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];

        let hash1 = XxHasher::hash_bytes(&data);
        let hash2 = FnvHasher::hash_bytes(&data);

        // Reverse the data
        let mut reversed = data;
        reversed.reverse();

        let hash3 = XxHasher::hash_bytes(&reversed);
        let hash4 = FnvHasher::hash_bytes(&reversed);

        // Hashes should be different for reversed data
        assert_ne!(hash1, hash3);
        assert_ne!(hash2, hash4);
    }

    #[test]
    fn test_xxhash_round_function() {
        let hasher = XxHash64::new(0);

        // Test the internal round function with known values
        let input = 0x123456789ABCDEF0;
        let result = hasher.round(0, input);

        // Result should be deterministic
        let expected = hasher.round(0, input);
        assert_eq!(result, expected);

        // Different inputs should give different results
        let result2 = hasher.round(0, input + 1);
        assert_ne!(result, result2);
    }

    #[test]
    fn test_hash_length_sensitivity() {
        // Test that hash is sensitive to input length
        let base = "test";
        let extended = "test_extended";

        let hash1 = XxHasher::hash_bytes(base.as_bytes());
        let hash2 = XxHasher::hash_bytes(extended.as_bytes());

        assert_ne!(hash1, hash2);

        let hash3 = FnvHasher::hash_bytes(base.as_bytes());
        let hash4 = FnvHasher::hash_bytes(extended.as_bytes());

        assert_ne!(hash3, hash4);
    }

    #[test]
    fn test_hash_builder_different_seeds() {
        let builder1 = HashBuilder::with_seed(0);
        let builder2 = HashBuilder::with_seed(42);
        let builder3 = HashBuilder::with_seed(0xDEADBEEF);

        let data = b"test data";

        let mut h1 = builder1.build_xxhash();
        let mut h2 = builder2.build_xxhash();
        let mut h3 = builder3.build_xxhash();

        h1.update(data);
        h2.update(data);
        h3.update(data);

        let hash1 = h1.finish();
        let hash2 = h2.finish();
        let hash3 = h3.finish();

        // All should be different due to different seeds
        assert_ne!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_ne!(hash2, hash3);
    }

    #[test]
    fn test_zero_length_updates() {
        let mut hasher1 = XxHash64::new(0);
        hasher1.update(b"test");
        hasher1.update(b""); // Zero length update
        hasher1.update(b"data");
        let hash1 = hasher1.finish();

        let hash2 = XxHasher::hash_bytes(b"testdata");

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_distribution() {
        // Test that hash function has good distribution
        let mut buckets = vec![0usize; 256];

        for i in 0..25600 {
            let data = format!("item_{}", i);
            let hash = XxHasher::hash_bytes(data.as_bytes());
            let bucket = (hash % 256) as usize;
            buckets[bucket] += 1;
        }

        // Check that no bucket is empty and distribution is reasonably uniform
        let min_count = *buckets.iter().min().unwrap();
        let max_count = *buckets.iter().max().unwrap();

        assert!(min_count > 0, "Hash distribution has empty buckets");
        assert!(max_count < 200, "Hash distribution is too skewed"); // Allow some variation

        // Check that the ratio isn't too extreme
        let ratio = max_count as f64 / min_count as f64;
        assert!(ratio < 3.0, "Hash distribution ratio too high: {}", ratio);
    }
}

//! UUID v4 implementation - replaces external uuid crate
//! 
//! Provides a lightweight UUID v4 generator using the existing rand crate.
//! UUID v4 is randomly generated and doesn't require MAC address or timestamp.

use rand::Rng;
use std::fmt;

/// A 128-bit UUID (Universally Unique Identifier)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Uuid {
    bytes: [u8; 16],
}

impl Uuid {
    /// Creates a new random UUID v4
    pub fn new_v4() -> Self {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 16];
        rng.fill(&mut bytes);
        
        // Set version (4) and variant bits according to RFC 4122
        bytes[6] = (bytes[6] & 0x0f) | 0x40; // Version 4
        bytes[8] = (bytes[8] & 0x3f) | 0x80; // Variant 10
        
        Uuid { bytes }
    }
    
    /// Creates a UUID from raw bytes
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Uuid { bytes }
    }
    
    /// Returns the raw bytes of the UUID
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.bytes
    }
    
    /// Converts UUID to a hyphenated string
    pub fn to_hyphenated_string(&self) -> String {
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.bytes[0], self.bytes[1], self.bytes[2], self.bytes[3],
            self.bytes[4], self.bytes[5],
            self.bytes[6], self.bytes[7],
            self.bytes[8], self.bytes[9],
            self.bytes[10], self.bytes[11], self.bytes[12], self.bytes[13], self.bytes[14], self.bytes[15]
        )
    }
    
    /// Converts UUID to a simple hex string (no hyphens)
    pub fn to_simple_string(&self) -> String {
        self.bytes.iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
    
    /// Parses a UUID from a hyphenated string
    pub fn parse(s: &str) -> Result<Self, UuidError> {
        let cleaned: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        
        if cleaned.len() != 32 {
            return Err(UuidError::InvalidLength);
        }
        
        let mut bytes = [0u8; 16];
        for i in 0..16 {
            let hex = &cleaned[i * 2..i * 2 + 2];
            bytes[i] = u8::from_str_radix(hex, 16)
                .map_err(|_| UuidError::InvalidHex)?;
        }
        
        Ok(Uuid { bytes })
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hyphenated_string())
    }
}

impl Default for Uuid {
    fn default() -> Self {
        Uuid::new_v4()
    }
}

/// UUID generator with configurable random source
pub struct UuidGenerator {
    // Could be extended to support different UUID versions
}

impl UuidGenerator {
    pub fn new() -> Self {
        UuidGenerator {}
    }
    
    /// Generate a new UUID v4
    pub fn generate(&self) -> Uuid {
        Uuid::new_v4()
    }
    
    /// Generate multiple UUIDs
    pub fn generate_batch(&self, count: usize) -> Vec<Uuid> {
        (0..count).map(|_| Uuid::new_v4()).collect()
    }
}

impl Default for UuidGenerator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub enum UuidError {
    InvalidLength,
    InvalidHex,
}

impl fmt::Display for UuidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UuidError::InvalidLength => write!(f, "Invalid UUID length"),
            UuidError::InvalidHex => write!(f, "Invalid hexadecimal character"),
        }
    }
}

impl std::error::Error for UuidError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::thread;
    use std::sync::Arc;
    
    #[test]
    fn test_uuid_v4_generation() {
        let uuid1 = Uuid::new_v4();
        let uuid2 = Uuid::new_v4();
        
        // UUIDs should be different
        assert_ne!(uuid1, uuid2);
        
        // Check version bits
        assert_eq!(uuid1.bytes[6] & 0xf0, 0x40);
        assert_eq!(uuid1.bytes[8] & 0xc0, 0x80);
    }
    
    #[test]
    fn test_uuid_string_conversion() {
        let uuid = Uuid::new_v4();
        let hyphenated = uuid.to_hyphenated_string();
        let simple = uuid.to_simple_string();
        
        assert_eq!(hyphenated.len(), 36); // With hyphens
        assert_eq!(simple.len(), 32); // Without hyphens
        
        // Parse back
        let parsed = Uuid::parse(&hyphenated).unwrap();
        assert_eq!(uuid, parsed);
    }
    
    #[test]
    fn test_uuid_generator() {
        let generator = UuidGenerator::new();
        let batch = generator.generate_batch(10);
        
        assert_eq!(batch.len(), 10);
        
        // All should be unique
        for i in 0..batch.len() {
            for j in i+1..batch.len() {
                assert_ne!(batch[i], batch[j]);
            }
        }
    }
    
    // Comprehensive test suite additions
    
    #[test]
    fn test_uuid_version_and_variant_bits() {
        for _ in 0..1000 {
            let uuid = Uuid::new_v4();
            
            // Version 4 check (bits 12-15 of time_hi_and_version)
            assert_eq!(uuid.bytes[6] & 0xf0, 0x40, "Version bits should be 0100");
            
            // Variant check (bits 6-7 of clock_seq_hi_and_reserved)
            assert_eq!(uuid.bytes[8] & 0xc0, 0x80, "Variant bits should be 10");
        }
    }
    
    #[test]
    fn test_uuid_from_bytes() {
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let uuid = Uuid::from_bytes(bytes);
        
        assert_eq!(uuid.as_bytes(), &bytes);
    }
    
    #[test]
    fn test_uuid_display_format() {
        let bytes = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
                     0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let uuid = Uuid::from_bytes(bytes);
        let display_str = format!("{}", uuid);
        let expected = "12345678-9abc-def0-1122-334455667788";
        
        assert_eq!(display_str, expected);
        assert_eq!(uuid.to_hyphenated_string(), expected);
    }
    
    #[test]
    fn test_uuid_simple_string() {
        let bytes = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
                     0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let uuid = Uuid::from_bytes(bytes);
        let simple = uuid.to_simple_string();
        let expected = "123456789abcdef01122334455667788";
        
        assert_eq!(simple, expected);
        assert_eq!(simple.len(), 32);
    }
    
    #[test]
    fn test_uuid_parse_hyphenated() {
        let uuid_str = "12345678-9abc-def0-1122-334455667788";
        let uuid = Uuid::parse(uuid_str).unwrap();
        let expected_bytes = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
                              0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        
        assert_eq!(uuid.as_bytes(), &expected_bytes);
    }
    
    #[test]
    fn test_uuid_parse_simple() {
        let uuid_str = "123456789abcdef01122334455667788";
        let uuid = Uuid::parse(uuid_str).unwrap();
        let expected_bytes = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
                              0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        
        assert_eq!(uuid.as_bytes(), &expected_bytes);
    }
    
    #[test]
    fn test_uuid_parse_mixed_case() {
        let uuid_str = "12345678-9ABC-DEF0-1122-334455667788";
        let uuid = Uuid::parse(uuid_str).unwrap();
        let expected_bytes = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
                              0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        
        assert_eq!(uuid.as_bytes(), &expected_bytes);
    }
    
    #[test]
    fn test_uuid_parse_with_extra_hyphens() {
        let uuid_str = "12-34-56-78-9abc-def0-1122-334455667788";
        let uuid = Uuid::parse(uuid_str).unwrap();
        let expected_bytes = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
                              0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        
        assert_eq!(uuid.as_bytes(), &expected_bytes);
    }
    
    #[test]
    fn test_uuid_parse_errors() {
        // Too short
        assert!(matches!(Uuid::parse("123"), Err(UuidError::InvalidLength)));
        
        // Too long
        assert!(matches!(Uuid::parse("123456789abcdef01122334455667788ff"), Err(UuidError::InvalidLength)));
        
        // Invalid hex characters
        assert!(matches!(Uuid::parse("12345678-9abc-def0-1122-33445566778g"), Err(UuidError::InvalidHex)));
        assert!(matches!(Uuid::parse("xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"), Err(UuidError::InvalidHex)));
        
        // Empty string
        assert!(matches!(Uuid::parse(""), Err(UuidError::InvalidLength)));
        
        // Only hyphens
        assert!(matches!(Uuid::parse("------------------------------------"), Err(UuidError::InvalidLength)));
    }
    
    #[test]
    fn test_uuid_roundtrip_conversions() {
        for _ in 0..100 {
            let original = Uuid::new_v4();
            
            // Test hyphenated roundtrip
            let hyphenated = original.to_hyphenated_string();
            let parsed_hyphenated = Uuid::parse(&hyphenated).unwrap();
            assert_eq!(original, parsed_hyphenated);
            
            // Test simple roundtrip
            let simple = original.to_simple_string();
            let parsed_simple = Uuid::parse(&simple).unwrap();
            assert_eq!(original, parsed_simple);
        }
    }
    
    #[test]
    fn test_uuid_uniqueness_stress() {
        let mut uuids = HashSet::new();
        let count = 100_000;
        
        for _ in 0..count {
            let uuid = Uuid::new_v4();
            assert!(uuids.insert(uuid), "Generated duplicate UUID!");
        }
        
        assert_eq!(uuids.len(), count);
    }
    
    #[test]
    fn test_uuid_concurrent_generation() {
        let count = 10_000;
        let thread_count = 10;
        let mut handles = vec![];
        let results = Arc::new(std::sync::Mutex::new(Vec::new()));
        
        for _ in 0..thread_count {
            let results = Arc::clone(&results);
            let handle = thread::spawn(move || {
                let mut local_uuids = Vec::new();
                for _ in 0..count / thread_count {
                    local_uuids.push(Uuid::new_v4());
                }
                results.lock().unwrap().extend(local_uuids);
            });
            handles.push(handle);
        }
        
        for handle in handles {
            handle.join().unwrap();
        }
        
        let uuids = results.lock().unwrap();
        let mut unique_uuids = HashSet::new();
        
        for uuid in uuids.iter() {
            assert!(unique_uuids.insert(*uuid), "Found duplicate UUID in concurrent generation!");
        }
        
        assert_eq!(unique_uuids.len(), count);
    }
    
    #[test]
    fn test_uuid_generator_consistency() {
        let generator = UuidGenerator::new();
        
        for _ in 0..1000 {
            let uuid = generator.generate();
            assert_eq!(uuid.bytes[6] & 0xf0, 0x40); // Version 4
            assert_eq!(uuid.bytes[8] & 0xc0, 0x80); // Variant
        }
    }
    
    #[test]
    fn test_uuid_generator_batch_uniqueness() {
        let generator = UuidGenerator::new();
        
        // Test various batch sizes
        for batch_size in [1, 5, 10, 50, 100, 1000] {
            let batch = generator.generate_batch(batch_size);
            assert_eq!(batch.len(), batch_size);
            
            let mut unique_set = HashSet::new();
            for uuid in &batch {
                assert!(unique_set.insert(*uuid), "Duplicate UUID in batch!");
            }
            
            assert_eq!(unique_set.len(), batch_size);
        }
    }
    
    #[test]
    fn test_uuid_generator_empty_batch() {
        let generator = UuidGenerator::new();
        let batch = generator.generate_batch(0);
        assert_eq!(batch.len(), 0);
    }
    
    #[test]
    fn test_uuid_generator_large_batch() {
        let generator = UuidGenerator::new();
        let batch = generator.generate_batch(10_000);
        assert_eq!(batch.len(), 10_000);
        
        let mut unique_set = HashSet::new();
        for uuid in &batch {
            assert!(unique_set.insert(*uuid));
        }
        assert_eq!(unique_set.len(), 10_000);
    }
    
    #[test]
    fn test_uuid_default() {
        let uuid1 = Uuid::default();
        let uuid2 = Uuid::default();
        
        // Default UUIDs should be different (they're randomly generated)
        assert_ne!(uuid1, uuid2);
        
        // Should have proper version/variant bits
        assert_eq!(uuid1.bytes[6] & 0xf0, 0x40);
        assert_eq!(uuid1.bytes[8] & 0xc0, 0x80);
    }
    
    #[test]
    fn test_uuid_generator_default() {
        let generator1 = UuidGenerator::default();
        let generator2 = UuidGenerator::default();
        
        // Different generators should produce different UUIDs
        let uuid1 = generator1.generate();
        let uuid2 = generator2.generate();
        assert_ne!(uuid1, uuid2);
    }
    
    #[test]
    fn test_uuid_error_display() {
        let invalid_length_error = UuidError::InvalidLength;
        let invalid_hex_error = UuidError::InvalidHex;
        
        assert_eq!(format!("{}", invalid_length_error), "Invalid UUID length");
        assert_eq!(format!("{}", invalid_hex_error), "Invalid hexadecimal character");
    }
    
    #[test]
    fn test_uuid_error_debug() {
        let error = UuidError::InvalidLength;
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("InvalidLength"));
    }
    
    #[test]
    fn test_uuid_hash_consistency() {
        use std::collections::HashMap;
        
        let uuid = Uuid::new_v4();
        let mut map = HashMap::new();
        
        map.insert(uuid, "test_value");
        assert_eq!(map.get(&uuid), Some(&"test_value"));
        
        // Same UUID should hash to same value
        let uuid_copy = Uuid::from_bytes(*uuid.as_bytes());
        assert_eq!(map.get(&uuid_copy), Some(&"test_value"));
    }
    
    #[test]
    fn test_uuid_equality_edge_cases() {
        let bytes = [0; 16];
        let uuid1 = Uuid::from_bytes(bytes);
        let uuid2 = Uuid::from_bytes(bytes);
        
        assert_eq!(uuid1, uuid2);
        
        let mut different_bytes = bytes;
        different_bytes[15] = 1;
        let uuid3 = Uuid::from_bytes(different_bytes);
        
        assert_ne!(uuid1, uuid3);
    }
    
    #[test]
    fn test_uuid_memory_layout() {
        let uuid = Uuid::new_v4();
        
        // UUID should be exactly 16 bytes
        assert_eq!(std::mem::size_of::<Uuid>(), 16);
        assert_eq!(uuid.as_bytes().len(), 16);
    }
    
    #[test]
    fn test_uuid_parse_boundary_conditions() {
        // Test all zeros
        let all_zeros = "00000000-0000-0000-0000-000000000000";
        let uuid = Uuid::parse(all_zeros).unwrap();
        assert_eq!(uuid.as_bytes(), &[0; 16]);
        
        // Test all F's
        let all_fs = "ffffffff-ffff-ffff-ffff-ffffffffffff";
        let uuid = Uuid::parse(all_fs).unwrap();
        assert_eq!(uuid.as_bytes(), &[0xff; 16]);
    }
    
    #[test]
    fn test_uuid_string_consistency() {
        for _ in 0..100 {
            let uuid = Uuid::new_v4();
            let hyphenated = uuid.to_hyphenated_string();
            let simple = uuid.to_simple_string();
            
            // Remove hyphens from hyphenated format should equal simple format
            let hyphenated_no_dash: String = hyphenated.chars().filter(|&c| c != '-').collect();
            assert_eq!(hyphenated_no_dash, simple);
            
            // Both should be lowercase
            assert_eq!(hyphenated, hyphenated.to_lowercase());
            assert_eq!(simple, simple.to_lowercase());
        }
    }
}
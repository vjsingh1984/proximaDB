// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Base62 encoding for compact collection IDs
//! 
//! Uses case-sensitive alphanumeric encoding (0-9, A-Z, a-z) to convert
//! microsecond timestamps into compact strings.

const BASE62_CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Encode a u64 value to base62 string
pub fn encode(mut num: u64) -> String {
    if num == 0 {
        return "0".to_string();
    }
    
    let mut result = Vec::new();
    while num > 0 {
        result.push(BASE62_CHARS[(num % 62) as usize]);
        num /= 62;
    }
    
    result.reverse();
    String::from_utf8(result).unwrap()
}

/// Decode a base62 string to u64 value
pub fn decode(s: &str) -> Result<u64, String> {
    let mut result = 0u64;
    
    for &byte in s.as_bytes() {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'A'..=b'Z' => byte - b'A' + 10,
            b'a'..=b'z' => byte - b'a' + 36,
            _ => return Err(format!("Invalid base62 character: {}", byte as char)),
        };
        
        result = result
            .checked_mul(62)
            .and_then(|r| r.checked_add(digit as u64))
            .ok_or_else(|| "Base62 decode overflow".to_string())?;
    }
    
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_base62_encoding() {
        assert_eq!(encode(0), "0");
        assert_eq!(encode(61), "z");
        assert_eq!(encode(62), "10");
        assert_eq!(encode(3843), "zz");
        
        // Test millisecond timestamp
        let timestamp_ms = 1736634592000u64; // Example millisecond timestamp
        let encoded_ms = encode(timestamp_ms);
        assert!(encoded_ms.len() < 9); // Should be much shorter than UUID
        println!("Millisecond timestamp {} -> base62: {} ({} chars)", timestamp_ms, encoded_ms, encoded_ms.len());
        
        // Test microsecond timestamp for comparison
        let timestamp_us = 1736634592000000u64; // Example microsecond timestamp
        let encoded_us = encode(timestamp_us);
        assert!(encoded_us.len() < 12); // Should be much shorter than UUID
        println!("Microsecond timestamp {} -> base62: {} ({} chars)", timestamp_us, encoded_us, encoded_us.len());
        
        // Test round trip
        assert_eq!(decode(&encoded_ms).unwrap(), timestamp_ms);
        assert_eq!(decode(&encoded_us).unwrap(), timestamp_us);
    }
    
    #[test]
    fn test_base62_decoding() {
        assert_eq!(decode("0").unwrap(), 0);
        assert_eq!(decode("z").unwrap(), 61);
        assert_eq!(decode("10").unwrap(), 62);
        assert_eq!(decode("zz").unwrap(), 3843);
        
        // Test error cases
        assert!(decode("!").is_err());
        assert!(decode("@").is_err());
    }
}
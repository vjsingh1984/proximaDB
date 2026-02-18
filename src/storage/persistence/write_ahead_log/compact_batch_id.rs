// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Compact BatchId Implementation
//!
//! Uses timestamp + counter approach for minimal storage overhead:
//! - 8 bytes: timestamp (milliseconds since epoch)
//! - 2 bytes: counter (supports 65,535 batches per millisecond)
//! - Total: 10 bytes vs ~100+ bytes for string-based BatchId

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Global counter for batch IDs within the same millisecond
static BATCH_COUNTER: AtomicU16 = AtomicU16::new(0);

/// Last timestamp used for batch ID generation
static LAST_TIMESTAMP: AtomicU64 = AtomicU64::new(0);

use serde::{Deserialize, Serialize};

/// Compact BatchId - only 10 bytes total
///
/// Serializes as a fixed 10-byte array for optimal storage:
/// - Bytes 0-7: timestamp_ms (little-endian u64)
/// - Bytes 8-9: counter (little-endian u16)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CompactBatchId {
    /// Milliseconds since epoch (8 bytes)
    timestamp_ms: u64,
    /// Counter within the same millisecond (2 bytes)
    counter: u16,
}

// Custom serialization to ensure compact 10-byte representation
impl Serialize for CompactBatchId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as bytes for compact storage
        if serializer.is_human_readable() {
            // For JSON/YAML etc, use base62 string
            serializer.serialize_str(&self.to_base62())
        } else {
            // For binary formats, use raw bytes
            serializer.serialize_bytes(&self.to_bytes())
        }
    }
}

impl<'de> Deserialize<'de> for CompactBatchId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, Visitor};

        struct CompactBatchIdVisitor;

        impl<'de> Visitor<'de> for CompactBatchIdVisitor {
            type Value = CompactBatchId;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a 10-byte array or base62 string")
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                CompactBatchId::from_bytes(v).ok_or_else(|| E::custom("invalid byte array"))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                CompactBatchId::from_base62(v).ok_or_else(|| E::custom("invalid base62 string"))
            }
        }

        if deserializer.is_human_readable() {
            deserializer.deserialize_str(CompactBatchIdVisitor)
        } else {
            deserializer.deserialize_bytes(CompactBatchIdVisitor)
        }
    }
}

impl CompactBatchId {
    /// Generate a new unique CompactBatchId
    pub fn new() -> Self {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // Use compare_exchange to atomically update timestamp and get counter
        let counter;
        loop {
            let last_ts = LAST_TIMESTAMP.load(Ordering::Acquire);

            if now_ms == last_ts {
                // Same millisecond, just increment counter
                counter = BATCH_COUNTER.fetch_add(1, Ordering::SeqCst);
                break;
            } else if now_ms > last_ts {
                // Try to update to new millisecond
                match LAST_TIMESTAMP.compare_exchange(
                    last_ts,
                    now_ms,
                    Ordering::SeqCst,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        // Successfully updated timestamp - this thread gets counter 0, set counter to 1 for next thread
                        BATCH_COUNTER.store(1, Ordering::SeqCst);
                        counter = 0;
                        break;
                    }
                    Err(_) => {
                        // Another thread updated it, retry
                        continue;
                    }
                }
            } else {
                // Clock went backwards? Use current counter
                counter = BATCH_COUNTER.fetch_add(1, Ordering::SeqCst);
                break;
            }
        }

        // Handle counter overflow (extremely rare - would need 65k+ batches/ms)
        if counter == u16::MAX {
            // Wait for next millisecond
            std::thread::sleep(std::time::Duration::from_micros(100));
            return Self::new();
        }

        Self {
            timestamp_ms: now_ms,
            counter,
        }
    }

    /// Create from components (for deserialization)
    pub fn from_components(timestamp_ms: u64, counter: u16) -> Self {
        Self {
            timestamp_ms,
            counter,
        }
    }

    /// Convert to bytes for storage (10 bytes total)
    pub fn to_bytes(&self) -> [u8; 10] {
        let mut bytes = [0u8; 10];
        bytes[0..8].copy_from_slice(&self.timestamp_ms.to_le_bytes());
        bytes[8..10].copy_from_slice(&self.counter.to_le_bytes());
        bytes
    }

    /// Create from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 10 {
            return None;
        }

        let timestamp_ms = u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let counter = u16::from_le_bytes([bytes[8], bytes[9]]);

        Some(Self {
            timestamp_ms,
            counter,
        })
    }

    /// Convert to base62 string for human-readable representation
    /// This is more compact than UUID strings
    pub fn to_base62(&self) -> String {
        // Combine timestamp and counter into a single u128
        let combined = ((self.timestamp_ms as u128) << 16) | (self.counter as u128);
        base62_encode(combined)
    }

    /// Parse from base62 string
    pub fn from_base62(s: &str) -> Option<Self> {
        let combined = base62_decode(s)?;
        let timestamp_ms = (combined >> 16) as u64;
        let counter = (combined & 0xFFFF) as u16;
        Some(Self {
            timestamp_ms,
            counter,
        })
    }

    /// Get timestamp in milliseconds
    pub fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }

    /// Get counter value
    pub fn counter(&self) -> u16 {
        self.counter
    }
}

/// Base62 encoding for compact string representation
const BASE62_CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn base62_encode(mut num: u128) -> String {
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

fn base62_decode(s: &str) -> Option<u128> {
    let mut result = 0u128;
    for &byte in s.as_bytes() {
        let digit = BASE62_CHARS.iter().position(|&b| b == byte)? as u128;
        result = result.checked_mul(62)?.checked_add(digit)?;
    }
    Some(result)
}

use std::sync::atomic::AtomicU64;

impl Default for CompactBatchId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CompactBatchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_base62())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_batch_id() {
        let id1 = CompactBatchId::new();
        let id2 = CompactBatchId::new();

        // Should be different
        assert_ne!(id1, id2);

        // Test serialization
        let bytes = id1.to_bytes();
        assert_eq!(bytes.len(), 10);

        let restored = CompactBatchId::from_bytes(&bytes).unwrap();
        assert_eq!(id1, restored);

        // Test base62
        let base62 = id1.to_base62();
        let from_base62 = CompactBatchId::from_base62(&base62).unwrap();
        assert_eq!(id1, from_base62);
    }

    #[test]
    fn test_counter_increment() {
        let ids: Vec<_> = (0..100).map(|_| CompactBatchId::new()).collect();

        // All IDs should be unique
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j]);
            }
        }
    }
}

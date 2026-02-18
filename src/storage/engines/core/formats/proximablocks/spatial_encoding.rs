//! 512-bit Spatial Encoding for High-Dimensional Embeddings
//!
//! Supports up to 64 PCA dimensions for modern embeddings (BGE-768, OpenAI-1536).
//! Provides unified spatial code types (64/128/256/512-bit) with automatic selection.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;

// ============================================================================
// U512: 512-bit Unsigned Integer
// ============================================================================

/// 512-bit unsigned integer for high-dimensional spatial codes.
///
/// Supports up to 64 dimensions at 8 bits/dimension for Z-Order encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct U512 {
    /// Four 128-bit parts: [bits 0-127, 128-255, 256-383, 384-511]
    pub parts: [u128; 4],
}

impl U512 {
    pub const ZERO: Self = Self { parts: [0; 4] };
    pub const MAX: Self = Self {
        parts: [u128::MAX; 4],
    };

    #[inline]
    pub const fn new(parts: [u128; 4]) -> Self {
        Self { parts }
    }

    #[inline]
    pub const fn from_u64(val: u64) -> Self {
        Self {
            parts: [val as u128, 0, 0, 0],
        }
    }

    #[inline]
    pub const fn from_u128(val: u128) -> Self {
        Self {
            parts: [val, 0, 0, 0],
        }
    }

    /// Check if value is in range [min, max] (inclusive)
    #[inline]
    pub fn in_range(&self, min: &Self, max: &Self) -> bool {
        self >= min && self <= max
    }

    /// Saturating subtraction (returns ZERO on underflow)
    pub fn saturating_sub(&self, other: &Self) -> Self {
        let mut result = [0u128; 4];
        let mut borrow = false;

        for i in 0..4 {
            let (diff, new_borrow) = if borrow {
                let (sub1, b1) = self.parts[i].overflowing_sub(1);
                let (sub2, b2) = sub1.overflowing_sub(other.parts[i]);
                (sub2, b1 || b2)
            } else {
                self.parts[i].overflowing_sub(other.parts[i])
            };

            result[i] = diff;
            borrow = new_borrow;
        }

        if borrow {
            Self::ZERO // Saturate to zero
        } else {
            Self { parts: result }
        }
    }

    /// Saturating addition (returns MAX on overflow)
    pub fn saturating_add(&self, other: &Self) -> Self {
        let mut result = [0u128; 4];
        let mut carry = false;

        for i in 0..4 {
            let (sum, new_carry) = if carry {
                let (add1, c1) = self.parts[i].overflowing_add(other.parts[i]);
                let (add2, c2) = add1.overflowing_add(1);
                (add2, c1 || c2)
            } else {
                self.parts[i].overflowing_add(other.parts[i])
            };

            result[i] = sum;
            carry = new_carry;
        }

        if carry {
            Self::MAX // Saturate to max
        } else {
            Self { parts: result }
        }
    }

    /// Absolute difference (always >= 0)
    pub fn abs_diff(&self, other: &Self) -> Self {
        if self >= other {
            self.saturating_sub(other)
        } else {
            other.saturating_sub(self)
        }
    }
}

impl PartialOrd for U512 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for U512 {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare from most significant to least significant
        for i in (0..4).rev() {
            match self.parts[i].cmp(&other.parts[i]) {
                Ordering::Equal => continue,
                other => return other,
            }
        }
        Ordering::Equal
    }
}

impl Serialize for U512 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut bytes = Vec::with_capacity(64);
        for part in &self.parts {
            bytes.extend_from_slice(&part.to_le_bytes());
        }
        serializer.serialize_bytes(&bytes)
    }
}

impl<'de> Deserialize<'de> for U512 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        if bytes.len() != 64 {
            return Err(serde::de::Error::custom(format!(
                "Expected 64 bytes for U512, got {}",
                bytes.len()
            )));
        }

        let mut parts = [0u128; 4];
        for (i, part) in parts.iter_mut().enumerate() {
            let start = i * 16;
            let end = start + 16;
            *part = u128::from_le_bytes(bytes[start..end].try_into().unwrap());
        }

        Ok(Self::new(parts))
    }
}

// ============================================================================
// SpatialCode: Unified Multi-Width Spatial Code
// ============================================================================

/// Unified spatial code supporting multiple bit widths.
///
/// Automatically selects appropriate width based on dimensionality:
/// - 64-bit: 1-8 dimensions @ 8 bits/dim
/// - 128-bit: 9-16 dimensions @ 8 bits/dim
/// - 256-bit: 17-32 dimensions @ 8 bits/dim
/// - 512-bit: 33-64 dimensions @ 8 bits/dim
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpatialCode {
    /// 64-bit code: up to 8 dims @ 8 bits/dim = 256 discrete values
    Code64(u64),

    /// 128-bit code: up to 16 dims @ 8 bits/dim = 256 discrete values
    Code128(u128),

    /// 256-bit code: up to 32 dims @ 8 bits/dim = 256 discrete values
    Code256 { low: u128, high: u128 },

    /// 512-bit code: up to 64 dims @ 8 bits/dim = 256 discrete values
    Code512(U512),
}

impl SpatialCode {
    /// Check if code is in range [min, max] (inclusive)
    pub fn in_range(&self, min: &Self, max: &Self) -> bool {
        match (self, min, max) {
            (Self::Code64(v), Self::Code64(mn), Self::Code64(mx)) => v >= mn && v <= mx,
            (Self::Code128(v), Self::Code128(mn), Self::Code128(mx)) => v >= mn && v <= mx,
            (
                Self::Code256 { low: vl, high: vh },
                Self::Code256 {
                    low: mnl,
                    high: mnh,
                },
                Self::Code256 {
                    low: mxl,
                    high: mxh,
                },
            ) => {
                // Compare high parts first, then low parts
                match vh.cmp(mnh) {
                    Ordering::Greater => match vh.cmp(mxh) {
                        Ordering::Less => true,
                        Ordering::Equal => vl <= mxl,
                        Ordering::Greater => false,
                    },
                    Ordering::Equal => vl >= mnl && vl <= mxl,
                    Ordering::Less => false,
                }
            }
            (Self::Code512(v), Self::Code512(mn), Self::Code512(mx)) => v.in_range(mn, mx),
            _ => false, // Type mismatch
        }
    }

    /// Calculate epsilon (search radius) as percentage of range
    ///
    /// # Arguments
    /// * `other` - Other code to calculate range from
    /// * `percentage` - Percentage of range (0.0-100.0)
    /// * `min_epsilon` - Minimum epsilon value
    pub fn epsilon(&self, other: &Self, percentage: f32, min_epsilon: u64) -> Self {
        match (self, other) {
            (Self::Code64(a), Self::Code64(b)) => {
                let range = a.abs_diff(*b);
                let epsilon = ((range as f64) * (percentage as f64 / 100.0)) as u64;
                Self::Code64(epsilon.max(min_epsilon))
            }
            (Self::Code128(a), Self::Code128(b)) => {
                let range = a.abs_diff(*b);
                let epsilon = ((range as f64) * (percentage as f64 / 100.0)) as u128;
                Self::Code128(epsilon.max(min_epsilon as u128))
            }
            (Self::Code256 { low: _, high: ah }, Self::Code256 { low: _, high: bh }) => {
                // Approximate: use high part for range calculation
                let range = ah.abs_diff(*bh);
                let epsilon_high = ((range as f64) * (percentage as f64 / 100.0)) as u128;
                Self::Code256 {
                    low: 0,
                    high: epsilon_high.max(min_epsilon as u128),
                }
            }
            (Self::Code512(a), Self::Code512(b)) => {
                let range = a.abs_diff(b);
                // Approximate: use first part for range calculation
                let epsilon_val = ((range.parts[0] as f64) * (percentage as f64 / 100.0)) as u128;
                Self::Code512(U512::new([epsilon_val.max(min_epsilon as u128), 0, 0, 0]))
            }
            _ => self.clone(), // Type mismatch, return self
        }
    }

    /// Saturating subtraction
    pub fn saturating_sub(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Code64(a), Self::Code64(b)) => Self::Code64(a.saturating_sub(*b)),
            (Self::Code128(a), Self::Code128(b)) => Self::Code128(a.saturating_sub(*b)),
            (Self::Code256 { low: al, high: ah }, Self::Code256 { low: bl, high: bh }) => {
                let (low, borrow) = al.overflowing_sub(*bl);
                let high = if borrow {
                    ah.saturating_sub(*bh).saturating_sub(1)
                } else {
                    ah.saturating_sub(*bh)
                };
                Self::Code256 { low, high }
            }
            (Self::Code512(a), Self::Code512(b)) => Self::Code512(a.saturating_sub(b)),
            _ => self.clone(),
        }
    }

    /// Saturating addition
    pub fn saturating_add(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Code64(a), Self::Code64(b)) => Self::Code64(a.saturating_add(*b)),
            (Self::Code128(a), Self::Code128(b)) => Self::Code128(a.saturating_add(*b)),
            (Self::Code256 { low: al, high: ah }, Self::Code256 { low: bl, high: bh }) => {
                let (low, carry) = al.overflowing_add(*bl);
                let high = if carry {
                    ah.saturating_add(*bh).saturating_add(1)
                } else {
                    ah.saturating_add(*bh)
                };
                Self::Code256 { low, high }
            }
            (Self::Code512(a), Self::Code512(b)) => Self::Code512(a.saturating_add(b)),
            _ => self.clone(),
        }
    }

    /// Get the code type
    pub fn code_type(&self) -> CodeType {
        match self {
            Self::Code64(_) => CodeType::Bits64,
            Self::Code128(_) => CodeType::Bits128,
            Self::Code256 { .. } => CodeType::Bits256,
            Self::Code512(_) => CodeType::Bits512,
        }
    }
}

impl PartialOrd for SpatialCode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SpatialCode {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Code64(a), Self::Code64(b)) => a.cmp(b),
            (Self::Code128(a), Self::Code128(b)) => a.cmp(b),
            (Self::Code256 { low: al, high: ah }, Self::Code256 { low: bl, high: bh }) => {
                // Compare high parts first (most significant), then low parts
                match ah.cmp(bh) {
                    Ordering::Equal => al.cmp(bl),
                    other => other,
                }
            }
            (Self::Code512(a), Self::Code512(b)) => a.cmp(b),
            // Mixed types: order by code type
            (a, b) => (a.code_type() as u8).cmp(&(b.code_type() as u8)),
        }
    }
}

impl std::fmt::Display for SpatialCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Code64(code) => write!(f, "Code64(0x{:016x})", code),
            Self::Code128(code) => write!(f, "Code128(0x{:032x})", code),
            Self::Code256 { low, high } => {
                write!(f, "Code256(0x{:032x}{:032x})", high, low)
            }
            Self::Code512(code) => {
                write!(
                    f,
                    "Code512(0x{:032x}{:032x}{:032x}{:032x})",
                    code.parts[3], code.parts[2], code.parts[1], code.parts[0]
                )
            }
        }
    }
}

impl Serialize for SpatialCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Code64(code) => {
                let mut bytes = vec![1u8]; // Type tag: 64-bit
                bytes.extend_from_slice(&code.to_le_bytes());
                serializer.serialize_bytes(&bytes)
            }
            Self::Code128(code) => {
                let mut bytes = vec![2u8]; // Type tag: 128-bit
                bytes.extend_from_slice(&code.to_le_bytes());
                serializer.serialize_bytes(&bytes)
            }
            Self::Code256 { low, high } => {
                let mut bytes = vec![3u8]; // Type tag: 256-bit
                bytes.extend_from_slice(&low.to_le_bytes());
                bytes.extend_from_slice(&high.to_le_bytes());
                serializer.serialize_bytes(&bytes)
            }
            Self::Code512(code) => {
                let mut bytes = vec![4u8]; // Type tag: 512-bit
                for part in &code.parts {
                    bytes.extend_from_slice(&part.to_le_bytes());
                }
                serializer.serialize_bytes(&bytes)
            }
        }
    }
}

impl<'de> Deserialize<'de> for SpatialCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;

        if bytes.is_empty() {
            return Err(serde::de::Error::custom("Empty spatial code"));
        }

        match bytes[0] {
            1 => {
                if bytes.len() != 9 {
                    return Err(serde::de::Error::custom("Invalid 64-bit code length"));
                }
                Ok(Self::Code64(u64::from_le_bytes(
                    bytes[1..9].try_into().unwrap(),
                )))
            }
            2 => {
                if bytes.len() != 17 {
                    return Err(serde::de::Error::custom("Invalid 128-bit code length"));
                }
                Ok(Self::Code128(u128::from_le_bytes(
                    bytes[1..17].try_into().unwrap(),
                )))
            }
            3 => {
                if bytes.len() != 33 {
                    return Err(serde::de::Error::custom("Invalid 256-bit code length"));
                }
                Ok(Self::Code256 {
                    low: u128::from_le_bytes(bytes[1..17].try_into().unwrap()),
                    high: u128::from_le_bytes(bytes[17..33].try_into().unwrap()),
                })
            }
            4 => {
                if bytes.len() != 65 {
                    return Err(serde::de::Error::custom("Invalid 512-bit code length"));
                }
                let mut parts = [0u128; 4];
                for (i, part) in parts.iter_mut().enumerate() {
                    let start = 1 + i * 16;
                    let end = start + 16;
                    *part = u128::from_le_bytes(bytes[start..end].try_into().unwrap());
                }
                Ok(Self::Code512(U512::new(parts)))
            }
            _ => Err(serde::de::Error::custom(format!(
                "Unknown spatial code type: {}",
                bytes[0]
            ))),
        }
    }
}

// ============================================================================
// CodeType: Spatial Code Type Selector
// ============================================================================

/// Spatial code type selector
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeType {
    Bits64,
    Bits128,
    Bits256,
    Bits512,
}

impl CodeType {
    /// Maximum bits for this code type
    pub fn max_bits(self) -> usize {
        match self {
            Self::Bits64 => 64,
            Self::Bits128 => 128,
            Self::Bits256 => 256,
            Self::Bits512 => 512,
        }
    }

    /// Maximum dimensions at 8 bits/dim
    pub fn max_dimensions(self) -> usize {
        self.max_bits() / 8
    }

    /// Select appropriate code type for given dimensions and bits_per_dim
    pub fn select(dimensions: usize, bits_per_dim: usize) -> Self {
        let total_bits = dimensions * bits_per_dim;

        if total_bits <= 64 {
            Self::Bits64
        } else if total_bits <= 128 {
            Self::Bits128
        } else if total_bits <= 256 {
            Self::Bits256
        } else if total_bits <= 512 {
            Self::Bits512
        } else {
            panic!("Total bits ({}) exceeds maximum (512)", total_bits);
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u512_arithmetic() {
        let a = U512::new([100, 0, 0, 0]);
        let b = U512::new([50, 0, 0, 0]);

        let sum = a.saturating_add(&b);
        assert_eq!(sum, U512::new([150, 0, 0, 0]));

        let diff = a.saturating_sub(&b);
        assert_eq!(diff, U512::new([50, 0, 0, 0]));

        // Test overflow saturation
        let max = U512::MAX;
        let result = max.saturating_add(&U512::new([1, 0, 0, 0]));
        assert_eq!(result, U512::MAX);

        // Test underflow saturation
        let zero = U512::ZERO;
        let result = zero.saturating_sub(&U512::new([1, 0, 0, 0]));
        assert_eq!(result, U512::ZERO);
    }

    #[test]
    fn test_u512_comparison() {
        let a = U512::new([100, 0, 0, 0]);
        let b = U512::new([50, 0, 0, 0]);
        let c = U512::new([100, 0, 0, 0]);

        assert!(a > b);
        assert!(b < a);
        assert_eq!(a, c);
        assert!(a.in_range(&b, &U512::new([200, 0, 0, 0])));
    }

    #[test]
    fn test_spatial_code_range() {
        let min = SpatialCode::Code64(100);
        let max = SpatialCode::Code64(200);
        let value = SpatialCode::Code64(150);

        assert!(value.in_range(&min, &max));
        assert!(!SpatialCode::Code64(50).in_range(&min, &max));
        assert!(!SpatialCode::Code64(250).in_range(&min, &max));
    }

    #[test]
    fn test_spatial_code_epsilon() {
        let a = SpatialCode::Code64(1000);
        let b = SpatialCode::Code64(2000);

        let epsilon = a.epsilon(&b, 10.0, 100);
        assert_eq!(epsilon, SpatialCode::Code64(100)); // 10% of 1000, min 100
    }

    #[test]
    fn test_code_type_selection() {
        assert_eq!(CodeType::select(8, 8), CodeType::Bits64);
        assert_eq!(CodeType::select(16, 8), CodeType::Bits128);
        assert_eq!(CodeType::select(32, 8), CodeType::Bits256);
        assert_eq!(CodeType::select(64, 8), CodeType::Bits512);
    }
}

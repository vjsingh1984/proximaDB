# ProximaCodec Lossy Schemes Verification

## Executive Summary

**Total Schemes**: 15  
**Always Lossless**: 10 schemes (67%)  
**Conditionally Lossy**: 4 schemes (27%)  
**Always Lossy**: 1 scheme (7%)

---

## Scheme-by-Scheme Analysis

### ✅ ALWAYS LOSSLESS (10 schemes)

These schemes guarantee perfect round-trip for all data types:

1. **Delta** - Stores differences from base value
   - Uses IEEE 754 bit pattern preservation (to_bits/from_bits)
   - Example: 3.14159265359f32 → perfectly recoverable

2. **DoubleDelta** - Delta of deltas
   - Uses IEEE 754 bit pattern preservation
   - Perfect for time-series floats

3. **PForDoubleDelta** - Patched double delta
   - Uses IEEE 754 bit pattern preservation
   - Handles outliers without loss

4. **Simple8b** - Variable-length integer packing
   - Stores full values in 64-bit words
   - Always recoverable

5. **VByte** - Variable-byte encoding (LEB128)
   - Stores full values with variable bytes
   - Always recoverable

6. **SparseBitmap** - Bitmap + non-zero values
   - Stores complete index and value
   - Perfect for 70-95% sparse data

7. **SparseCOO** - Coordinate format (index, value) pairs
   - Stores complete coordinates
   - Perfect for >95% sparse data

8. **Dictionary** - Value → integer code mapping
   - Maintains complete dictionary
   - Perfect for low-cardinality data

9. **RunLength** - (value, count) pairs
   - Stores exact values and counts
   - Perfect for constant/repeated data

10. **Adaptive** - Automatic scheme selection
    - Delegates to other lossless schemes
    - Maintains losslessness

---

### ⚠️ CONDITIONALLY LOSSY (4 schemes)

These schemes are lossy ONLY when bits < type_size:

#### 1. **BitPacked { bits }**

**When Lossy:**
```rust
// LOSSY: 8 bits for f32 (32 bits needed)
ProximaScheme::BitPacked { bits: 8 }

// Example
let original = 3.14159265359f32;
// bits: 01000000010010010000111111011011
// Truncate to 8 bits: 11011011
// Lost: 24 high-order bits
let decoded = /* corrupted value */;
assert_ne!(original, decoded); // ❌ NOT EQUAL
```

**Lossless Solution:**
```rust
// LOSSLESS: 32 bits for f32
ProximaScheme::BitPacked { bits: 32 }

let original = 3.14159265359f32;
let bits = original.to_bits(); // All 32 bits
let decoded = f32::from_bits(bits); // Perfect recovery
assert_eq!(original, decoded); // ✅ EXACT MATCH
```

**Guidelines:**
- f32: Use `bits: 32`
- f64: Use `bits: 64`
- i32/u32: Use `bits: 32`
- i64/u64: Use `bits: 64`

---

#### 2. **Zigzag { bits }**

**When Lossy:**
```rust
// LOSSY: 16 bits for i32 (32 bits needed)
ProximaScheme::Zigzag { bits: 16 }

// Example: Large signed integer
let original: i32 = 100_000;
// Zigzag encode: 200000 (unsigned)
// Truncate to 16 bits: 3392
// Decode: 1696 (WRONG!)
assert_ne!(original, 1696); // ❌ LOST PRECISION
```

**Lossless Solution:**
```rust
// LOSSLESS: 32 bits for i32
ProximaScheme::Zigzag { bits: 32 }

let original: i32 = 100_000;
// Full 32-bit zigzag encoding
let decoded = /* perfect recovery */;
assert_eq!(original, decoded); // ✅ EXACT MATCH
```

**Important:**
- ⚠️ Zigzag is ALWAYS lossy for floats (TypeId::F32, TypeId::F64)
- Reason: Zigzag designed for SIGNED INTEGERS, not IEEE 754 floats
- **Don't use Zigzag for floats!** Use Delta, DoubleDelta, or BitPacked instead

**Guidelines:**
- i32: Use `bits: 32`
- i64: Use `bits: 64`
- Floats: Use different scheme (Delta, DoubleDelta, BitPacked)

---

#### 3. **FrameOfReference { reference, bits }**

**When Lossy:**
```rust
// LOSSY: 16 bits for f32 (32 bits needed)
ProximaScheme::FrameOfReference { reference: 0, bits: 16 }

// Example
let original = 42.75f32;
let bits_i32 = original.to_bits() as i32;
let offset = bits_i32 - (reference as i32);
// Truncate offset to 16 bits
let truncated = offset & 0xFFFF;
// Decode: corrupted
```

**Lossless Solution:**
```rust
// LOSSLESS: 32 bits for f32
ProximaScheme::FrameOfReference { reference: 0, bits: 32 }

let original = 42.75f32;
let bits_i32 = original.to_bits() as i32;
let offset = bits_i32 - (reference as i32);
// No truncation - full 32 bits
let decoded_bits = (reference as i32) + offset;
let decoded = f32::from_bits(decoded_bits as u32);
assert_eq!(original, decoded); // ✅ EXACT MATCH
```

**Guidelines:**
- f32: Use `bits: 32`
- f64: Use `bits: 64`
- i32/u32: Use `bits: 32`
- i64/u64: Use `bits: 64`

---

#### 4. **PForDelta { majority_bits, base }**

**When Lossy:**
```rust
// LOSSY: 16 majority_bits for f32 (32 bits needed)
ProximaScheme::PForDelta { majority_bits: 16, base: 0 }

// Most values encoded with 16 bits (lossy)
// Exceptions stored separately (lossless but rare)
// Net result: LOSSY for typical values
```

**Lossless Solution:**
```rust
// LOSSLESS: 32 majority_bits for f32
ProximaScheme::PForDelta { majority_bits: 32, base: 0 }

// All values (majority + exceptions) encoded losslessly
// Perfect round-trip guaranteed
```

**Guidelines:**
- f32: Use `majority_bits: 32`
- f64: Use `majority_bits: 64`
- i32/u32: Use `majority_bits: 32`
- i64/u64: Use `majority_bits: 64`

---

### ❌ ALWAYS LOSSY (1 scheme)

#### **Gorilla**

**Why Lossy:**
- Uses XOR-based compression between consecutive float values
- Compresses leading zeros, trailing zeros, and intermediate bits
- Block size limitations prevent perfect recovery
- **Designed for acceptable precision loss in time-series data**

**Example:**
```rust
ProximaScheme::Gorilla

// Time-series floats
let values = vec![1.0f32, 1.00001, 1.00002, 1.00003];

// Gorilla XOR encoding:
// 1. XOR consecutive values
// 2. Compress leading/trailing zeros
// 3. Store control bits + changed bits
// 
// Result: Small compression of changed bits may lose precision
// For time-series: Acceptable (0.01-0.1% error)
// For exact computation: NOT ACCEPTABLE
```

**Lossless Alternatives:**
```rust
// For float time-series, use instead:

// Option 1: Delta encoding (lossless)
ProximaScheme::Delta { base: 0 }

// Option 2: DoubleDelta (lossless, better for smooth data)
ProximaScheme::DoubleDelta { first_value: 0, first_delta: 1 }

// Option 3: BitPacked (lossless with bits=32)
ProximaScheme::BitPacked { bits: 32 }

// Option 4: FrameOfReference (lossless with bits=32)
ProximaScheme::FrameOfReference { reference: 0, bits: 32 }
```

**When to Use Gorilla:**
- Time-series monitoring (acceptable ~0.1% error)
- High-frequency sensor data (compression > precision)
- Real-time streaming (speed > exactness)

**When NOT to Use Gorilla:**
- Financial calculations (need exact values)
- Scientific simulations (need precision)
- ML embeddings (need exact vectors)
- Any exact-match requirements

---

## Making Schemes Lossless - Quick Reference

### For f32 (32-bit floats):
```rust
// ✅ LOSSLESS configurations
ProximaScheme::BitPacked { bits: 32 }
ProximaScheme::FrameOfReference { reference: 0, bits: 32 }
ProximaScheme::PForDelta { majority_bits: 32, base: 0 }
ProximaScheme::Delta { base: 0 }  // Always lossless
ProximaScheme::DoubleDelta { first_value: 0, first_delta: 1 }  // Always lossless

// ⚠️ DON'T USE
ProximaScheme::Zigzag { .. }  // Not for floats!
ProximaScheme::Gorilla  // Always lossy
```

### For i32 (32-bit integers):
```rust
// ✅ LOSSLESS configurations
ProximaScheme::BitPacked { bits: 32 }
ProximaScheme::Zigzag { bits: 32 }
ProximaScheme::FrameOfReference { reference: 0, bits: 32 }
ProximaScheme::PForDelta { majority_bits: 32, base: 0 }
ProximaScheme::Delta { base: 0 }  // Always lossless
ProximaScheme::Simple8b  // Always lossless
ProximaScheme::VByte  // Always lossless
```

### For f64/i64 (64-bit types):
```rust
// ✅ LOSSLESS configurations
ProximaScheme::BitPacked { bits: 64 }
ProximaScheme::Zigzag { bits: 64 }  // i64 only, not f64!
ProximaScheme::FrameOfReference { reference: 0, bits: 64 }
ProximaScheme::PForDelta { majority_bits: 64, base: 0 }
// All "always lossless" schemes work for 64-bit types
```

---

## Implementation Verification

### Testing Losslessness

```rust
fn verify_lossless<T: Encodable + Decodable + PartialEq + Copy>(
    values: &[T],
    scheme: ProximaScheme,
) {
    // Encode
    let encoded = encode(values, &scheme).expect("encode failed");
    
    // Decode
    let decoded = decode(&encoded, values.len()).expect("decode failed");
    
    // Verify exact match
    assert_eq!(values, &decoded[..], "Lossless encoding failed!");
}

// Example usage
#[test]
fn test_verify_lossless() {
    let values = vec![3.14159265359f32, 2.71828182846, 1.41421356237];
    
    // ✅ These should pass
    verify_lossless(&values, ProximaScheme::Delta { base: 0 });
    verify_lossless(&values, ProximaScheme::DoubleDelta { first_value: 0, first_delta: 1 });
    verify_lossless(&values, ProximaScheme::BitPacked { bits: 32 });
    
    // ❌ These should fail (lossy)
    // verify_lossless(&values, ProximaScheme::BitPacked { bits: 8 });  // FAILS
    // verify_lossless(&values, ProximaScheme::Gorilla);  // FAILS
}
```

---

## Summary Tables

### Losslessness by Scheme

| Scheme | f32 Lossless | f64 Lossless | i32 Lossless | i64 Lossless | Notes |
|--------|--------------|--------------|--------------|--------------|-------|
| Delta | ✅ Always | ✅ Always | ✅ Always | ✅ Always | IEEE 754 bit preservation |
| BitPacked | ✅ bits≥32 | ✅ bits≥64 | ✅ bits≥32 | ✅ bits≥64 | Conditional |
| FrameOfRef | ✅ bits≥32 | ✅ bits≥64 | ✅ bits≥32 | ✅ bits≥64 | Conditional |
| PForDelta | ✅ bits≥32 | ✅ bits≥64 | ✅ bits≥32 | ✅ bits≥64 | Conditional |
| Zigzag | ❌ Never | ❌ Never | ✅ bits≥32 | ✅ bits≥64 | Not for floats! |
| DoubleDelta | ✅ Always | ✅ Always | ✅ Always | ✅ Always | IEEE 754 bit preservation |
| PForDoubleDelta | ✅ Always | ✅ Always | ✅ Always | ✅ Always | IEEE 754 bit preservation |
| Simple8b | ✅ Always | ✅ Always | ✅ Always | ✅ Always | - |
| VByte | ✅ Always | ✅ Always | ✅ Always | ✅ Always | - |
| Gorilla | ❌ Never | ❌ Never | ✅ Always | ✅ Always | XOR compression lossy for floats |
| SparseBitmap | ✅ Always | ✅ Always | ✅ Always | ✅ Always | - |
| SparseCOO | ✅ Always | ✅ Always | ✅ Always | ✅ Always | - |
| Dictionary | ✅ Always | ✅ Always | ✅ Always | ✅ Always | - |
| RunLength | ✅ Always | ✅ Always | ✅ Always | ✅ Always | - |
| Adaptive | ✅ Always | ✅ Always | ✅ Always | ✅ Always | Delegates to lossless schemes |

### Recommended Configurations

| Data Type | Best Lossless Schemes | Configuration |
|-----------|----------------------|---------------|
| f32 embeddings | Delta, DoubleDelta | `Delta { base: 0 }` |
| f32 normalized | BitPacked | `BitPacked { bits: 32 }` |
| f32 time-series | DoubleDelta | `DoubleDelta { first_value: 0, first_delta: 1 }` |
| i32 IDs | Simple8b, VByte | `Simple8b` or `VByte` |
| i32 sequential | DoubleDelta, Delta | `DoubleDelta { .. }` |
| i32 signed small | Zigzag | `Zigzag { bits: 32 }` |
| f32/i32 sparse | SparseCOO, SparseBitmap | `SparseCOO` if >95% zeros |
| f32/i32 constant | RunLength | `RunLength` |
| f32/i32 low-cardinality | Dictionary | `Dictionary` |

---

## Conclusion

**Key Findings:**
1. ✅ **10/15 schemes (67%)** are ALWAYS lossless
2. ⚠️ **4/15 schemes (27%)** are CONDITIONALLY lossless (use bits ≥ type_size)
3. ❌ **1/15 schemes (7%)** is ALWAYS lossy (Gorilla for floats)

**Recommendations:**
1. **For exact computation**: Use always-lossless schemes or set bits = type_size
2. **For f32 embeddings**: Prefer Delta or DoubleDelta (always lossless)
3. **Avoid Gorilla for floats** unless acceptable precision loss (~0.1%)
4. **Never use Zigzag for floats** - it's designed for signed integers only
5. **Test losslessness**: Always verify round-trip equality in tests

**Making Existing Code Lossless:**
```rust
// ❌ BEFORE (lossy)
ProximaScheme::BitPacked { bits: 8 }

// ✅ AFTER (lossless)
ProximaScheme::BitPacked { bits: 32 }  // For f32

// ❌ BEFORE (lossy)
ProximaScheme::Gorilla

// ✅ AFTER (lossless alternative)
ProximaScheme::DoubleDelta { first_value: 0, first_delta: 1 }
```

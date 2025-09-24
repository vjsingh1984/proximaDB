# FastLanes Block Encoding Visual Guide

## Overview
FastLanes provides three vector encoding strategies, each optimized for different access patterns and data characteristics.

## 1. Block-Level Structure

```
┌─────────────────────────────────── FASTLANES BLOCK ──────────────────────────────────┐
│                                                                                       │
│  ┌──────────────────────────── HEADER (10 bytes) ─────────────────────────────┐      │
│  │ [0x01] Format Version (1 byte)                                             │      │
│  │ [0xXX] Encoding Marker (1 byte) - Determines encoding scheme               │      │
│  │ [u32]  Record Count (4 bytes) - Number of vectors                          │      │
│  │ [u32]  Dimension (4 bytes) - Vector dimension                              │      │
│  └─────────────────────────────────────────────────────────────────────────────┘      │
│                                                                                       │
│  ┌──────────────────────── VECTOR DATA (Variable) ────────────────────────────┐      │
│  │ Strategy-specific encoding (see below)                                     │      │
│  └─────────────────────────────────────────────────────────────────────────────┘      │
│                                                                                       │
│  ┌────────────────────── ID DICTIONARY (Variable) ────────────────────────────┐      │
│  │ [u32] Dictionary Size                                                      │      │
│  │ For each unique ID:                                                        │      │
│  │   [u32] ID Length                                                          │      │
│  │   [bytes] ID String                                                        │      │
│  │ [u32] Encoded Indices Length                                               │      │
│  │ [bytes] Delta-encoded ID indices                                           │      │
│  └─────────────────────────────────────────────────────────────────────────────┘      │
│                                                                                       │
│  ┌───────────────── SPARSE METADATA COLUMNS (Variable) ───────────────────────┐      │
│  │ [u32] Number of Metadata Keys                                              │      │
│  │ For each key:                                                              │      │
│  │   [u32] Key Name Length                                                    │      │
│  │   [bytes] Key Name                                                         │      │
│  │   [u32] Presence Bitmap Length                                             │      │
│  │   [bytes] Bitmap (1 bit per record)                                        │      │
│  │   [u32] Compressed Values Length                                           │      │
│  │   [bytes] Compressed Values                                                │      │
│  └─────────────────────────────────────────────────────────────────────────────┘      │
│                                                                                       │
│  ┌─────────────────────── TIMESTAMPS (Variable) ──────────────────────────────┐      │
│  │ [u32] Timestamp Data Length                                                │      │
│  │ [bytes] Delta-encoded timestamps                                           │      │
│  └─────────────────────────────────────────────────────────────────────────────┘      │
│                                                                                       │
│  ┌────────────────── BLOCK METADATA (Variable) ───────────────────────────────┐      │
│  │ [u32] Metadata Length                                                      │      │
│  │ [bytes] Bincode-serialized FastLanesBlockMetadata                          │      │
│  └─────────────────────────────────────────────────────────────────────────────┘      │
│                                                                                       │
└───────────────────────────────────────────────────────────────────────────────────────┘

COMPRESSION WRAPPER (Optional):
┌────────────────────────────────────────────────────────────────────────────────┐
│ If compressed:                                                                │
│   [0x80-0x83] Compression Marker (1 byte)                                    │
│     0x80 = LZ4, 0x81 = Zstd, 0x82 = Snappy, 0x83 = Gzip                    │
│   [u32] Original Size (4 bytes)                                              │
│   [bytes] Compressed Data (entire block above)                               │
│                                                                              │
│ If not compressed:                                                           │
│   [0x00] No Compression Marker (1 byte)                                      │
│   [bytes] Uncompressed Data (entire block above)                             │
└────────────────────────────────────────────────────────────────────────────────┘
```

## 2. Vector Encoding Strategies

### 2.1 FullVector Strategy (Row-wise Storage with Field-Level Compression)

```
FullVector Layout (Best for sequential access of complete vectors):

Original Data (3 vectors × 4D):
  V1: [1.1, 1.2, 1.3, 1.4]
  V2: [2.1, 2.2, 2.3, 2.4]
  V3: [3.1, 3.2, 3.3, 3.4]

NEW Field-Level Compression Layout (Version 0x01):
┌─────────────────────────────────────────────────────────────────────┐
│ HEADER:                                                             │
│   [0x46] 'F' marker                                                │
│   [0x56] 'V' marker  → "FV" = FullVector                          │
│   [0x01] Version (field-level compression)                        │
│   [u32]  Dimension = 4                                            │
│   [u32]  Count = 3                                                │
├─────────────────────────────────────────────────────────────────────┤
│ VECTOR FIELD (FastLanes + Compression):                           │
│   [0x11] Compression Marker (0x11 = Zstd, 0x10 = LZ4, 0x00 = None)│
│   [u32]  Original Size (bytes)                                    │
│   [u32]  Compressed Size (bytes)                                  │
│   [bytes] Compressed Data:                                        │
│     - FastLanes Delta encoded:                                    │
│       * First vector raw: [1.1, 1.2, 1.3, 1.4]                  │
│       * Deltas: Δ[1.0, 1.0, 1.0, 1.0], Δ[1.0, 1.0, 1.0, 1.0]   │
│     - Then compressed with specified algorithm                     │
└─────────────────────────────────────────────────────────────────────┘

Advantages:
- Excellent compression ratios (1.43x with Zstd, 1.25x with LZ4)
- Sequential access optimized for complete vectors
- FastLanes delta encoding eliminates redundancy
- Field-level compression allows per-field optimization

Memory Layout:
[FV][01][dim][cnt][comp_marker][orig_size][comp_size][compressed_data...]
```

### 2.2 TransposeVector Strategy (Columnar Storage with Per-Dimension Field Compression)

```
TransposeVector Layout (Best for dimension-wise operations):

Original Data (3 vectors × 4D):
  V1: [1.1, 1.2, 1.3, 1.4]
  V2: [2.1, 2.2, 2.3, 2.4]
  V3: [3.1, 3.2, 3.3, 3.4]

Transposition (RxD → DxR):
  D0: [1.1, 2.1, 3.1]  ← All first dimensions
  D1: [1.2, 2.2, 3.2]  ← All second dimensions
  D2: [1.3, 2.3, 3.3]  ← All third dimensions
  D3: [1.4, 2.4, 3.4]  ← All fourth dimensions

NEW Per-Dimension Field Compression Layout (Version 0x01):
┌─────────────────────────────────────────────────────────────────────┐
│ HEADER:                                                             │
│   [0x54] 'T' marker                                                │
│   [0x56] 'V' marker  → "TV" = TransposeVector                     │
│   [0x01] Version (per-dimension field compression)                │
│   [u32]  Dimension = 4                                            │
│   [u32]  Count = 3                                                │
├─────────────────────────────────────────────────────────────────────┤
│ DIMENSION FIELD 0 (FastLanes + Compression):                      │
│   [0x11] Compression Marker (0x11 = Zstd, 0x10 = LZ4, 0x00 = None)│
│   [u32]  Original Size (bytes)                                    │
│   [u32]  Compressed Size (bytes)                                  │
│   [bytes] Compressed FastLanes(D0: [1.1, 2.1, 3.1])             │
├─────────────────────────────────────────────────────────────────────┤
│ DIMENSION FIELD 1:                                                │
│   [0x11] Compression Marker                                       │
│   [u32]  Original Size                                            │
│   [u32]  Compressed Size                                          │
│   [bytes] Compressed FastLanes(D1: [1.2, 2.2, 3.2])             │
├─────────────────────────────────────────────────────────────────────┤
│ DIMENSION FIELD 2:                                                │
│   [0x11] Compression Marker                                       │
│   [u32]  Original Size                                            │
│   [u32]  Compressed Size                                          │
│   [bytes] Compressed FastLanes(D2: [1.3, 2.3, 3.3])             │
├─────────────────────────────────────────────────────────────────────┤
│ DIMENSION FIELD 3:                                                │
│   [0x11] Compression Marker                                       │
│   [u32]  Original Size                                            │
│   [u32]  Compressed Size                                          │
│   [bytes] Compressed FastLanes(D3: [1.4, 2.4, 3.4])             │
└─────────────────────────────────────────────────────────────────────┘

Advantages:
- Each dimension compressed independently for optimal efficiency
- Perfect for analytical workloads accessing specific dimensions
- Good compression ratios (1.33x with Zstd, 1.12x with LZ4)
- Supports partial dimension loading

Memory Layout:
[TV][01][dim][cnt][D0_marker][D0_sizes][D0_data][D1_marker][D1_sizes][D1_data]...

Benefits:
• Each dimension compressed independently
• SIMD-friendly columnar access
• Better compression for similar values in same dimension
```

### 2.3 GroupedVector Strategy (Cache-Optimized 64D Groups with Header-Based Compression)

```
GroupedVector Layout (Best for high-dimensional vectors):

Original Data (2 vectors × 256D):
  V1: [v1_0, v1_1, ..., v1_255]
  V2: [v2_0, v2_1, ..., v2_255]

Grouping (64D groups for cache locality):
  Group 0: dims [0-63]   → 256 bytes (fits L1/L2 cache line)
  Group 1: dims [64-127]
  Group 2: dims [128-191]
  Group 3: dims [192-255]

OPTIMIZED Header-Based Compression Layout (Version 0x01):
┌─────────────────────────────────────────────────────────────────────┐
│ HEADER:                                                             │
│   [0x47] 'G' marker                                                │
│   [0x56] 'V' marker  → "GV" = GroupedVector                       │
│   [0x01] Version (optimized layout)                               │
│   [u32]  Dimension = 256                                          │
│   [u32]  Count = 2                                                │
│   [u32]  Num Groups = 4                                           │
│   [0x11] Compression Algorithm (shared by all groups)              │
│        └─ 0x00=None, 0x10=LZ4, 0x11=Zstd, 0x12=Snappy           │
├─────────────────────────────────────────────────────────────────────┤
│ GROUP 0 (dims 0-63):                                              │
│   [u32] Start Dim = 0                                             │
│   [u32] Group Dims = 64                                           │
│   [u32] Data Size (final size after compression)                   │
│   [bytes] Data (FastLanes encoded, then compressed per header)     │
│           └─ FastLanes([v1_0..v1_63, v2_0..v2_63])               │
│                        └─ Row-wise within group ─┘                │
├─────────────────────────────────────────────────────────────────────┤
│ GROUP 1 (dims 64-127):                                            │
│   [u32] Start Dim = 64                                            │
│   [u32] Group Dims = 64                                           │
│   [u32] Data Size                                                 │
│   [bytes] Data (compressed using header algorithm)                │
├─────────────────────────────────────────────────────────────────────┤
│ GROUP 2 & 3: Similar structure...                                 │
└─────────────────────────────────────────────────────────────────────┘

Advantages:
- Best compression ratios (1.51x with Zstd, 1.34x with LZ4)
- 22% reduction in metadata overhead (87→68 bytes for 4 groups)
- Single compression decision for all groups (better cache locality)
- Simplified decode path (fewer branches)
- Row-wise access within groups preserves vector coherency

Memory Layout:
[GV][01][dim][cnt][groups][comp_alg][G0_start][G0_dims][G0_size][G0_data]...

Memory Access Pattern:
• Each 64D group fits in cache (256 bytes)
• Row-wise within groups for vector coherency
• Header-based compression for uniform performance
```

## 3. FastLanes Encoding Schemes

```
┌──────────────────── FASTLANES ENCODING MARKERS ─────────────────────┐
│                                                                      │
│ 0x00: Raw (no encoding)                                            │
│ 0x01: BitPacked(8)  - Pack into 8 bits                           │
│ 0x02: BitPacked(16) - Pack into 16 bits                          │
│ 0x03: BitPacked(32) - Pack into 32 bits                          │
│ 0x04: Delta(base=0) - Delta encoding from base                   │
│ 0x05: FrameOfReference - Values relative to reference frame       │
│ 0x06: PatchedBase - Base value with patches for outliers         │
│ 0x07: XOR - XOR encoding for floating point                      │
│                                                                      │
│ Data Type Markers (in encoded data):                               │
│ 0x80: f32 data                                                     │
│ 0x81: f64 data                                                     │
│ 0x82: i64 data                                                     │
│ 0x83: INT8 quantized                                               │
│ 0x84: u16 data                                                     │
│ 0x85: u32 data                                                     │
│ 0x86: PQ4 codes                                                    │
│ 0x87: PQ8 codes                                                    │
└──────────────────────────────────────────────────────────────────────┘
```

## 4. Field-Level Compression Architecture (NEW)

**Key Innovation: Independent Field Compression**

All three vector strategies now implement field-level compression where each logical field is compressed independently:

```
┌──────────────── FIELD-LEVEL COMPRESSION BENEFITS ────────────────────┐
│                                                                       │
│ FullVector Strategy:                                                  │
│ • Single vector field with delta encoding + compression               │
│ • 1.43x compression ratio with Zstd                                  │
│ • Optimal for sequential vector access                                │
│                                                                       │
│ TransposeVector Strategy:                                             │
│ • Each dimension = separate compressed field                          │
│ • Perfect for analytical workloads                                   │
│ • 1.33x compression ratio with Zstd                                  │
│ • Enables partial dimension loading                                   │
│                                                                       │
│ GroupedVector Strategy:                                               │
│ • Header-based compression for all 64D groups                        │
│ • 22% metadata overhead reduction (87→68 bytes)                      │
│ • 1.51x compression ratio with Zstd (best overall)                   │
│ • Simplified decode path                                             │
│                                                                       │
│ Configurable Metadata Compression:                                   │
│ • metadata_algorithm field in BlockCompressionConfig                 │
│ • Smart fallback: uses main algorithm if not specified               │
│ • No more hardcoded Zstd for metadata                               │
│                                                                       │
└───────────────────────────────────────────────────────────────────────┘
```

**Architecture Principles:**
- **Field Independence**: Each field (vectors, IDs, metadata, timestamps) compressed separately
- **Algorithm Flexibility**: Different fields can use different compression algorithms
- **Optimal Granularity**: Compression applied at the most effective level (vector, dimension, or group)
- **Performance First**: Layouts optimized for both compression ratio and decode speed

**Compression Marker Consistency:**
- `0x00`: No compression
- `0x10`: LZ4 compression
- `0x11`: Zstd compression
- `0x12`: Snappy compression
- `0x13`: Gzip compression

## 5. Compression Markers & Application

```
┌────────────────── COMPRESSION FLOW ──────────────────────┐
│                                                          │
│  Input Data                                              │
│      ↓                                                   │
│  Strategy Selection (Auto/Manual)                        │
│      ├─ FullVector (D ≤ 128)                           │
│      ├─ TransposeVector (columnar operations)          │
│      └─ GroupedVector (D > 128)                        │
│      ↓                                                   │
│  FastLanes Encoding                                      │
│      ├─ Delta (sequential data)                        │
│      ├─ BitPacked (bounded range)                      │
│      └─ FrameOfReference (clustered values)           │
│      ↓                                                   │
│  Compression Decision                                    │
│      ├─ Check if size reduction > threshold            │
│      └─ Apply algorithm if beneficial                  │
│      ↓                                                   │
│  Final Output                                           │
│      ├─ [0x80-0x83] + size + compressed               │
│      └─ [0x00] + uncompressed                         │
└──────────────────────────────────────────────────────────┘

Compression Algorithms:
┌─────────────────────────────────────────────────────────┐
│ Marker │ Algorithm │ Use Case                          │
├────────┼───────────┼────────────────────────────────────┤
│ 0x80   │ LZ4       │ Fast, general purpose             │
│ 0x81   │ Zstd      │ Better ratio, slower              │
│ 0x82   │ Snappy    │ Very fast, moderate ratio         │
│ 0x83   │ Gzip      │ High ratio, slow                  │
│ 0x00   │ None      │ Already compressed/small data     │
└─────────────────────────────────────────────────────────┘
```

## 5. Decoding Flow

```
┌──────────────────── DECODING PIPELINE ────────────────────┐
│                                                           │
│  Read First Byte                                          │
│      ├─ 0x80-0x8F: Compressed block                      │
│      │   └─ Decompress → Continue                        │
│      ├─ 0x00: Uncompressed marker                        │
│      │   └─ Skip marker → Continue                       │
│      └─ 0x01: Format version → Continue                  │
│      ↓                                                    │
│  Read Header                                              │
│      ├─ Format version                                   │
│      ├─ Encoding marker                                  │
│      ├─ Record count                                     │
│      └─ Dimension                                        │
│      ↓                                                    │
│  Detect Vector Strategy                                   │
│      ├─ [0x46,0x56]: FullVector                         │
│      ├─ [0x47,0x56]: GroupedVector                      │
│      └─ Default: TransposeVector                         │
│      ↓                                                    │
│  Decode Vector Data                                       │
│      └─ Strategy-specific decoder                        │
│      ↓                                                    │
│  Decode IDs, Metadata, Timestamps                         │
│      └─ Dictionary decoding, sparse columns              │
│      ↓                                                    │
│  Reconstruct VectorRecords                                │
└───────────────────────────────────────────────────────────┘
```

## 6. Example: 1000 vectors × 384D with GroupedVector + LZ4

```
Raw Size: 1000 × 384 × 4 bytes = 1,536,000 bytes

After Encoding & Compression:
┌─────────────────────────────────────────────────────────────┐
│ Block Structure:                           Size (bytes)     │
├─────────────────────────────────────────────────────────────┤
│ Compression Marker (LZ4)                   1               │
│ Original Size                              4               │
│ Compressed Block:                          ~400,000        │
│   ├─ Header                               10              │
│   ├─ GroupedVector Data:                  ~380,000        │
│   │   ├─ 6 groups × 64D each                             │
│   │   ├─ Each group: ~63,000 bytes                       │
│   │   └─ Per-group LZ4 compression                       │
│   ├─ ID Dictionary                        ~5,000          │
│   ├─ Metadata Columns                     ~10,000         │
│   ├─ Timestamps                           ~4,000          │
│   └─ Block Metadata                       ~1,000          │
├─────────────────────────────────────────────────────────────┤
│ Total Size                                ~400,005         │
│ Compression Ratio                         3.84x            │
└─────────────────────────────────────────────────────────────┘

Benefits of GroupedVector for this case:
• 6 cache-friendly groups (64D each)
• Selective decompression possible
• Better SIMD utilization
• Maintains row-wise coherency within groups
```

## 7. Selection Guidelines

```
┌────────────────── STRATEGY SELECTION MATRIX ──────────────────┐
│                                                               │
│ Dimension │ Access Pattern      │ Recommended Strategy       │
├───────────┼────────────────────┼────────────────────────────┤
│ D ≤ 64    │ Any                │ FullVector                 │
│ D ≤ 128   │ Sequential         │ FullVector                 │
│ D ≤ 128   │ Dimension-wise     │ TransposeVector           │
│ D > 128   │ Sequential         │ GroupedVector             │
│ D > 128   │ Mixed              │ GroupedVector             │
│ D > 512   │ Dimension-wise     │ TransposeVector           │
│ D > 1024  │ Any                │ GroupedVector (mandatory)  │
└───────────────────────────────────────────────────────────────┘

Compression Algorithm Selection:
┌────────────────────────────────────────────────────────────────┐
│ Workload Type │ Priority        │ Recommended                │
├───────────────┼─────────────────┼────────────────────────────┤
│ Real-time     │ Latency         │ Snappy or None             │
│ Batch         │ Throughput      │ LZ4                        │
│ Archive       │ Compression     │ Zstd or Gzip               │
│ Mixed         │ Balance         │ LZ4 or Snappy              │
└────────────────────────────────────────────────────────────────┘
```
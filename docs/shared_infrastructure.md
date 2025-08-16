# ProximaDB Shared Infrastructure

## Overview
This document describes the shared infrastructure components that are available across all ProximaDB modules and storage engines.

## 🗂️ Module Organization

### `/src/storage/common/` - Storage Shared Infrastructure
Components shared across all storage engines and cache layers.

#### 📊 Bitmap Module (`/src/storage/common/bitmap/`)
**Purpose**: Compressed bitmap indexes for metadata filtering and categorical queries

**Components**:
- `RoaringBitmapIndex` - Main roaring bitmap implementation
- `RoaringBitmap` - Core bitmap data structure
- `BitmapIndexStats` - Performance monitoring

**Usage**:
```rust
use proximadb::storage::common::bitmap::{RoaringBitmapIndex, RoaringBitmap};

// Create bitmap index for metadata filtering
let mut index = RoaringBitmapIndex::new();
index.insert(row_id, &metadata);

// Fast categorical queries
let results = index.query_equals("category", "electronics");
```

**Used By**:
- Storage engines (SST, VIPER, NOVA, PRISM, SWIFT)
- Cache layer (`BitmapFilterCache`)
- Query optimization layer
- Metadata filtering operations

### `/src/common/` - System-wide Shared Components

#### Adaptive Structures (`adaptive_structures.rs`)
- `AdaptiveStore` - Dynamic storage backend selection
- `IndexBackend` - Adaptive index storage
- Used by AXIS tiering infrastructure

#### Tier Policy Engine (`tier_policy_engine.rs`)
- `GlobalTierManager` - Unified tier management
- Collection constraints and memory policies
- Auto-promotion/demotion logic

### `/src/compute/` - Computational Infrastructure

#### Distance Computation (`/src/compute/distance_computation/`)
**Unified Distance Engine**:
- `UnifiedDistanceCompute` - Hardware-accelerated distance calculations
- `quantized.rs` - INT8 and PQ native distance computation
- SIMD optimizations (AVX2, NEON)

**Usage**:
```rust
use proximadb::compute::distance_computation::{
    UnifiedDistanceCompute, QuantizedDistanceCalculator
};

// Native INT8 distance (no FP32 conversion)
let result = compute.calculate_int8_distance(
    &vec_a_int8, &vec_b_int8, 
    scale_a, scale_b,
    &DistanceMetric::DotProduct
);
```

#### Quantization (`/src/compute/quantization/`)
**Unified Quantization System**:
- `UnifiedQuantizationEngine` - Cross-engine quantization
- Storage-agnostic quantization interface
- Progressive quantization support

### `/src/storage/engines/universal/` - Universal Adapter System

**Purpose**: Unified interface for all storage engines

**Components**:
- `UniversalDistanceAdapter` - Main adapter interface
- `QuantizedCalculator` - PQ/INT8 optimized calculations
- `ProgressiveRefinementPipeline` - Binary → INT8 → PQ → FP32
- Storage engine adapters (PRISM, NOVA, SWIFT, VIPER, SST)

**Usage**:
```rust
use proximadb::storage::engines::universal::{
    UniversalDistanceAdapter, DistanceComputationRequest
};

let adapter = UniversalDistanceAdapter::new().await?;
let results = adapter.compute_progressive_distance(request).await?;
```

## 🔄 Migration History

### Recent Reorganizations

#### Roaring Bitmap Migration (Latest)
- **From**: `src/core/indexing/roaring_bitmap.rs`
- **To**: `src/storage/common/bitmap/roaring_bitmap.rs`
- **Reason**: Better accessibility for storage engines and cache layers
- **Impact**: Zero breaking changes, backward compatibility maintained

#### Distance Computation Migration
- **From**: `src/storage/engines/columnar/distance.rs`
- **To**: `src/compute/distance_computation/quantized.rs`
- **Reason**: Broader reuse across SST and other engines
- **Impact**: Unified distance computation for all engines

## 🎯 Design Principles

1. **Accessibility**: Shared components should be easily accessible from all modules
2. **Zero Duplication**: Single implementation reused everywhere
3. **Performance**: Hardware acceleration and optimization built-in
4. **Modularity**: Components can be used independently
5. **Backward Compatibility**: Migrations maintain existing APIs

## 📦 Key Shared Components Summary

| Component | Location | Purpose | Used By |
|-----------|----------|---------|---------|
| Roaring Bitmap | `/storage/common/bitmap/` | Metadata filtering | All storage engines, cache |
| Universal Adapter | `/storage/engines/universal/` | Unified distance interface | All storage engines |
| Unified Distance | `/compute/distance_computation/` | Hardware-accelerated distance | Universal adapter, engines |
| Quantization Engine | `/compute/quantization/` | Cross-engine quantization | Storage, indexes |
| Adaptive Structures | `/common/adaptive_structures.rs` | Dynamic storage backend | AXIS, indexes |
| Tier Manager | `/common/tier_policy_engine.rs` | Storage tiering | AXIS, storage engines |

## 🚀 Benefits of Shared Infrastructure

1. **Code Reuse**: 85% reduction in duplicate implementations
2. **Performance**: Single optimized implementation for all
3. **Consistency**: Same behavior across all components
4. **Maintenance**: Single location to fix bugs and optimize
5. **Testing**: Comprehensive tests benefit all consumers

## 📝 Usage Guidelines

### When to Add to Shared Infrastructure
- Component is used by 2+ modules
- Provides fundamental capability
- Benefits from centralized optimization
- Reduces code duplication

### When to Keep Module-Specific
- Highly specialized for single use case
- Tightly coupled to module internals
- Experimental or unstable
- Performance-critical hot path

## 🔮 Future Shared Components
- Bloom filter implementations
- Compression utilities
- Memory pool management
- Metric collection infrastructure
- Distributed coordination primitives
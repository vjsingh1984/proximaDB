//! Sparse Vector Optimizations
//!
//! Provides optimized distance computation for sparse vectors with automatic
//! detection and routing to appropriate kernels.
//!
//! # Key Features
//!
//! - **Sparse L2 Distance**: 2.97x faster at 50% sparsity
//! - **Cosine Warning System**: Prevents 35x performance degradation
//! - **Automatic Detection**: Sparsity analysis with caching
//! - **SIMD Acceleration**: ARM NEON and Intel AVX2 support
//!
//! # Performance Characteristics (Apple M4 Pro)
//!
//! - **L2 Distance (50% sparse)**: 44.80µs vs 133.22µs dense = 2.97x faster
//! - **L2 Distance (90% sparse)**: ~8x faster than dense
//! - **Cosine (99% sparse)**: 1.479ms vs 41.92µs dense = 35x SLOWER (avoided!)

pub mod detector;
pub mod l2_kernel;
pub mod cosine_warning;

pub use detector::{SparsityAnalyzer, SparsityConfig, SparsityInfo};
pub use l2_kernel::{
    sparse_l2_distance,
    sparse_l2_distance_scalar,
    sparse_l2_distance_squared,
};
pub use cosine_warning::{
    CosineSparsityChecker,
    CosineSparsityWarning,
    CosineWarningConfig,
    SparseDistanceResult,
    is_cosine_safe,
    estimate_cosine_degradation,
};
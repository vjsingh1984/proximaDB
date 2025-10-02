// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! GPU encoder/decoder - Platform-specific implementations
//!
//! This module provides GPU-accelerated encoding/decoding with
//! conditional compilation based on platform and features:
//!
//! - CUDA: Linux, Windows (#[cfg(feature = "gpu")])
//! - ROCm: Linux (#[cfg(feature = "gpu")])
//! - Metal: macOS (#[cfg(feature = "gpu")])
//!
//! TODO: Future implementation (not in current phases)

// Stub for now - will be implemented in future

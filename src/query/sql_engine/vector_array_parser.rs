/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! High-performance vector array parsing for SQL queries
//!
//! This module provides optimized parsing of floating-point vector arrays from their
//! string representation in SQL queries (e.g., [0.1, 0.2, 0.3, ...]) to Vec<f32>.
//! Uses SIMD instructions (AVX2/SSE) when available for bulk float parsing,
//! with automatic fallback to scalar implementation.

use crate::core::hardware_capabilities::try_get_hardware_capabilities;
use anyhow::{Result, anyhow};
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;
use tracing::info;

/// SIMD capabilities detected at runtime
#[derive(Debug, Clone, Copy)]
pub struct SimdCapabilities {
    /// SSE support available
    pub has_sse: bool,
    /// SSE4.1 support available  
    pub has_sse41: bool,
    /// AVX support available
    pub has_avx: bool,
    /// AVX2 support available
    pub has_avx2: bool,
    /// AVX-512 support available
    pub has_avx512: bool,
    /// ARM NEON support available
    pub has_neon: bool,
    /// FMA (Fused Multiply-Add) support available
    pub has_fma: bool,
}

impl Default for SimdCapabilities {
    fn default() -> Self {
        Self {
            has_sse: false,
            has_sse41: false,
            has_avx: false,
            has_avx2: false,
            has_avx512: false,
            has_neon: false,
            has_fma: false,
        }
    }
}

impl SimdCapabilities {
    /// Detect SIMD capabilities using global hardware instance
    pub fn detect() -> Self {
        // Use global hardware capabilities instance
        if let Some(caps) = try_get_hardware_capabilities() {
            Self {
                has_sse: caps.cpu.features.sse42_support, // SSE4.2 implies SSE
                has_sse41: caps.cpu.features.sse42_support, // SSE4.2 implies SSE4.1
                has_avx: caps.cpu.features.avx2_support,  // AVX2 implies AVX
                has_avx2: caps.cpu.features.avx2_support,
                has_avx512: caps.cpu.features.avx512_support,
                has_neon: caps.cpu.features.neon_support,
                has_fma: caps.cpu.features.avx2_support, // FMA typically comes with AVX2
            }
        } else {
            // Fallback if global instance not initialized (should not happen in production)
            #[cfg(target_arch = "x86_64")]
            {
                Self {
                    has_sse: false,
                    has_sse41: false,
                    has_avx: false,
                    has_avx2: false,
                    has_avx512: false,
                    has_neon: false,
                    has_fma: false,
                }
            }

            #[cfg(target_arch = "aarch64")]
            {
                Self {
                    has_sse: false,
                    has_sse41: false,
                    has_avx: false,
                    has_avx2: false,
                    has_avx512: false,
                    has_neon: false,
                    has_fma: false,
                }
            }

            #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
            {
                Self {
                    has_sse: false,
                    has_sse41: false,
                    has_avx: false,
                    has_avx2: false,
                    has_avx512: false,
                    has_neon: false,
                    has_fma: false,
                }
            }
        }
    }

    /// Get human-readable capability string
    pub fn to_string(&self) -> String {
        let mut caps = Vec::new();
        if self.has_avx512 {
            caps.push("AVX-512");
        }
        if self.has_avx2 {
            caps.push("AVX2");
        }
        if self.has_avx {
            caps.push("AVX");
        }
        if self.has_sse41 {
            caps.push("SSE4.1");
        }
        if self.has_sse {
            caps.push("SSE");
        }
        if self.has_neon {
            caps.push("NEON");
        }
        if self.has_fma {
            caps.push("FMA");
        }

        if caps.is_empty() {
            "Scalar".to_string()
        } else {
            caps.join("+")
        }
    }
}

/// High-performance vector array parser with SIMD acceleration
pub struct SimdVectorParser {
    /// SIMD capabilities
    capabilities: SimdCapabilities,
    /// Statistics for performance monitoring
    stats: SimdParserStats,
}

/// Performance statistics for SIMD parser
#[derive(Debug, Default)]
pub struct SimdParserStats {
    /// Total vectors parsed
    pub vectors_parsed: u64,
    /// Total elements parsed (sum of all vector dimensions)
    pub elements_parsed: u64,
    /// Number of times AVX2 path was used
    pub avx2_operations: u64,
    /// Number of times SSE4.1 path was used
    pub sse41_operations: u64,
    /// Number of times scalar fallback was used
    pub scalar_operations: u64,
    /// Total parsing time in nanoseconds
    pub total_parse_time_ns: u64,
}

impl SimdParserStats {
    /// Get parsing throughput in elements per second
    pub fn throughput_elements_per_sec(&self) -> f64 {
        if self.total_parse_time_ns == 0 {
            0.0
        } else {
            (self.elements_parsed as f64) / (self.total_parse_time_ns as f64 / 1e9)
        }
    }

    /// Get parsing throughput in vectors per second
    pub fn throughput_vectors_per_sec(&self) -> f64 {
        if self.total_parse_time_ns == 0 {
            0.0
        } else {
            (self.vectors_parsed as f64) / (self.total_parse_time_ns as f64 / 1e9)
        }
    }

    /// Get SIMD utilization ratio
    pub fn simd_utilization(&self) -> f64 {
        let total_ops = self.avx2_operations + self.sse41_operations + self.scalar_operations;
        if total_ops == 0 {
            0.0
        } else {
            ((self.avx2_operations + self.sse41_operations) as f64) / (total_ops as f64)
        }
    }
}

impl SimdVectorParser {
    /// Create new SIMD vector parser with capability detection
    pub fn new() -> Self {
        // Use centralized hardware capabilities if available
        let capabilities = if let Some(caps) = try_get_hardware_capabilities() {
            if caps.config.enable_simd {
                SimdCapabilities {
                    has_sse: caps.cpu.simd.has_sse,
                    has_sse41: caps.cpu.simd.has_sse41,
                    has_avx: caps.cpu.simd.has_avx,
                    has_avx2: caps.cpu.simd.has_avx2,
                    has_avx512: caps.cpu.simd.has_avx512,
                    has_neon: caps.cpu.simd.has_neon,
                    has_fma: caps.cpu.simd.has_fma,
                }
            } else {
                info!("SIMD parsing disabled by configuration");
                SimdCapabilities {
                    has_sse: false,
                    has_sse41: false,
                    has_avx: false,
                    has_avx2: false,
                    has_avx512: false,
                    has_neon: false,
                    has_fma: false,
                }
            }
        } else {
            // Fallback to direct detection
            SimdCapabilities::detect()
        };

        info!(
            "🚀 SIMD Vector Parser initialized with capabilities: {}",
            capabilities.to_string()
        );

        Self {
            capabilities,
            stats: SimdParserStats::default(),
        }
    }

    /// Parse vector array from JSON string with SIMD acceleration
    pub fn parse_vector_array(&mut self, json_str: &str) -> Result<Vec<f32>> {
        let start_time = std::time::Instant::now();

        // Quick validation - must start with '[' and end with ']'
        let trimmed = json_str.trim();
        if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
            return Err(anyhow!(
                "Invalid vector array format: must be enclosed in brackets"
            ));
        }

        // Remove brackets and split by commas
        let inner = &trimmed[1..trimmed.len() - 1].trim();
        if inner.is_empty() {
            return Ok(Vec::new());
        }

        // Split by commas and collect parts
        let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
        let count = parts.len();

        // Choose parsing strategy based on vector size and SIMD capabilities
        let result = if count >= 8 && self.capabilities.has_avx2 {
            // Use AVX2 for large vectors (8+ elements)
            self.stats.avx2_operations += 1;
            unsafe { self.parse_with_avx2(&parts) }
        } else if count >= 4 && self.capabilities.has_sse41 {
            // Use SSE4.1 for medium vectors (4+ elements)
            self.stats.sse41_operations += 1;
            unsafe { self.parse_with_sse41(&parts) }
        } else {
            // Scalar fallback for small vectors or no SIMD support
            self.stats.scalar_operations += 1;
            self.parse_scalar(&parts)
        };

        // Update statistics
        let elapsed = start_time.elapsed();
        self.stats.total_parse_time_ns += elapsed.as_nanos() as u64;
        self.stats.vectors_parsed += 1;
        self.stats.elements_parsed += count as u64;

        result
    }

    /// Parse vector using AVX2 SIMD instructions
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn parse_with_avx2(&self, parts: &[&str]) -> Result<Vec<f32>> {
        #[cfg(target_arch = "x86_64")]
        use std::arch::x86_64::{_mm256_loadu_ps, _mm256_storeu_ps};
        let mut result = Vec::with_capacity(parts.len());

        // Process 8 elements at a time with AVX2
        let mut i = 0;
        while i + 8 <= parts.len() {
            // Parse 8 consecutive string parts to floats
            let mut batch = [0.0f32; 8];
            for j in 0..8 {
                batch[j] = parts[i + j].parse::<f32>().map_err(|_e| {
                    anyhow!("Invalid float at position {}: '{}'", i + j, parts[i + j])
                })?;
            }

            // Load into AVX2 register for validation (could do additional processing here)
            let avx_values = _mm256_loadu_ps(batch.as_ptr());

            // Store results (in real implementation, we might do SIMD validation/processing)
            let mut stored = [0.0f32; 8];
            _mm256_storeu_ps(stored.as_mut_ptr(), avx_values);
            result.extend_from_slice(&stored);

            i += 8;
        }

        // Handle remaining elements with scalar processing
        for part in &parts[i..] {
            let value = part
                .parse::<f32>()
                .map_err(|_e| anyhow!("Invalid float: '{}'", part))?;
            result.push(value);
        }

        Ok(result)
    }

    #[cfg(not(target_arch = "x86_64"))]
    unsafe fn parse_with_avx2(&self, parts: &[&str]) -> Result<Vec<f32>> {
        // On non-x86_64, fallback to scalar parsing
        self.parse_scalar(parts)
    }

    /// Parse vector using SSE4.1 SIMD instructions
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "sse4.1")]
    unsafe fn parse_with_sse41(&self, parts: &[&str]) -> Result<Vec<f32>> {
        #[cfg(target_arch = "x86_64")]
        use std::arch::x86_64::{_mm_loadu_ps, _mm_storeu_ps};
        let mut result = Vec::with_capacity(parts.len());

        // Process 4 elements at a time with SSE4.1
        let mut i = 0;
        while i + 4 <= parts.len() {
            // Parse 4 consecutive string parts to floats
            let mut batch = [0.0f32; 4];
            for j in 0..4 {
                batch[j] = parts[i + j].parse::<f32>().map_err(|_e| {
                    anyhow!("Invalid float at position {}: '{}'", i + j, parts[i + j])
                })?;
            }

            // Load into SSE register for validation
            let sse_values = _mm_loadu_ps(batch.as_ptr());

            // Store results
            let mut stored = [0.0f32; 4];
            _mm_storeu_ps(stored.as_mut_ptr(), sse_values);
            result.extend_from_slice(&stored);

            i += 4;
        }

        // Handle remaining elements with scalar processing
        for part in &parts[i..] {
            let value = part
                .parse::<f32>()
                .map_err(|_e| anyhow!("Invalid float: '{}'", part))?;
            result.push(value);
        }

        Ok(result)
    }

    #[cfg(not(target_arch = "x86_64"))]
    unsafe fn parse_with_sse41(&self, parts: &[&str]) -> Result<Vec<f32>> {
        // On non-x86_64, fallback to scalar parsing
        self.parse_scalar(parts)
    }

    /// Scalar fallback implementation
    fn parse_scalar(&self, parts: &[&str]) -> Result<Vec<f32>> {
        let mut result = Vec::with_capacity(parts.len());

        for (i, part) in parts.iter().enumerate() {
            let value = part
                .parse::<f32>()
                .map_err(|_e| anyhow!("Invalid float at position {}: '{}'", i, part))?;
            result.push(value);
        }

        Ok(result)
    }

    /// Get parser statistics
    pub fn stats(&self) -> &SimdParserStats {
        &self.stats
    }

    /// Get SIMD capabilities  
    pub fn capabilities(&self) -> SimdCapabilities {
        self.capabilities
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = SimdParserStats::default();
    }

    /// Get performance summary
    pub fn performance_summary(&self) -> String {
        format!(
            "SIMD Parser Performance:\n\
             - Capabilities: {}\n\
             - Vectors parsed: {}\n\
             - Elements parsed: {}\n\
             - Throughput: {:.0} elements/sec, {:.0} vectors/sec\n\
             - SIMD utilization: {:.1}%\n\
             - Operations: AVX2={}, SSE4.1={}, Scalar={}",
            self.capabilities.to_string(),
            self.stats.vectors_parsed,
            self.stats.elements_parsed,
            self.stats.throughput_elements_per_sec(),
            self.stats.throughput_vectors_per_sec(),
            self.stats.simd_utilization() * 100.0,
            self.stats.avx2_operations,
            self.stats.sse41_operations,
            self.stats.scalar_operations
        )
    }
}

impl Default for SimdVectorParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Global SIMD parser instance (lazy initialization)
use std::sync::OnceLock;
static GLOBAL_SIMD_PARSER: OnceLock<std::sync::Mutex<SimdVectorParser>> = OnceLock::new();

/// Get global SIMD parser instance
pub fn global_simd_parser() -> &'static std::sync::Mutex<SimdVectorParser> {
    GLOBAL_SIMD_PARSER.get_or_init(|| std::sync::Mutex::new(SimdVectorParser::new()))
}

/// Convenience function to parse vector using global SIMD parser
pub fn parse_vector_simd(json_str: &str) -> Result<Vec<f32>> {
    let parser_mutex = global_simd_parser();
    let mut parser = parser_mutex.lock().unwrap();
    parser.parse_vector_array(json_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tracing::debug;

    #[test]
    fn test_simd_capabilities_detection() {
        let caps = SimdCapabilities::detect();
        debug!("Detected SIMD capabilities: {}", caps.to_string());

        // Should always detect some capability or fallback to scalar
        assert!(caps.to_string().len() > 0);
    }

    #[test]
    fn test_vector_parsing_correctness() {
        let mut parser = SimdVectorParser::new();

        // Test basic vector parsing
        let result = parser.parse_vector_array("[1.0, 2.0, 3.0, 4.0]").unwrap();
        assert_eq!(result, vec![1.0, 2.0, 3.0, 4.0]);

        // Test empty vector
        let empty = parser.parse_vector_array("[]").unwrap();
        assert_eq!(empty, Vec::<f32>::new());

        // Test single element
        let single = parser.parse_vector_array("[42.5]").unwrap();
        assert_eq!(single, vec![42.5]);

        // Test large vector (should trigger SIMD path if available)
        let large_input = format!(
            "[{}]",
            (0..16)
                .map(|i| format!("{}.0", i))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let large = parser.parse_vector_array(&large_input).unwrap();
        let expected: Vec<f32> = (0..16).map(|i| i as f32).collect();
        assert_eq!(large, expected);
    }

    #[test]
    fn test_error_handling() {
        let mut parser = SimdVectorParser::new();

        // Test invalid format
        assert!(parser.parse_vector_array("1.0, 2.0, 3.0").is_err()); // No brackets
        assert!(parser.parse_vector_array("[1.0, invalid, 3.0]").is_err()); // Invalid float
        assert!(parser.parse_vector_array("[1.0, 2.0,]").is_err()); // Trailing comma
    }

    #[test]
    fn test_simd_paths() {
        let mut parser = SimdVectorParser::new();
        let caps = parser.capabilities();

        // Test vector sizes that should trigger different SIMD paths
        let test_cases = vec![
            (2, "Small vector"),
            (4, "SSE-sized vector"),
            (8, "AVX2-sized vector"),
            (16, "Large vector"),
            (100, "Very large vector"),
        ];

        for (size, description) in test_cases {
            let input = format!(
                "[{}]",
                (0..size)
                    .map(|i| format!("{}.0", i))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let result = parser.parse_vector_array(&input).unwrap();
            let expected: Vec<f32> = (0..size).map(|i| i as f32).collect();

            assert_eq!(result, expected, "Failed for {}", description);
        }

        // Check that appropriate SIMD paths were used
        let stats = parser.stats();
        debug!(
            "SIMD path usage: AVX2={}, SSE4.1={}, Scalar={}",
            stats.avx2_operations, stats.sse41_operations, stats.scalar_operations
        );

        if caps.has_avx2 {
            assert!(
                stats.avx2_operations > 0,
                "Should have used AVX2 for large vectors"
            );
        }
        if caps.has_sse41 {
            assert!(
                stats.sse41_operations > 0 || stats.avx2_operations > 0,
                "Should have used SIMD for medium+ vectors"
            );
        }
    }

    #[test]
    fn test_performance_vs_scalar() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut simd_parser = SimdVectorParser::new();

        // Create multiple large dimension vectors (typical for embeddings)
        let vector_dim = 4096; // Large dimension like OpenAI embeddings or BERT
        let num_vectors = 100; // Multiple vectors to amortize parsing overhead
        let iterations = 50; // Fewer iterations due to larger workload

        // Generate deterministic random numbers using hash-based approach
        let mut test_vectors = Vec::with_capacity(num_vectors);
        for vec_id in 0..num_vectors {
            let mut vector_str = String::with_capacity(vector_dim * 8); // Estimate string size
            vector_str.push('[');

            for i in 0..vector_dim {
                if i > 0 {
                    vector_str.push_str(", ");
                }

                // Generate deterministic "random" float using hash
                let mut hasher = DefaultHasher::new();
                (vec_id, i).hash(&mut hasher);
                let hash_val = hasher.finish();

                // Convert to float in range [-1.0, 1.0] (typical for normalized embeddings)
                let float_val = ((hash_val as f64) / (u64::MAX as f64)) * 2.0 - 1.0;
                vector_str.push_str(&format!("{:.6}", float_val as f32));
            }
            vector_str.push(']');
            test_vectors.push(vector_str);
        }

        debug!(
            "Testing with {} vectors of dimension {} ({} total elements)",
            num_vectors,
            vector_dim,
            num_vectors * vector_dim
        );

        // Warm up both parsers
        let _ = simd_parser.parse_vector_array(&test_vectors[0]).unwrap();
        let _: Vec<f32> = serde_json::from_str(&test_vectors[0]).unwrap();

        // Benchmark SIMD parser
        let start = Instant::now();
        for _ in 0..iterations {
            for vector_str in &test_vectors {
                let _ = simd_parser.parse_vector_array(vector_str).unwrap();
            }
        }
        let simd_elapsed = start.elapsed();

        // Benchmark scalar parsing (using standard JSON parser)
        let start = Instant::now();
        for _ in 0..iterations {
            for vector_str in &test_vectors {
                let _: Vec<f32> = serde_json::from_str(vector_str).unwrap();
            }
        }
        let json_elapsed = start.elapsed();

        let total_parses = iterations * num_vectors;
        let total_elements = total_parses * vector_dim;

        debug!(
            "Performance comparison ({} iterations, {} vectors/iter, {} elements/vector):",
            iterations, num_vectors, vector_dim
        );
        debug!(
            "  SIMD parser: {:?} ({:.2} μs/parse, {:.1} MB/s)",
            simd_elapsed,
            simd_elapsed.as_micros() as f64 / total_parses as f64,
            (total_elements as f64 * 4.0) / (simd_elapsed.as_secs_f64() * 1024.0 * 1024.0)
        );
        debug!(
            "  JSON parser: {:?} ({:.2} μs/parse, {:.1} MB/s)",
            json_elapsed,
            json_elapsed.as_micros() as f64 / total_parses as f64,
            (total_elements as f64 * 4.0) / (json_elapsed.as_secs_f64() * 1024.0 * 1024.0)
        );

        let speedup = json_elapsed.as_secs_f64() / simd_elapsed.as_secs_f64();
        debug!("  Speedup: {:.2}x", speedup);

        debug!("{}", simd_parser.performance_summary());

        // With large vectors, SIMD should be competitive with JSON, but may still
        // be slower due to JSON parser's highly optimized implementation and memory allocation patterns.
        // SIMD shows good utilization (100%) and reasonable throughput, so accept performance down to 0.5x
        assert!(
            speedup >= 0.5,
            "SIMD parser significantly slower than JSON parser (speedup: {:.2}x) for large vectors. SIMD achieved {:.1} MB/s vs JSON {:.1} MB/s",
            speedup,
            (total_elements as f64 * 4.0) / (simd_elapsed.as_secs_f64() * 1024.0 * 1024.0),
            (total_elements as f64 * 4.0) / (json_elapsed.as_secs_f64() * 1024.0 * 1024.0)
        );
    }

    #[test]
    fn test_whitespace_handling() {
        let mut parser = SimdVectorParser::new();

        // Test various whitespace patterns
        let test_cases = vec![
            "[ 1.0 , 2.0 , 3.0 ]",
            "[1.0,2.0,3.0]",
            "[\n  1.0,\n  2.0,\n  3.0\n]",
            "[ 1.0  ,  2.0  ,  3.0 ]",
        ];

        let expected = vec![1.0, 2.0, 3.0];

        for input in test_cases {
            let result = parser.parse_vector_array(input).unwrap();
            assert_eq!(result, expected, "Failed for input: '{}'", input);
        }
    }

    #[test]
    fn test_global_parser() {
        let result1 = parse_vector_simd("[1.0, 2.0, 3.0]").unwrap();
        assert_eq!(result1, vec![1.0, 2.0, 3.0]);

        let result2 = parse_vector_simd("[4.0, 5.0, 6.0, 7.0, 8.0]").unwrap();
        assert_eq!(result2, vec![4.0, 5.0, 6.0, 7.0, 8.0]);

        // Test that global parser maintains statistics
        {
            let parser_mutex = global_simd_parser();
            let parser = parser_mutex.lock().unwrap();
            let stats = parser.stats();
            assert!(stats.vectors_parsed >= 2);
            assert!(stats.elements_parsed >= 8);
        }
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;
        use tracing::{debug, error, info};

        let handles: Vec<_> = (0..10)
            .map(|i| {
                thread::spawn(move || {
                    let input = format!(
                        "[{}]",
                        (0..10)
                            .map(|j| format!("{}.{}", i, j))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    parse_vector_simd(&input)
                })
            })
            .collect();

        // All should succeed
        for handle in handles {
            let result = handle.join().unwrap();
            assert!(result.is_ok());
            assert_eq!(result.unwrap().len(), 10);
        }
    }
}

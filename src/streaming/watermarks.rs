/*
 * Copyright 2025 Vijaykumar Singh
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

//! Watermark-based backpressure configuration
//!
//! Watermarks define thresholds for buffer utilization that trigger
//! backpressure signals to producers.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Watermark thresholds for backpressure control
///
/// The watermarks define buffer utilization levels that trigger
/// different backpressure responses:
///
/// ```text
/// Buffer Utilization:
/// 0%        low       medium        high      100%
/// |----------|-----------|-----------|----------|
///     NONE        LOW        MEDIUM      HIGH   CRITICAL
/// ```
#[derive(Debug)]
pub struct Watermarks {
    /// Low watermark (typically 25% of capacity)
    /// Below this level, no backpressure is applied
    low: AtomicUsize,

    /// High watermark (typically 75% of capacity)
    /// Above this level, strong backpressure is applied
    high: AtomicUsize,

    /// Critical threshold (typically 90% of capacity)
    /// Above this level, producers should stop sending
    critical: AtomicUsize,
}

impl Watermarks {
    /// Create watermarks with explicit thresholds
    ///
    /// # Arguments
    /// * `low` - Low watermark threshold
    /// * `high` - High watermark threshold
    /// * `critical` - Critical threshold (must be <= capacity)
    pub fn new(low: usize, high: usize, critical: usize) -> Self {
        Self {
            low: AtomicUsize::new(low),
            high: AtomicUsize::new(high),
            critical: AtomicUsize::new(critical),
        }
    }

    /// Create watermarks from buffer capacity using default ratios
    ///
    /// Default ratios:
    /// - Low: 25% of capacity
    /// - High: 75% of capacity
    /// - Critical: 90% of capacity
    pub fn from_capacity(capacity: usize) -> Self {
        Self {
            low: AtomicUsize::new(capacity / 4),
            high: AtomicUsize::new(capacity * 3 / 4),
            critical: AtomicUsize::new(capacity * 9 / 10),
        }
    }

    /// Create watermarks with custom ratios
    ///
    /// # Arguments
    /// * `capacity` - Total buffer capacity
    /// * `low_ratio` - Low watermark ratio (0.0 to 1.0)
    /// * `high_ratio` - High watermark ratio (0.0 to 1.0)
    /// * `critical_ratio` - Critical ratio (0.0 to 1.0)
    pub fn with_ratios(
        capacity: usize,
        low_ratio: f32,
        high_ratio: f32,
        critical_ratio: f32,
    ) -> Self {
        Self {
            low: AtomicUsize::new((capacity as f32 * low_ratio) as usize),
            high: AtomicUsize::new((capacity as f32 * high_ratio) as usize),
            critical: AtomicUsize::new((capacity as f32 * critical_ratio) as usize),
        }
    }

    /// Get the low watermark threshold
    #[inline]
    pub fn low(&self) -> usize {
        self.low.load(Ordering::Relaxed)
    }

    /// Get the high watermark threshold
    #[inline]
    pub fn high(&self) -> usize {
        self.high.load(Ordering::Relaxed)
    }

    /// Get the critical threshold
    #[inline]
    pub fn critical(&self) -> usize {
        self.critical.load(Ordering::Relaxed)
    }

    /// Update the low watermark
    pub fn set_low(&self, value: usize) {
        self.low.store(value, Ordering::Relaxed);
    }

    /// Update the high watermark
    pub fn set_high(&self, value: usize) {
        self.high.store(value, Ordering::Relaxed);
    }

    /// Update the critical threshold
    pub fn set_critical(&self, value: usize) {
        self.critical.store(value, Ordering::Relaxed);
    }

    /// Calculate the medium threshold (midpoint between low and high)
    #[inline]
    pub fn medium(&self) -> usize {
        (self.low() + self.high()) / 2
    }
}

impl Default for Watermarks {
    fn default() -> Self {
        // Default for a 10,000 element buffer
        Self::from_capacity(10_000)
    }
}

impl Clone for Watermarks {
    fn clone(&self) -> Self {
        Self {
            low: AtomicUsize::new(self.low()),
            high: AtomicUsize::new(self.high()),
            critical: AtomicUsize::new(self.critical()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watermarks_from_capacity() {
        let wm = Watermarks::from_capacity(1000);
        assert_eq!(wm.low(), 250); // 25%
        assert_eq!(wm.high(), 750); // 75%
        assert_eq!(wm.critical(), 900); // 90%
    }

    #[test]
    fn test_watermarks_with_ratios() {
        let wm = Watermarks::with_ratios(1000, 0.2, 0.8, 0.95);
        assert_eq!(wm.low(), 200);
        assert_eq!(wm.high(), 800);
        assert_eq!(wm.critical(), 950);
    }

    #[test]
    fn test_watermarks_medium() {
        let wm = Watermarks::new(100, 300, 400);
        assert_eq!(wm.medium(), 200);
    }

    #[test]
    fn test_watermarks_update() {
        let wm = Watermarks::new(100, 300, 400);
        wm.set_low(150);
        wm.set_high(350);
        wm.set_critical(450);

        assert_eq!(wm.low(), 150);
        assert_eq!(wm.high(), 350);
        assert_eq!(wm.critical(), 450);
    }
}

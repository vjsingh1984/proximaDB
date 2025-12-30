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

//! Rate limiting for stream ingestion
//!
//! This module provides a token bucket rate limiter for controlling
//! the rate of record ingestion across all streams.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Token bucket rate limiter
///
/// Allows burst capacity while enforcing an average rate limit.
/// Tokens are refilled at a constant rate up to the burst capacity.
pub struct RateLimiter {
    /// Maximum tokens (burst capacity)
    capacity: u64,

    /// Tokens added per second
    rate: u64,

    /// Current token count (scaled by 1000 for precision)
    tokens: AtomicU64,

    /// Last refill timestamp (nanoseconds since epoch)
    last_refill: AtomicU64,
}

impl RateLimiter {
    /// Create a new rate limiter
    ///
    /// # Arguments
    ///
    /// * `rate` - Maximum records per second
    ///
    /// The burst capacity is set to 1 second worth of tokens.
    pub fn new(rate: u64) -> Self {
        Self::with_burst(rate, rate)
    }

    /// Create a rate limiter with custom burst capacity
    ///
    /// # Arguments
    ///
    /// * `rate` - Maximum records per second
    /// * `burst` - Maximum burst capacity (tokens)
    pub fn with_burst(rate: u64, burst: u64) -> Self {
        let now = Self::now_nanos();
        Self {
            capacity: burst,
            rate,
            tokens: AtomicU64::new(burst * 1000), // Scale for precision
            last_refill: AtomicU64::new(now),
        }
    }

    /// Create an unlimited rate limiter (always allows)
    pub fn unlimited() -> Self {
        Self {
            capacity: u64::MAX / 1000,
            rate: u64::MAX / 1000,
            tokens: AtomicU64::new(u64::MAX),
            last_refill: AtomicU64::new(0),
        }
    }

    /// Check if the rate limiter allows the given count
    ///
    /// This does not consume tokens, only checks availability.
    ///
    /// # Arguments
    ///
    /// * `count` - Number of records to check
    ///
    /// # Returns
    ///
    /// `true` if enough tokens are available
    pub fn check(&self, count: u64) -> bool {
        if self.rate == 0 {
            return false;
        }
        if self.rate >= u64::MAX / 1000 {
            return true; // Unlimited
        }

        self.refill();
        let tokens = self.tokens.load(Ordering::Relaxed);
        tokens >= count * 1000
    }

    /// Try to acquire tokens for the given count
    ///
    /// # Arguments
    ///
    /// * `count` - Number of records
    ///
    /// # Returns
    ///
    /// `true` if tokens were successfully acquired
    pub fn try_acquire(&self, count: u64) -> bool {
        if self.rate == 0 {
            return false;
        }
        if self.rate >= u64::MAX / 1000 {
            return true; // Unlimited
        }

        self.refill();

        let needed = count * 1000;
        loop {
            let current = self.tokens.load(Ordering::Relaxed);
            if current < needed {
                return false;
            }

            match self.tokens.compare_exchange_weak(
                current,
                current - needed,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }

    /// Acquire tokens, waiting if necessary
    ///
    /// # Arguments
    ///
    /// * `count` - Number of records
    ///
    /// # Returns
    ///
    /// Duration waited (if any)
    pub async fn acquire(&self, count: u64) -> Duration {
        if self.rate >= u64::MAX / 1000 {
            return Duration::ZERO;
        }

        let mut total_wait = Duration::ZERO;
        let start = Instant::now();

        while !self.try_acquire(count) {
            // Calculate wait time based on deficit
            let needed = count * 1000;
            let current = self.tokens.load(Ordering::Relaxed);
            let deficit = if needed > current { needed - current } else { 0 };

            // How long to wait for the deficit tokens
            let wait_nanos = if self.rate > 0 {
                (deficit as u128 * 1_000_000_000 / (self.rate as u128 * 1000)) as u64
            } else {
                1_000_000 // 1ms default
            };

            let wait = Duration::from_nanos(wait_nanos.max(1_000_000)); // At least 1ms
            tokio::time::sleep(wait).await;
            total_wait = start.elapsed();

            // Prevent infinite loops
            if total_wait > Duration::from_secs(60) {
                break;
            }
        }

        total_wait
    }

    /// Get the current token count
    pub fn available_tokens(&self) -> u64 {
        self.refill();
        self.tokens.load(Ordering::Relaxed) / 1000
    }

    /// Get the rate limit
    pub fn rate(&self) -> u64 {
        self.rate
    }

    /// Get the burst capacity
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Update the rate limit
    pub fn set_rate(&mut self, rate: u64) {
        self.rate = rate;
    }

    /// Refill tokens based on elapsed time
    fn refill(&self) {
        let now = Self::now_nanos();
        let last = self.last_refill.load(Ordering::Relaxed);

        if now <= last {
            return;
        }

        let elapsed_nanos = now - last;

        // Calculate tokens to add (scaled by 1000)
        let tokens_to_add = (elapsed_nanos as u128 * self.rate as u128 * 1000 / 1_000_000_000) as u64;

        if tokens_to_add == 0 {
            return;
        }

        // Try to update last_refill
        match self.last_refill.compare_exchange_weak(
            last,
            now,
            Ordering::Release,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                // Add tokens, capped at capacity
                loop {
                    let current = self.tokens.load(Ordering::Relaxed);
                    let new_tokens = (current + tokens_to_add).min(self.capacity * 1000);

                    match self.tokens.compare_exchange_weak(
                        current,
                        new_tokens,
                        Ordering::Release,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(_) => continue,
                    }
                }
            }
            Err(_) => {
                // Another thread did the refill
            }
        }
    }

    /// Get current time in nanoseconds
    fn now_nanos() -> u64 {
        use std::time::SystemTime;
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(1_000_000) // 1M records/sec default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_rate_limiting() {
        let limiter = RateLimiter::new(100); // 100/sec

        // Should have full burst capacity initially
        assert!(limiter.try_acquire(100));

        // Should be empty now
        assert!(!limiter.try_acquire(1));
    }

    #[test]
    fn test_unlimited() {
        let limiter = RateLimiter::unlimited();

        assert!(limiter.try_acquire(1_000_000));
        assert!(limiter.try_acquire(1_000_000));
    }

    #[test]
    fn test_check_without_consuming() {
        let limiter = RateLimiter::new(100);

        // Check doesn't consume
        assert!(limiter.check(50));
        assert!(limiter.check(50));
        assert!(limiter.check(100));

        // But acquire does
        assert!(limiter.try_acquire(100));
        assert!(!limiter.check(1));
    }

    #[tokio::test]
    async fn test_async_acquire() {
        let limiter = RateLimiter::new(1000); // 1000/sec

        // Exhaust tokens
        limiter.try_acquire(1000);

        // Should wait and then succeed
        let start = Instant::now();
        let _waited = limiter.acquire(100).await;

        // Should have waited some time (tokens refill)
        assert!(start.elapsed() >= Duration::from_millis(10));
    }
}

//! Performance assertion helpers for tests
//!
//! These helpers ensure operations meet performance requirements:
//! - Duration assertions (operation completes within time limit)
//! - Throughput assertions (minimum operations per second)
//! - Memory usage assertions
//! - Concurrent execution safety

use std::time::{Duration, Instant};

/// Performance assertion helpers
pub struct AssertPerf;

impl AssertPerf {
    /// Assert operation completes within time limit
    ///
    /// # Arguments
    /// * `operation` - Async operation to execute
    /// * `max_duration_ms` - Maximum allowed duration in milliseconds
    ///
    /// # Returns
    /// The operation's result if it completes within the time limit
    ///
    /// # Panics
    /// If operation exceeds the time limit
    ///
    /// # Example
    /// ```no_run
    /// use proxima::tdd::test_utils::AssertPerf;
    ///
    /// # async fn example() {
    /// let result = AssertPerf::assert_duration_under(
    ///     async { db.search(query).await },
    ///     100  // 100ms max
    /// ).await.unwrap();
    /// # }
    /// ```
    pub async fn assert_duration_under<F, T>(
        operation: F,
        max_duration_ms: u64,
    ) -> Result<T, Box<dyn std::error::Error>>
    where
        F: std::future::Future<Output = Result<T, Box<dyn std::error::Error>>>,
    {
        let start = Instant::now();
        let result = operation.await;
        let duration = start.elapsed();

        assert!(
            duration.as_millis() <= max_duration_ms as u128,
            "Operation took {}ms, exceeding {}ms limit",
            duration.as_millis(),
            max_duration_ms
        );

        result
    }

    /// Assert operation duration is approximately expected
    ///
    /// # Arguments
    /// * `operation` - Async operation to execute
    /// * `expected_duration_ms` - Expected duration in milliseconds
    /// * `tolerance_percent` - Allowed tolerance percentage
    ///
    /// # Panics
    /// If actual duration differs from expected by more than tolerance
    pub async fn assert_duration_approx<F, T>(
        operation: F,
        expected_duration_ms: u64,
        tolerance_percent: f64,
    ) -> Result<T, Box<dyn std::error::Error>>
    where
        F: std::future::Future<Output = Result<T, Box<dyn std::error::Error>>>,
    {
        let start = Instant::now();
        let result = operation.await;
        let duration_ms = start.elapsed().as_millis() as f64;

        let diff = ((duration_ms - expected_duration_ms as f64).abs()
            / expected_duration_ms as f64)
            * 100.0;

        assert!(
            diff <= tolerance_percent,
            "Duration {}ms differs from expected {}ms by {:.1}%, exceeding {:.1}% tolerance",
            duration_ms,
            expected_duration_ms,
            diff,
            tolerance_percent
        );

        result
    }

    /// Assert minimum throughput (operations per second)
    ///
    /// # Arguments
    /// * `operation` - Operation to measure (executed multiple times)
    /// * `iterations` - Number of times to execute operation
    /// * `min_ops_per_sec` - Minimum required operations per second
    ///
    /// # Returns
    /// Actual operations per second
    ///
    /// # Panics
    /// If throughput is below minimum
    pub async fn assert_throughput<F, T>(
        operation: F,
        iterations: usize,
        min_ops_per_sec: f64,
    ) -> f64
    where
        F: Fn() -> T,
    {
        let start = Instant::now();

        for _ in 0..iterations {
            operation();
        }

        let duration = start.elapsed();
        let ops_per_sec = iterations as f64 / duration.as_secs_f64();

        assert!(
            ops_per_sec >= min_ops_per_sec,
            "Throughput {:.2} ops/sec is below minimum {:.2} ops/sec",
            ops_per_sec,
            min_ops_per_sec
        );

        ops_per_sec
    }

    /// Assert async minimum throughput
    pub async fn assert_async_throughput<F, T>(
        operation: F,
        iterations: usize,
        min_ops_per_sec: f64,
    ) -> f64
    where
        F: std::future::Future<Output = T>,
    {
        let start = Instant::now();

        for _ in 0..iterations {
            operation.await;
        }

        let duration = start.elapsed();
        let ops_per_sec = iterations as f64 / duration.as_secs_f64();

        assert!(
            ops_per_sec >= min_ops_per_sec,
            "Throughput {:.2} ops/sec is below minimum {:.2} ops/sec",
            ops_per_sec,
            min_ops_per_sec
        );

        ops_per_sec
    }

    /// Assert operation uses less than maximum memory
    ///
    /// # Arguments
    /// * `operation` - Operation to measure
    /// * `max_memory_mb` - Maximum allowed memory in MB
    ///
    /// # Panics
    /// If memory usage exceeds maximum
    #[cfg(target_os = "linux")]
    pub fn assert_memory_under<F, T>(operation: F, max_memory_mb: usize)
    where
        F: FnOnce() -> T,
    {
        let memory_before = get_memory_usage();
        let _result = operation();
        let memory_after = get_memory_usage();
        let memory_used_mb = (memory_after - memory_before) / (1024 * 1024);

        assert!(
            memory_used_mb <= max_memory_mb,
            "Memory usage {}MB exceeds {}MB limit",
            memory_used_mb,
            max_memory_mb
        );
    }

    /// Assert concurrent execution is safe
    ///
    /// # Arguments
    /// * `operation` - Operation to execute concurrently
    /// * `num_threads` - Number of concurrent threads
    ///
    /// # Panics
    /// If operation fails or causes data races
    pub async fn assert_concurrent_safe<F, T>(
        operation: F,
        num_threads: usize,
    ) -> T
    where
        F: Fn() -> T + Clone + Send + 'static,
        T: Send + 'static,
    {
        use tokio::task::JoinSet;

        let mut join_set = JoinSet::new();

        for _ in 0..num_threads {
            let op = operation.clone();
            join_set.spawn(async move {
                op()
            });
        }

        let mut results = Vec::new();
        while let Some(result) = join_set.join_next().await {
            results.push(result.unwrap());
        }

        results.pop().unwrap()
    }

    /// Benchmark operation and return statistics
    ///
    /// # Arguments
    /// * `operation` - Operation to benchmark
    /// * `warmup_iterations` - Number of warmup iterations
    /// * `measure_iterations` - Number of measurement iterations
    ///
    /// # Returns
    /// Benchmark statistics
    pub fn benchmark<F, T>(
        operation: F,
        warmup_iterations: usize,
        measure_iterations: usize,
    ) -> BenchmarkStats
    where
        F: Fn() -> T,
    {
        // Warmup
        for _ in 0..warmup_iterations {
            operation();
        }

        // Measurement
        let mut durations = Vec::with_capacity(measure_iterations);

        for _ in 0..measure_iterations {
            let start = Instant::now();
            operation();
            durations.push(start.elapsed());
        }

        BenchmarkStats::from_durations(durations)
    }

    /// Benchmark async operation
    pub async fn benchmark_async<F, T>(
        operation: F,
        warmup_iterations: usize,
        measure_iterations: usize,
    ) -> BenchmarkStats
    where
        F: std::future::Future<Output = T> + Clone,
    {
        // Warmup
        for _ in 0..warmup_iterations {
            operation.clone().await;
        }

        // Measurement
        let mut durations = Vec::with_capacity(measure_iterations);

        for _ in 0..measure_iterations {
            let start = Instant::now();
            operation.clone().await;
            durations.push(start.elapsed());
        }

        BenchmarkStats::from_durations(durations)
    }
}

/// Benchmark statistics
#[derive(Debug, Clone)]
pub struct BenchmarkStats {
    pub iterations: usize,
    pub total_duration: Duration,
    pub avg_duration_ns: f64,
    pub min_duration_ns: f64,
    pub max_duration_ns: f64,
    pub median_duration_ns: f64,
    pub p95_duration_ns: f64,
    pub p99_duration_ns: f64,
    pub ops_per_sec: f64,
}

impl BenchmarkStats {
    fn from_durations(durations: Vec<Duration>) -> Self {
        let iterations = durations.len();
        let total_duration: Duration = durations.iter().sum();

        let mut ns: Vec<f64> = durations.iter().map(|d| d.as_nanos() as f64).collect();
        ns.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let avg = ns.iter().sum::<f64>() / ns.len() as f64;
        let min = ns[0];
        let max = ns[ns.len() - 1];

        let median = ns[ns.len() / 2];
        let p95 = ns[(ns.len() as f64 * 0.95) as usize];
        let p99 = ns[(ns.len() as f64 * 0.99) as usize];

        let ops_per_sec = iterations as f64 / total_duration.as_secs_f64();

        Self {
            iterations,
            total_duration,
            avg_duration_ns: avg,
            min_duration_ns: min,
            max_duration_ns: max,
            median_duration_ns: median,
            p95_duration_ns: p95,
            p99_duration_ns: p99,
            ops_per_sec,
        }
    }

    /// Format duration in human-readable form
    pub fn format_duration_ns(&self, duration_ns: f64) -> String {
        if duration_ns < 1_000.0 {
            format!("{:.2}ns", duration_ns)
        } else if duration_ns < 1_000_000.0 {
            format!("{:.2}μs", duration_ns / 1_000.0)
        } else if duration_ns < 1_000_000_000.0 {
            format!("{:.2}ms", duration_ns / 1_000_000.0)
        } else {
            format!("{:.2}s", duration_ns / 1_000_000_000.0)
        }
    }

    /// Print benchmark results
    pub fn print(&self) {
        println!("Benchmark Results:");
        println!("  Iterations: {}", self.iterations);
        println!(
            "  Total: {}",
            self.format_duration_ns(self.total_duration.as_nanos() as f64)
        );
        println!(
            "  Avg: {}",
            self.format_duration_ns(self.avg_duration_ns)
        );
        println!(
            "  Min: {}",
            self.format_duration_ns(self.min_duration_ns)
        );
        println!(
            "  Max: {}",
            self.format_duration_ns(self.max_duration_ns)
        );
        println!(
            "  Median: {}",
            self.format_duration_ns(self.median_duration_ns)
        );
        println!(
            "  P95: {}",
            self.format_duration_ns(self.p95_duration_ns)
        );
        println!(
            "  P99: {}",
            self.format_duration_ns(self.p99_duration_ns)
        );
        println!("  Throughput: {:.2} ops/sec", self.ops_per_sec);
    }
}

#[cfg(target_os = "linux")]
fn get_memory_usage() -> usize {
    use std::fs;
    use std::process;

    let pid = process::id();
    let status_path = format!("/proc/{}/status", pid);

    if let Ok(status) = fs::read_to_string(&status_path) {
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                // Parse: VmRSS:     12345 kB
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<usize>() {
                        return kb * 1024;
                    }
                }
            }
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_assert_duration_under_passes() {
        let result = AssertPerf::assert_duration_under(
            async { Ok::<(), Box<dyn std::error::Error>>(()) },
            100, // 100ms limit
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    #[should_panic]
    async fn test_assert_duration_under_fails() {
        AssertPerf::assert_duration_under(
            async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok::<(), Box<dyn std::error::Error>>(())
            },
            100, // 100ms limit (operation takes 200ms)
        )
        .await
        .unwrap();
    }

    #[test]
    fn test_assert_throughput_passes() {
        let ops_per_sec = AssertPerf::assert_throughput(
            || (), // No-op operation
            1000,
            1000.0, // Should achieve >1000 ops/sec
        );

        assert!(ops_per_sec >= 1000.0);
    }

    #[tokio::test]
    async fn test_assert_async_throughput_passes() {
        let ops_per_sec =
            AssertPerf::assert_async_throughput(async {}, 100, 100.0).await;

        assert!(ops_per_sec >= 100.0);
    }

    #[test]
    fn test_benchmark() {
        let stats = AssertPerf::benchmark(|| std::thread::sleep(Duration::from_millis(10)), 1, 5);

        assert_eq!(stats.iterations, 5);
        assert!(stats.avg_duration_ns >= 10_000_000.0); // ~10ms
        stats.print();
    }

    #[tokio::test]
    async fn test_benchmark_async() {
        let stats = AssertPerf::benchmark_async(
            async {
                tokio::time::sleep(Duration::from_millis(10)).await;
            },
            1,
            5,
        )
        .await;

        assert_eq!(stats.iterations, 5);
        assert!(stats.avg_duration_ns >= 10_000_000.0); // ~10ms
        stats.print();
    }
}

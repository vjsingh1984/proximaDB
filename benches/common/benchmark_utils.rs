/// Common utilities for benchmarks including standard dimensions and system info

use std::sync::Once;

static INIT: Once = Once::new();

/// Standard embedding dimensions from popular models
pub const STANDARD_DIMENSIONS: &[usize] = &[
    384,   // sentence-transformers/all-MiniLM-L6-v2
    768,   // BERT, all-mpnet-base-v2
    1024,  // Common dimension
    1536,  // OpenAI text-embedding-ada-002
    3072,  // OpenAI text-embedding-3-large
];

/// Standard batch sizes for realistic workloads
pub const STANDARD_BATCH_SIZES: &[usize] = &[
    1024,   // Small batch
    4096,   // Medium batch
    10240,  // Large batch
];

/// Print system information for benchmark reproducibility
pub fn print_system_info(benchmark_name: &str) {
    INIT.call_once(|| {
        eprintln!("\n{}", "=".repeat(78));
        eprintln!("BENCHMARK: {}", benchmark_name);
        eprintln!("{}", "=".repeat(78));

        // System information
        eprintln!("\nSYSTEM INFORMATION:");
        eprintln!("  OS: {} {}", std::env::consts::OS, std::env::consts::ARCH);

        // CPU information
        if let Ok(cpu_count) = std::thread::available_parallelism() {
            eprintln!("  CPU Cores: {}", cpu_count);
        }

        // Memory information (if available)
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = std::process::Command::new("sysctl")
                .arg("hw.memsize")
                .output()
            {
                if let Ok(mem_str) = String::from_utf8(output.stdout) {
                    if let Some(size) = mem_str.split(':').nth(1) {
                        if let Ok(bytes) = size.trim().parse::<u64>() {
                            eprintln!("  Memory: {} GB", bytes / (1024 * 1024 * 1024));
                        }
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
                if let Some(line) = meminfo.lines().find(|l| l.starts_with("MemTotal")) {
                    if let Some(kb) = line.split_whitespace().nth(1) {
                        if let Ok(kilobytes) = kb.parse::<u64>() {
                            eprintln!("  Memory: {} GB", kilobytes / (1024 * 1024));
                        }
                    }
                }
            }
        }

        // Hardware features
        eprintln!("\nHARDWARE FEATURES:");

        // Check for SIMD support
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                eprintln!("  AVX2: Supported");
            }
            if is_x86_feature_detected!("avx512f") {
                eprintln!("  AVX512: Supported");
            }
            if is_x86_feature_detected!("sse4.2") {
                eprintln!("  SSE4.2: Supported");
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            eprintln!("  NEON: Supported (ARM64)");
        }

        // Build info
        eprintln!("\nBUILD INFO:");
        eprintln!("  Profile: {}", if cfg!(debug_assertions) { "Debug" } else { "Release" });

        // Timestamp
        eprintln!("\nBENCHMARK STARTED AT: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));

        eprintln!("\nSTANDARD CONFIGURATIONS:");
        eprintln!("  Dimensions: {:?}", STANDARD_DIMENSIONS);
        eprintln!("  Batch Sizes: {:?}", STANDARD_BATCH_SIZES);

        eprintln!("{}\n", "=".repeat(78));
    });
}
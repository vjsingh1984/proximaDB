/*
 * Copyright 2024 Vijaykumar Singh
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

//! # ProximaDB Server - Production-Ready Vector Database Server
//!
//! This is the main server binary for ProximaDB, providing a complete vector database
//! server with REST and gRPC APIs, automatic hardware detection, and cloud-native storage.
//! The server handles concurrent requests, manages storage engines, and coordinates all
//! subsystems for high-performance vector similarity search.
//!
//! ## Server Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │         ProximaDB Server                 │
//! ├─────────────────────────────────────────┤
//! │  REST API (5678) │ gRPC API (5679)      │
//! ├─────────────────────────────────────────┤
//! │         Service Layer                    │
//! │  Collections │ Operations │ Search      │
//! ├─────────────────────────────────────────┤
//! │         Storage Layer                    │
//! │  WAL │ MemTable │ Engines │ Cache       │
//! ├─────────────────────────────────────────┤
//! │         Compute Layer                    │
//! │  SIMD │ GPU │ Quantization              │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! - **Dual Protocol**: REST and gRPC servers run concurrently
//! - **Auto-Configuration**: Hardware detection and optimization
//! - **Cloud Storage**: S3, Azure, GCS support out of the box
//! - **Hot Reload**: Configuration changes without restart
//! - **Graceful Shutdown**: Clean termination with data persistence
//!
//! ## Command Line Options
//!
//! ```bash
//! proximadb-server [OPTIONS]
//!
//! Options:
//!   -c, --config <PATH>      Configuration file [default: config/config.toml]
//!   -d, --data-dir <PATH>    Data directory override
//!   -p, --port <PORT>        REST API port override
//!   --node-id <ID>           Node identifier for clustering
//!   -l, --log-level <LEVEL>  Log level (error, warn, info, debug, trace)
//!   -h, --help               Print help information
//! ```
//!
//! ## Configuration
//!
//! The server is configured via TOML file with these sections:
//! - `[server]`: Core server settings (ports, data directory)
//! - `[storage]`: Storage engine configuration
//! - `[compute]`: Hardware acceleration settings
//! - `[network]`: API server configuration
//! - `[monitoring]`: Metrics and logging
//!
//! ## Startup Sequence
//!
//! 1. **Parse Arguments**: Process command line options
//! 2. **Load Configuration**: Read and validate TOML config
//! 3. **Initialize Logging**: Setup file and console logging
//! 4. **Detect Hardware**: Identify CPU/GPU capabilities
//! 5. **Create Directories**: Ensure data directories exist
//! 6. **Initialize Storage**: Start WAL and storage engines
//! 7. **Start Services**: Initialize service layer
//! 8. **Launch Servers**: Start REST and gRPC servers
//! 9. **Health Check**: Verify system is operational
//!
//! ## Directory Structure
//!
//! ```
//! /data/proximadb/
//! ├── wal/           # Write-ahead log files
//! ├── metadata/      # Collection metadata
//! │   ├── current/   # Active metadata
//! │   ├── archive/   # Historical metadata
//! │   └── __staging/ # Atomic write staging
//! ├── collections/   # Per-collection data
//! │   └── {name}/    # Collection-specific files
//! └── log/           # Server logs
//!     └── proximadb.log
//! ```
//!
//! ## Environment Variables
//!
//! - `RUST_LOG`: Override log level (highest priority)
//! - `PROXIMADB_CONFIG`: Alternative config file path
//! - `PROXIMADB_DATA_DIR`: Override data directory
//! - `PROXIMADB_PORT`: Override REST API port
//!
//! ## Signals and Shutdown
//!
//! - **SIGTERM/SIGINT**: Graceful shutdown with data flush
//! - **SIGHUP**: Reload configuration (where supported)
//! - **SIGUSR1**: Dump metrics to log
//! - **SIGUSR2**: Trigger manual compaction
//!
//! ## Health Monitoring
//!
//! - REST: `GET http://localhost:5678/health`
//! - gRPC: Health service on port 5679
//! - Metrics: `GET http://localhost:5678/metrics`
//!
//! ## Production Deployment
//!
//! ```bash
//! # Docker deployment (recommended)
//! docker run -d \\
//!   -p 5678:5678 -p 5679:5679 \\
//!   -v /data:/data \\
//!   -v /config:/config \\
//!   proximadb/proximadb:latest \\
//!   --config /config/production.toml
//!
//! # Systemd service
//! sudo systemctl start proximadb
//! sudo systemctl enable proximadb
//!
//! # Kubernetes
//! kubectl apply -f proximadb-deployment.yaml
//! ```

use clap::Parser;
use proximadb::ProximaDB;
use proximadb::core::ConfigLoader;
use proximadb::core::hardware_capabilities::initialize_hardware_capabilities;
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "proximadb-server")]
#[command(about = "ProximaDB cloud-native vector database server")]
struct Args {
    #[arg(short, long, default_value = "config/config.toml")]
    config: PathBuf,

    #[arg(short, long)]
    data_dir: Option<PathBuf>,

    #[arg(short, long)]
    port: Option<u16>,

    #[arg(long)]
    node_id: Option<String>,

    #[arg(short, long, help = "Set log level (error, warn, info, debug, trace)")]
    log_level: Option<String>,
}

/// Ensure all required directories exist based on configuration
async fn ensure_required_directories(config: &proximadb::core::Config) -> anyhow::Result<()> {
    info!("🔧 Ensuring all required directories exist...");

    // Extract base data directory from storage configuration
    let base_data_dir = config.server.data_dir.to_string_lossy();
    info!("📂 Base data directory: {}", base_data_dir);

    // Create base data directory
    tokio::fs::create_dir_all(base_data_dir.as_ref())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create base data directory: {}", e))?;

    // Create storage location directories
    for location in &config.storage.storage_locations {
        if let Some(path) = location.url.strip_prefix("file://") {
            info!("📂 Creating storage location directory: {}", path);
            tokio::fs::create_dir_all(path).await.map_err(|e| {
                anyhow::anyhow!("Failed to create storage directory {}: {}", path, e)
            })?;
        }
    }

    // Create metadata directory with required subdirectories
    let metadata_url = &config.storage.metadata_url;
    if let Some(base_path) = metadata_url.strip_prefix("file://") {
        info!("📂 Creating metadata directories at: {}", base_path);

        // Create base metadata directory
        tokio::fs::create_dir_all(base_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create metadata directory: {}", e))?;

        // Create required subdirectories
        let subdirs = ["current", "archive", "__staging"];
        for subdir in &subdirs {
            let subdir_path = format!("{}/{}", base_path, subdir);
            info!("  📁 Creating subdirectory: {}", subdir_path);
            tokio::fs::create_dir_all(&subdir_path).await.map_err(|e| {
                anyhow::anyhow!("Failed to create subdirectory {}: {}", subdir_path, e)
            })?;
        }
    }

    // SST directories are now created per-collection under storage locations
    // The SST config only contains operational parameters
    // No need to create global SST directories - collections create their own
    // directories based on storage assignments from the assignment service
    info!("✅ SST directories will be created per-collection by assignment service");

    // Log directory creation is handled by the logging framework itself

    info!("✅ All required directories created successfully");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize tracing with rolling file appender
    use tracing_appender::rolling::{RollingFileAppender, Rotation};
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

    // Create log directory if it doesn't exist
    std::fs::create_dir_all("./log").expect("Failed to create log directory");

    // Create rolling file appender (daily rotation)
    let file_appender = RollingFileAppender::new(Rotation::DAILY, "./log", "proximadb.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Create console appender for stdout - production friendly format
    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(false) // Cleaner output without module paths
        .with_line_number(false) // No line numbers in production
        .with_file(false) // No file names in production
        .with_thread_ids(false) // No thread IDs for cleaner output
        .with_thread_names(false) // No thread names
        .compact() // Use compact format for better readability
        .with_writer(std::io::stdout);

    // Create file appender layer
    let file_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_line_number(true)
        .with_file(true)
        .with_ansi(false) // No ANSI colors in file
        .with_writer(non_blocking);

    // Parse arguments first to get config path
    let args = Args::parse();

    // Load configuration first to get log level from TOML
    let mut config = ConfigLoader::load_with_defaults(args.config.to_string_lossy().as_ref())?;

    // Initialize subscriber with both console and file output - fully configurable
    // Priority: Environment variable > CLI args > TOML config > Default (INFO)
    // Default to info level - only show info, warn, error (no debug/trace)
    let log_level = std::env::var("RUST_LOG")
        .or_else(|_| args.log_level.clone().ok_or(()))
        .unwrap_or_else(|_| {
            // If config has a log level, use it, otherwise default to info
            if config.monitoring.log_level.is_empty()
                || config.monitoring.log_level == "debug"
                || config.monitoring.log_level == "trace"
            {
                // Override debug/trace with info for production
                "info".to_string()
            } else {
                config.monitoring.log_level.clone()
            }
        });

    // Create environment filter with ProximaDB-specific defaults
    // This ensures we only see info and above for proximadb modules
    let env_filter = EnvFilter::try_new(&log_level)
        .or_else(|_| EnvFilter::try_new("proximadb=info"))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    info!(
        "🚀 ProximaDB Server v{} starting",
        env!("CARGO_PKG_VERSION")
    );
    info!(
        "📊 Log level: {} (use RUST_LOG env or --log-level to change)",
        log_level
    );

    // Override with CLI arguments
    if let Some(data_dir) = args.data_dir {
        config.server.data_dir = data_dir;
    }
    if let Some(port) = args.port {
        config.server.port = port;
    }
    if let Some(node_id) = args.node_id {
        config.server.node_id = node_id;
    }

    // Initialize hardware capabilities detection early with configuration
    info!("🔧 Initializing hardware detection...");
    let hardware_config = config.hardware.clone().unwrap_or_default();
    if let Err(e) = initialize_hardware_capabilities(hardware_config) {
        warn!("⚠️ Hardware capability detection failed: {}", e);
        info!("Continuing with CPU-only mode");
    }

    info!("Starting ProximaDB server with config: {:?}", config);

    // Ensure all required directories exist
    ensure_required_directories(&config).await?;

    // Create and start the database
    let mut db = ProximaDB::new(config).await?;

    // Start the database engine
    if let Err(e) = db.start().await {
        error!("Failed to start ProximaDB: {}", e);
        return Err(e);
    }

    info!("ProximaDB server started successfully");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Received shutdown signal, stopping server...");

    // Graceful shutdown
    if let Err(e) = db.stop().await {
        error!("Error during shutdown: {}", e);
    }

    info!("ProximaDB server stopped");
    Ok(())
}

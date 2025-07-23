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

//! ProximaDB Server - Main server binary for the ProximaDB vector database

use clap::Parser;
use proximadb::compute::hardware_detection::HardwareCapabilities;
use proximadb::{ConfigLoader, ProximaDB};
use std::path::{Path, PathBuf};
use tracing::{error, info};

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
}

/// Ensure all required directories exist based on configuration
async fn ensure_required_directories(config: &proximadb::core::Config) -> anyhow::Result<()> {
    info!("🔧 Ensuring all required directories exist...");
    
    // Extract base data directory from storage configuration
    let base_data_dir = config.server.data_dir.to_string_lossy();
    info!("📂 Base data directory: {}", base_data_dir);
    
    // Create base data directory
    tokio::fs::create_dir_all(base_data_dir.as_ref()).await
        .map_err(|e| anyhow::anyhow!("Failed to create base data directory: {}", e))?;
    
    // Create storage location directories
    for location in &config.storage.storage_locations {
        if let Some(path) = location.url.strip_prefix("file://") {
            info!("📂 Creating storage location directory: {}", path);
            tokio::fs::create_dir_all(path).await
                .map_err(|e| anyhow::anyhow!("Failed to create storage directory {}: {}", path, e))?;
        }
    }
    
    // Create metadata directory with required subdirectories
    let metadata_url = &config.storage.metadata_url;
    if let Some(base_path) = metadata_url.strip_prefix("file://") {
        info!("📂 Creating metadata directories at: {}", base_path);
        
        // Create base metadata directory
        tokio::fs::create_dir_all(base_path).await
            .map_err(|e| anyhow::anyhow!("Failed to create metadata directory: {}", e))?;
            
        // Create required subdirectories
        let subdirs = ["current", "archive", "__staging"];
        for subdir in &subdirs {
            let subdir_path = format!("{}/{}", base_path, subdir);
            info!("  📁 Creating subdirectory: {}", subdir_path);
            tokio::fs::create_dir_all(&subdir_path).await
                .map_err(|e| anyhow::anyhow!("Failed to create subdirectory {}: {}", subdir_path, e))?;
        }
    }
    
    // LSM directories are now created per-collection under storage locations
    // The LSM config only contains operational parameters
    let lsm_config = &config.storage.lsm_config;
    
    info!("📂 Creating LSM WAL directory: {}", lsm_config.wal_directory);
    tokio::fs::create_dir_all(&lsm_config.wal_directory).await
        .map_err(|e| anyhow::anyhow!("Failed to create LSM WAL directory: {}", e))?;
    
    info!("📂 Creating LSM data directory: {}", lsm_config.data_directory);
    tokio::fs::create_dir_all(&lsm_config.data_directory).await
        .map_err(|e| anyhow::anyhow!("Failed to create LSM data directory: {}", e))?;
    
    // Log directory creation is handled by the logging framework itself
    
    info!("✅ All required directories created successfully");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize tracing with rolling file appender
    use tracing_appender::rolling::{RollingFileAppender, Rotation};
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    // Create log directory if it doesn't exist
    std::fs::create_dir_all("./log").expect("Failed to create log directory");

    // Create rolling file appender (daily rotation)
    let file_appender = RollingFileAppender::new(Rotation::DAILY, "./log", "proximadb.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Create console appender for stdout
    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_line_number(true)
        .with_file(true)
        .with_writer(std::io::stdout);

    // Create file appender layer
    let file_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_line_number(true)
        .with_file(true)
        .with_ansi(false) // No ANSI colors in file
        .with_writer(non_blocking);

    // Initialize subscriber with both console and file output - TRACE level for deep debugging
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .with(console_layer)
        .with(file_layer)
        .init();

    // Initialize hardware capabilities detection early to prevent crashes
    info!("🔧 Initializing hardware detection...");
    let _hardware_caps = HardwareCapabilities::initialize();

    let args = Args::parse();

    // Load configuration with default merging and cloud support
    let mut config = ConfigLoader::load_with_defaults(args.config.to_string_lossy().as_ref())?;

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

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
use std::path::PathBuf;
use tracing::{info, error};
use proximadb::{ProximaDB, Config};
use proximadb::compute::hardware_detection::HardwareCapabilities;

#[derive(Parser)]
#[command(name = "proximadb-server")]
#[command(about = "ProximaDB cloud-native vector database server")]
struct Args {
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
    
    #[arg(short, long)]
    data_dir: Option<PathBuf>,
    
    #[arg(short, long)]
    port: Option<u16>,
    
    #[arg(long)]
    node_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Initialize tracing with debug level for comprehensive logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(true)
        .with_line_number(true)
        .with_file(true)
        .init();
    
    // Initialize hardware capabilities detection early to prevent crashes
    info!("🔧 Initializing hardware detection...");
    let _hardware_caps = HardwareCapabilities::initialize();
    
    let args = Args::parse();
    
    // Load configuration
    let mut config = if args.config.exists() {
        let config_str = std::fs::read_to_string(&args.config)?;
        toml::from_str::<Config>(&config_str)?
    } else {
        info!("Configuration file not found, using defaults");
        Config::default()
    };
    
    // Override with CLI arguments
    if let Some(data_dir) = args.data_dir {
        config.server.data_dir = data_dir;
    }
    if let Some(port) = args.port {
        config.server.port = port;
        config.api.grpc_port = port + 10;
        config.api.rest_port = port;
    }
    if let Some(node_id) = args.node_id {
        config.server.node_id = node_id;
    }
    
    info!("Starting ProximaDB server with config: {:?}", config);
    
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
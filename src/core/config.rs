use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub consensus: ConsensusConfig,
    pub api: ApiConfig,
    pub monitoring: MonitoringConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub node_id: String,
    pub bind_address: String,
    pub port: u16,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub data_dirs: Vec<PathBuf>,
    pub wal_dir: PathBuf,
    pub mmap_enabled: bool,
    pub lsm_config: LsmConfig,
    pub cache_size_mb: u64,
    pub bloom_filter_bits: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LsmConfig {
    pub memtable_size_mb: u64,
    pub level_count: u8,
    pub compaction_threshold: u32,
    pub block_size_kb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusConfig {
    pub node_id: Option<u64>,
    pub cluster_peers: Vec<String>,
    pub election_timeout_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub snapshot_threshold: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub grpc_port: u16,
    pub rest_port: u16,
    pub max_request_size_mb: u64,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    pub metrics_enabled: bool,
    pub dashboard_port: u16,
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                node_id: "node-1".to_string(),
                bind_address: "0.0.0.0".to_string(),
                port: 8080,
                data_dir: PathBuf::from("./data"),
            },
            storage: StorageConfig {
                data_dirs: vec![PathBuf::from("./data/storage")],
                wal_dir: PathBuf::from("./data/wal"),
                mmap_enabled: true,
                lsm_config: LsmConfig {
                    memtable_size_mb: 64,
                    level_count: 7,
                    compaction_threshold: 4,
                    block_size_kb: 64,
                },
                cache_size_mb: 512,
                bloom_filter_bits: 10,
            },
            consensus: ConsensusConfig {
                node_id: Some(1),
                cluster_peers: vec![],
                election_timeout_ms: 5000,
                heartbeat_interval_ms: 1000,
                snapshot_threshold: 1000,
            },
            api: ApiConfig {
                grpc_port: 9090,
                rest_port: 8080,
                max_request_size_mb: 100,
                timeout_seconds: 30,
            },
            monitoring: MonitoringConfig {
                metrics_enabled: true,
                dashboard_port: 3000,
                log_level: "info".to_string(),
            },
        }
    }
}
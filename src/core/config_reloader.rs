//! Dynamic Configuration Reloading for ProximaDB
//!
//! This module provides hot configuration reloading capabilities for production
//! deployments where configuration changes need to be applied without restart.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{RwLock, broadcast, watch};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::core::config::Config;
use crate::core::config_loader::ConfigLoader;

/// Configuration change event
#[derive(Debug, Clone)]
pub struct ConfigChangeEvent {
    /// Kind of configuration change that occurred
    pub change_type: ConfigChangeType,
    /// Names of configuration sections that were modified
    pub affected_sections: Vec<String>,
    /// Previous configuration before the change
    pub old_config: Option<Config>,
    /// Updated configuration after the change
    pub new_config: Config,
    /// Wall-clock time when the change was detected
    pub timestamp: SystemTime,
}

/// Types of configuration changes
#[derive(Debug, Clone)]
pub enum ConfigChangeType {
    /// Complete configuration reload
    FullReload,
    /// Specific section updated
    SectionUpdate {
        /// Name of the configuration section that changed
        section: String,
    },
    /// Server settings changed
    ServerConfigUpdate,
    /// Storage settings changed
    StorageConfigUpdate,
    /// Cache settings changed
    CacheConfigUpdate,
}

/// Dynamic configuration reloader
pub struct ConfigReloader {
    /// Current configuration
    config: Arc<RwLock<Config>>,

    /// Configuration file path
    config_path: String,

    /// Config loader
    #[allow(dead_code)]
    loader: ConfigLoader,

    /// File modification time tracking
    last_modified: Arc<RwLock<SystemTime>>,

    /// Change notification broadcast
    change_notifier: broadcast::Sender<ConfigChangeEvent>,

    /// Reload interval
    check_interval: Duration,

    /// Reload task handle
    reload_task: Option<tokio::task::JoinHandle<()>>,

    /// Shutdown signal
    shutdown: watch::Receiver<bool>,
}

impl ConfigReloader {
    /// Create new configuration reloader
    pub async fn new(
        config_path: String,
        initial_config: Config,
        shutdown: watch::Receiver<bool>,
    ) -> Result<Self> {
        let (change_tx, _) = broadcast::channel(100);

        // Get initial file modification time
        let last_modified = tokio::fs::metadata(&config_path)
            .await.map_or_else(|_| SystemTime::now(), |m| m.modified().unwrap_or(SystemTime::now()));

        Ok(Self {
            config: Arc::new(RwLock::new(initial_config)),
            config_path,
            loader: ConfigLoader,
            last_modified: Arc::new(RwLock::new(last_modified)),
            change_notifier: change_tx,
            check_interval: Duration::from_secs(30), // Check every 30 seconds
            reload_task: None,
            shutdown,
        })
    }

    /// Start dynamic configuration reloading
    pub async fn start_reloading(&mut self) -> Result<()> {
        if self.reload_task.is_some() {
            warn!("Configuration reloading already started");
            return Ok(());
        }

        info!(
            "Starting dynamic configuration reloading for: {}",
            self.config_path
        );

        let config_path = self.config_path.clone();
        let config = self.config.clone();
        let last_modified = self.last_modified.clone();
        let change_notifier = self.change_notifier.clone();
        let check_interval = self.check_interval;
        let mut shutdown = self.shutdown.clone();

        let reload_task = tokio::spawn(async move {
            let mut interval = interval(check_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = Self::check_and_reload_config(
                            &config_path,
                            &config,
                            &last_modified,
                            &change_notifier,
                        ).await {
                            error!("Configuration reload failed: {}", e);
                        }
                    }
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            info!("Configuration reloader shutting down");
                            break;
                        }
                    }
                }
            }
        });

        self.reload_task = Some(reload_task);
        Ok(())
    }

    /// Check file modification and reload if needed
    async fn check_and_reload_config(
        config_path: &str,
        config: &Arc<RwLock<Config>>,
        last_modified: &Arc<RwLock<SystemTime>>,
        change_notifier: &broadcast::Sender<ConfigChangeEvent>,
    ) -> Result<()> {
        // Check file modification time
        let metadata = tokio::fs::metadata(config_path)
            .await
            .context("Failed to read config file metadata")?;

        let file_modified = metadata.modified().unwrap_or_else(|_| SystemTime::now());

        let last_mod_time = *last_modified.read().await;

        if file_modified <= last_mod_time {
            return Ok(()); // No changes
        }

        debug!("Configuration file changed, reloading: {}", config_path);

        // Load new configuration
        let new_config = ConfigLoader::load_with_defaults(config_path)
            .map_err(|e| anyhow::anyhow!("Failed to load new configuration: {}", e))?;

        // Store old config for comparison
        let old_config = {
            let current = config.read().await;
            current.clone()
        };

        // Determine what changed
        let affected_sections = Self::analyze_config_changes(&old_config, &new_config);
        let change_type = if affected_sections.len() > 3 {
            ConfigChangeType::FullReload
        } else if affected_sections.contains(&"server".to_string()) {
            ConfigChangeType::ServerConfigUpdate
        } else if affected_sections.contains(&"storage".to_string()) {
            ConfigChangeType::StorageConfigUpdate
        } else if affected_sections.contains(&"cache".to_string()) {
            ConfigChangeType::CacheConfigUpdate
        } else {
            ConfigChangeType::SectionUpdate {
                section: affected_sections.first().cloned().unwrap_or_default(),
            }
        };

        // Update configuration
        {
            let mut current = config.write().await;
            *current = new_config.clone();
        }

        // Update modification time
        {
            let mut last_mod = last_modified.write().await;
            *last_mod = file_modified;
        }

        // Notify subscribers of configuration change
        let change_event = ConfigChangeEvent {
            change_type: change_type.clone(),
            affected_sections,
            old_config: Some(old_config),
            new_config,
            timestamp: SystemTime::now(),
        };

        if let Err(e) = change_notifier.send(change_event) {
            warn!("Failed to broadcast configuration change: {}", e);
        }

        info!("Configuration reloaded successfully: {:?}", change_type);
        Ok(())
    }

    /// Analyze differences between old and new configuration
    fn analyze_config_changes(old_config: &Config, new_config: &Config) -> Vec<String> {
        let mut changed_sections = Vec::new();

        // Compare server settings
        if old_config.server != new_config.server {
            changed_sections.push("server".to_string());
        }

        // Compare storage settings (simplified comparison)
        if old_config.storage.metadata_url != new_config.storage.metadata_url {
            changed_sections.push("storage".to_string());
        }

        // In a full implementation, you'd compare all config sections
        // This is a simplified version for demonstration

        changed_sections
    }

    /// Subscribe to configuration changes
    pub fn subscribe_to_changes(&self) -> broadcast::Receiver<ConfigChangeEvent> {
        self.change_notifier.subscribe()
    }

    /// Get current configuration
    pub async fn current_config(&self) -> Config {
        self.config.read().await.clone()
    }

    /// Manually trigger configuration reload
    pub async fn reload_now(&self) -> Result<()> {
        Self::check_and_reload_config(
            &self.config_path,
            &self.config,
            &self.last_modified,
            &self.change_notifier,
        )
        .await
    }

    /// Stop configuration reloading
    pub async fn stop(&mut self) {
        if let Some(task) = self.reload_task.take() {
            task.abort();
            info!("Configuration reloader stopped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_config_change_detection() {
        let old_config = Config::default();
        let mut new_config = Config::default();

        // Modify server settings
        new_config.server.port = 6789;

        let changes = ConfigReloader::analyze_config_changes(&old_config, &new_config);
        assert!(changes.contains(&"server".to_string()));
    }

    #[test]
    fn test_config_change_event() {
        let event = ConfigChangeEvent {
            change_type: ConfigChangeType::ServerConfigUpdate,
            affected_sections: vec!["server".to_string()],
            old_config: None,
            new_config: Config::default(),
            timestamp: SystemTime::now(),
        };

        assert!(matches!(
            event.change_type,
            ConfigChangeType::ServerConfigUpdate
        ));
        assert_eq!(event.affected_sections[0], "server");
    }
}

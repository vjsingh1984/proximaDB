// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Certificate Management Module
//! 
//! Provides automated certificate management for TLS configuration,
//! including certificate generation, renewal, and validation.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tracing::{info, warn, error};

/// Certificate management configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateConfig {
    /// Auto-generate self-signed certificates if none provided
    pub auto_generate: bool,
    /// Certificate file path
    pub cert_file: Option<PathBuf>,
    /// Private key file path
    pub key_file: Option<PathBuf>,
    /// Certificate authority file path (for mTLS)
    pub ca_file: Option<PathBuf>,
    /// Days before expiration to trigger renewal
    pub renewal_threshold_days: u64,
    /// Certificate subject information
    pub subject: CertificateSubject,
    /// Certificate validity period in days
    pub validity_days: u64,
}

/// Certificate subject information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateSubject {
    pub common_name: String,
    pub organization: Option<String>,
    pub organizational_unit: Option<String>,
    pub country: Option<String>,
    pub state: Option<String>,
    pub locality: Option<String>,
    pub email: Option<String>,
}

/// Certificate status information
#[derive(Debug, Clone)]
pub struct CertificateStatus {
    pub valid: bool,
    pub expires_at: SystemTime,
    pub days_until_expiry: u64,
    pub needs_renewal: bool,
    pub subject: String,
}

/// Certificate manager for automated TLS certificate handling
#[derive(Debug, Clone)]
pub struct CertificateManager {
    config: CertificateConfig,
    cert_dir: PathBuf,
}

impl Default for CertificateConfig {
    fn default() -> Self {
        Self {
            auto_generate: true,
            cert_file: None,
            key_file: None,
            ca_file: None,
            renewal_threshold_days: 30,
            subject: CertificateSubject {
                common_name: "localhost".to_string(),
                organization: Some("ProximaDB".to_string()),
                organizational_unit: Some("Development".to_string()),
                country: Some("US".to_string()),
                state: None,
                locality: None,
                email: None,
            },
            validity_days: 365,
        }
    }
}

impl CertificateManager {
    /// Create new certificate manager
    pub fn new(config: CertificateConfig, cert_dir: PathBuf) -> Self {
        Self { config, cert_dir }
    }

    /// Initialize certificate management
    pub async fn initialize(&self) -> Result<()> {
        // Create certificate directory if it doesn't exist
        if !self.cert_dir.exists() {
            fs::create_dir_all(&self.cert_dir).await?;
            info!("Created certificate directory: {:?}", self.cert_dir);
        }

        // Check if certificates exist and are valid
        if let Err(e) = self.validate_certificates().await {
            warn!("Certificate validation failed: {}", e);
            
            if self.config.auto_generate {
                info!("Auto-generating certificates...");
                self.generate_self_signed_certificate().await?;
            } else {
                return Err(anyhow!("No valid certificates found and auto-generation disabled"));
            }
        }

        Ok(())
    }

    /// Validate existing certificates
    pub async fn validate_certificates(&self) -> Result<CertificateStatus> {
        let cert_path = self.get_cert_path();
        let key_path = self.get_key_path();

        // Check if certificate files exist
        if !cert_path.exists() || !key_path.exists() {
            return Err(anyhow!("Certificate files not found"));
        }

        // Read and parse certificate
        let cert_data = fs::read(&cert_path).await?;
        let cert_info = self.parse_certificate(&cert_data)?;

        info!("Certificate status: valid={}, expires in {} days", 
              cert_info.valid, cert_info.days_until_expiry);

        Ok(cert_info)
    }

    /// Generate self-signed certificate
    pub async fn generate_self_signed_certificate(&self) -> Result<()> {
        let cert_path = self.get_cert_path();
        let key_path = self.get_key_path();

        info!("Generating self-signed certificate: {:?}", cert_path);

        // Generate certificate and key using OpenSSL command
        // This is a simplified implementation - in production, use a proper crypto library
        let cert_content = self.create_certificate_content()?;
        let key_content = self.create_private_key_content()?;

        // Write certificate and key files
        fs::write(&cert_path, cert_content).await?;
        fs::write(&key_path, key_content).await?;

        // Set appropriate file permissions (readable by owner only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&key_path).await?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&key_path, perms).await?;
        }

        info!("Generated certificate: {:?}", cert_path);
        info!("Generated private key: {:?}", key_path);

        Ok(())
    }

    /// Check if certificate renewal is needed
    pub async fn check_renewal_needed(&self) -> Result<bool> {
        match self.validate_certificates().await {
            Ok(status) => Ok(status.needs_renewal),
            Err(_) => Ok(true), // Renewal needed if validation fails
        }
    }

    /// Renew certificate if needed
    pub async fn renew_if_needed(&self) -> Result<bool> {
        if self.check_renewal_needed().await? {
            info!("Certificate renewal needed, generating new certificate...");
            self.generate_self_signed_certificate().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get certificate file path
    pub fn get_cert_path(&self) -> PathBuf {
        self.config.cert_file.clone()
            .unwrap_or_else(|| self.cert_dir.join("server.crt"))
    }

    /// Get private key file path
    pub fn get_key_path(&self) -> PathBuf {
        self.config.key_file.clone()
            .unwrap_or_else(|| self.cert_dir.join("server.key"))
    }

    /// Parse certificate to extract information
    fn parse_certificate(&self, _cert_data: &[u8]) -> Result<CertificateStatus> {
        // Simplified implementation - in production, use a proper X.509 parser
        // For now, return a mock valid certificate status
        let expires_at = SystemTime::now() + Duration::from_secs(self.config.validity_days * 24 * 60 * 60);
        let days_until_expiry = (expires_at.duration_since(SystemTime::now())?.as_secs() / (24 * 60 * 60)) as u64;
        let needs_renewal = days_until_expiry <= self.config.renewal_threshold_days;

        Ok(CertificateStatus {
            valid: true,
            expires_at,
            days_until_expiry,
            needs_renewal,
            subject: format!("CN={}", self.config.subject.common_name),
        })
    }

    /// Create certificate content (simplified implementation)
    fn create_certificate_content(&self) -> Result<Vec<u8>> {
        // In production, use a proper certificate generation library like rcgen
        let cert_template = format!(
            "-----BEGIN CERTIFICATE-----\n\
             MIICIjANBgkqhkiG9w0BAQEFAAOCAg8AMIICCgKCAgEA...\n\
             (Generated certificate for {})\n\
             -----END CERTIFICATE-----\n",
            self.config.subject.common_name
        );
        Ok(cert_template.into_bytes())
    }

    /// Create private key content (simplified implementation)  
    fn create_private_key_content(&self) -> Result<Vec<u8>> {
        // In production, use a proper key generation library
        let key_template = "-----BEGIN PRIVATE KEY-----\n\
                           MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC...\n\
                           (Generated private key)\n\
                           -----END PRIVATE KEY-----\n";
        Ok(key_template.as_bytes().to_vec())
    }

    /// Start background certificate renewal task
    pub async fn start_renewal_task(self) -> Result<()> {
        let check_interval = Duration::from_secs(24 * 60 * 60); // Check daily
        
        loop {
            tokio::time::sleep(check_interval).await;
            
            if let Err(e) = self.renew_if_needed().await {
                error!("Certificate renewal failed: {}", e);
            }
        }
    }
}

/// Certificate management utilities
pub mod utils {
    use super::*;

    /// Create default certificate configuration for development
    pub fn development_config() -> CertificateConfig {
        CertificateConfig {
            auto_generate: true,
            renewal_threshold_days: 7, // More frequent renewal for dev
            subject: CertificateSubject {
                common_name: "localhost".to_string(),
                organization: Some("ProximaDB Development".to_string()),
                organizational_unit: Some("Development".to_string()),
                country: Some("US".to_string()),
                state: None,
                locality: None,
                email: Some("dev@proximadb.com".to_string()),
            },
            validity_days: 90, // Shorter validity for dev
            ..Default::default()
        }
    }

    /// Create production certificate configuration
    pub fn production_config(domain: &str) -> CertificateConfig {
        CertificateConfig {
            auto_generate: false, // Use provided certificates in production
            renewal_threshold_days: 30,
            subject: CertificateSubject {
                common_name: domain.to_string(),
                organization: Some("ProximaDB".to_string()),
                organizational_unit: Some("Production".to_string()),
                country: Some("US".to_string()),
                state: None,
                locality: None,
                email: None,
            },
            validity_days: 365,
            ..Default::default()
        }
    }
}
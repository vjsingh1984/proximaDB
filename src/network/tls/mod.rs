// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # TLS Configuration Module
//! 
//! Provides TLS configuration and certificate management for ProximaDB servers.

pub mod certificate_manager;

pub use certificate_manager::{
    CertificateConfig, CertificateManager, CertificateStatus, CertificateSubject
};

use anyhow::Result;
use std::path::PathBuf;

/// TLS configuration for ProximaDB
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Certificate manager
    pub certificate_manager: Option<CertificateManager>,
    /// Enable TLS
    pub enabled: bool,
    /// Require client certificates (mTLS)
    pub require_client_certs: bool,
}

impl TlsConfig {
    /// Create new TLS configuration
    pub fn new(enabled: bool) -> Self {
        Self {
            certificate_manager: None,
            enabled,
            require_client_certs: false,
        }
    }

    /// Enable automatic certificate management
    pub fn with_auto_certificates(mut self, cert_dir: PathBuf) -> Self {
        let cert_config = CertificateConfig::default();
        self.certificate_manager = Some(CertificateManager::new(cert_config, cert_dir));
        self
    }

    /// Enable mTLS (mutual TLS)
    pub fn with_mtls(mut self) -> Self {
        self.require_client_certs = true;
        self
    }

    /// Initialize TLS configuration
    pub async fn initialize(&self) -> Result<()> {
        if let Some(ref cert_manager) = self.certificate_manager {
            cert_manager.initialize().await?;
        }
        Ok(())
    }

    /// Get certificate paths
    pub fn get_certificate_paths(&self) -> Option<(PathBuf, PathBuf)> {
        self.certificate_manager.as_ref().map(|cm| {
            (cm.get_cert_path(), cm.get_key_path())
        })
    }
}
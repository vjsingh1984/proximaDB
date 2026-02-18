// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Certificate Management Module
//!
//! Provides automated certificate management for TLS configuration,
//! including certificate generation, renewal, and validation using
//! real cryptographic implementations.
//!
//! ## Features
//!
//! - Self-signed certificate generation with rcgen
//! - Certificate Authority (CA) generation for mTLS
//! - Client certificate generation signed by CA
//! - Certificate parsing and validation with x509-parser
//! - Automatic renewal based on expiration threshold
//!
//! ## Usage
//!
//! ```rust,no_run
//! use proximadb::network::tls::{CertificateManager, CertificateConfig};
//!
//! // Create certificate manager
//! let config = CertificateConfig::default();
//! let manager = CertificateManager::new(config, "/tmp/certs".into());
//!
//! // Generate self-signed certificate
//! let cert = manager.generate_self_signed().unwrap();
//!
//! // Parse and validate certificate
//! let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();
//! println!("Subject CN: {:?}", parsed.subject_cn);
//! ```

use anyhow::{Result, anyhow};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tokio::fs;
use tracing::{error, info, warn};
use x509_parser::prelude::*;

// Use external time crate types with explicit path to avoid conflict with x509_parser::prelude::*
type OffsetDateTime = ::time::OffsetDateTime;
type TimeDuration = ::time::Duration;

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
    /// Subject Alternative Names (SANs)
    pub san_dns_names: Vec<String>,
    /// IP addresses for SAN
    pub san_ip_addresses: Vec<String>,
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
    pub issuer: String,
    pub serial: String,
    pub not_before: SystemTime,
}

/// Generated certificate and key pair
#[derive(Debug, Clone)]
pub struct GeneratedCertificate {
    /// PEM-encoded certificate
    pub cert_pem: String,
    /// PEM-encoded private key
    pub key_pem: String,
}

/// Parsed certificate information
#[derive(Debug, Clone)]
pub struct ParsedCertificate {
    /// Subject Common Name
    pub subject_cn: Option<String>,
    /// Issuer Common Name
    pub issuer_cn: Option<String>,
    /// Certificate not valid before
    pub not_before: SystemTime,
    /// Certificate not valid after
    pub not_after: SystemTime,
    /// Certificate serial number
    pub serial: String,
    /// Subject Alternative Names (DNS)
    pub san_dns_names: Vec<String>,
    /// Is this a CA certificate
    pub is_ca: bool,
    /// Key usage flags
    pub key_usage: Vec<String>,
}

/// TLS-related errors
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("Certificate generation failed: {0}")]
    CertificateGeneration(String),
    #[error("Certificate parsing failed: {0}")]
    CertificateParse(String),
    #[error("Certificate validation failed: {0}")]
    CertificateValidation(String),
    #[error("Key generation failed: {0}")]
    KeyGeneration(String),
    #[error("File I/O error: {0}")]
    FileIO(String),
    #[error("Certificate expired")]
    CertificateExpired,
    #[error("Certificate not yet valid")]
    CertificateNotYetValid,
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
            san_dns_names: vec!["localhost".to_string()],
            san_ip_addresses: vec!["127.0.0.1".to_string(), "::1".to_string()],
        }
    }
}

impl Default for CertificateSubject {
    fn default() -> Self {
        Self {
            common_name: "localhost".to_string(),
            organization: Some("ProximaDB".to_string()),
            organizational_unit: Some("Development".to_string()),
            country: Some("US".to_string()),
            state: None,
            locality: None,
            email: None,
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
                self.generate_and_save_certificates().await?;
            } else {
                return Err(anyhow!(
                    "No valid certificates found and auto-generation disabled"
                ));
            }
        }

        Ok(())
    }

    /// Generate a self-signed certificate for development/testing
    pub fn generate_self_signed(&self) -> Result<GeneratedCertificate, TlsError> {
        let mut params = CertificateParams::default();

        // Set distinguished name
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, &self.config.subject.common_name);
        if let Some(ref org) = self.config.subject.organization {
            dn.push(DnType::OrganizationName, org);
        }
        if let Some(ref ou) = self.config.subject.organizational_unit {
            dn.push(DnType::OrganizationalUnitName, ou);
        }
        if let Some(ref country) = self.config.subject.country {
            dn.push(DnType::CountryName, country);
        }
        if let Some(ref state) = self.config.subject.state {
            dn.push(DnType::StateOrProvinceName, state);
        }
        if let Some(ref locality) = self.config.subject.locality {
            dn.push(DnType::LocalityName, locality);
        }
        params.distinguished_name = dn;

        // Set validity period
        let now = OffsetDateTime::now_utc();
        params.not_before = now;
        params.not_after = now + TimeDuration::days(self.config.validity_days as i64);

        // Add Subject Alternative Names
        let mut san_list = Vec::new();
        for dns_name in &self.config.san_dns_names {
            san_list.push(SanType::DnsName(dns_name.clone()));
        }
        for ip_str in &self.config.san_ip_addresses {
            if let Ok(ip) = ip_str.parse::<std::net::IpAddr>() {
                san_list.push(SanType::IpAddress(ip));
            }
        }
        params.subject_alt_names = san_list;

        // Key usage for server certificate
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];

        // Use ECDSA P-256 for modern security
        params.alg = &PKCS_ECDSA_P256_SHA256;

        // Generate key pair
        let key_pair = KeyPair::generate(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| TlsError::KeyGeneration(e.to_string()))?;
        params.key_pair = Some(key_pair);

        // Generate certificate
        let cert = rcgen::Certificate::from_params(params)
            .map_err(|e| TlsError::CertificateGeneration(e.to_string()))?;

        Ok(GeneratedCertificate {
            cert_pem: cert
                .serialize_pem()
                .map_err(|e| TlsError::CertificateGeneration(e.to_string()))?,
            key_pem: cert.serialize_private_key_pem(),
        })
    }

    /// Generate a Certificate Authority (CA) certificate for mTLS
    pub fn generate_ca(&self) -> Result<GeneratedCertificate, TlsError> {
        let mut params = CertificateParams::default();

        // Set distinguished name for CA
        let mut dn = DistinguishedName::new();
        dn.push(
            DnType::CommonName,
            format!("{} CA", self.config.subject.common_name),
        );
        if let Some(ref org) = self.config.subject.organization {
            dn.push(DnType::OrganizationName, org);
        }
        if let Some(ref ou) = self.config.subject.organizational_unit {
            dn.push(DnType::OrganizationalUnitName, format!("{} CA", ou));
        }
        if let Some(ref country) = self.config.subject.country {
            dn.push(DnType::CountryName, country);
        }
        params.distinguished_name = dn;

        // Set validity period (CA certs typically last longer)
        let now = OffsetDateTime::now_utc();
        params.not_before = now;
        params.not_after = now + TimeDuration::days((self.config.validity_days * 3) as i64);

        // Mark as CA
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];

        // Use ECDSA P-256
        params.alg = &PKCS_ECDSA_P256_SHA256;

        // Generate key pair
        let key_pair = KeyPair::generate(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| TlsError::KeyGeneration(e.to_string()))?;
        params.key_pair = Some(key_pair);

        // Generate certificate
        let cert = rcgen::Certificate::from_params(params)
            .map_err(|e| TlsError::CertificateGeneration(e.to_string()))?;

        Ok(GeneratedCertificate {
            cert_pem: cert
                .serialize_pem()
                .map_err(|e| TlsError::CertificateGeneration(e.to_string()))?,
            key_pem: cert.serialize_private_key_pem(),
        })
    }

    /// Generate a client certificate signed by a CA for mTLS
    ///
    /// This generates a client certificate that can be used for mTLS authentication.
    /// The certificate is signed by the CA provided (ca_cert_pem and ca_key_pem).
    pub fn generate_client_cert(
        &self,
        client_cn: &str,
        ca_cert_pem: &str,
        ca_key_pem: &str,
    ) -> Result<GeneratedCertificate, TlsError> {
        // Parse CA key
        let ca_key_pair = KeyPair::from_pem(ca_key_pem)
            .map_err(|e| TlsError::CertificateParse(format!("CA key parse error: {}", e)))?;

        // Parse CA certificate to get issuer info
        let (_, pem_block) = parse_x509_pem(ca_cert_pem.as_bytes())
            .map_err(|e| TlsError::CertificateParse(format!("CA cert PEM parse error: {:?}", e)))?;
        let (_, ca_x509) = X509Certificate::from_der(&pem_block.contents)
            .map_err(|e| TlsError::CertificateParse(format!("CA cert DER parse error: {:?}", e)))?;

        // Extract CA issuer CN for the client cert's issuer field
        let ca_cn = ca_x509
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .unwrap_or("CA");

        // Create CA certificate params for signing
        let mut ca_params = CertificateParams::default();
        let mut ca_dn = DistinguishedName::new();
        ca_dn.push(DnType::CommonName, ca_cn);
        ca_params.distinguished_name = ca_dn;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        ca_params.alg = &PKCS_ECDSA_P256_SHA256;
        ca_params.key_pair = Some(ca_key_pair);

        let ca_cert = rcgen::Certificate::from_params(ca_params).map_err(|e| {
            TlsError::CertificateGeneration(format!("CA cert generation error: {}", e))
        })?;

        // Create client certificate parameters
        let mut params = CertificateParams::default();

        // Set distinguished name for client
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, client_cn);
        if let Some(ref org) = self.config.subject.organization {
            dn.push(DnType::OrganizationName, org);
        }
        params.distinguished_name = dn;

        // Set validity period
        let now = OffsetDateTime::now_utc();
        params.not_before = now;
        params.not_after = now + TimeDuration::days(self.config.validity_days as i64);

        // Key usage for client certificate
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];

        // Use ECDSA P-256
        params.alg = &PKCS_ECDSA_P256_SHA256;

        // Generate key pair for client
        let key_pair = KeyPair::generate(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| TlsError::KeyGeneration(e.to_string()))?;
        params.key_pair = Some(key_pair);

        // Generate certificate signed by CA
        let client_cert = rcgen::Certificate::from_params(params)
            .map_err(|e| TlsError::CertificateGeneration(e.to_string()))?;

        Ok(GeneratedCertificate {
            cert_pem: client_cert
                .serialize_pem_with_signer(&ca_cert)
                .map_err(|e| TlsError::CertificateGeneration(e.to_string()))?,
            key_pem: client_cert.serialize_private_key_pem(),
        })
    }

    /// Parse and validate a certificate from PEM
    pub fn parse_certificate(&self, pem: &[u8]) -> Result<ParsedCertificate, TlsError> {
        let (_, pem_block) = parse_x509_pem(pem)
            .map_err(|e| TlsError::CertificateParse(format!("PEM parse error: {:?}", e)))?;

        let (_, cert) = X509Certificate::from_der(&pem_block.contents)
            .map_err(|e| TlsError::CertificateParse(format!("DER parse error: {:?}", e)))?;

        // Extract Subject CN
        let subject_cn = cert
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .map(|s| s.to_string());

        // Extract Issuer CN
        let issuer_cn = cert
            .issuer()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .map(|s| s.to_string());

        // Convert validity times
        let not_before = asn1_time_to_system_time(&cert.validity().not_before)?;
        let not_after = asn1_time_to_system_time(&cert.validity().not_after)?;

        // Extract SANs
        let san_dns_names = cert
            .subject_alternative_name()
            .ok()
            .flatten()
            .map(|san| {
                san.value
                    .general_names
                    .iter()
                    .filter_map(|name| match name {
                        GeneralName::DNSName(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Check if CA
        let is_ca = cert
            .basic_constraints()
            .ok()
            .flatten()
            .map(|bc| bc.value.ca)
            .unwrap_or(false);

        // Extract key usage
        let key_usage = cert
            .key_usage()
            .ok()
            .flatten()
            .map(|ku| {
                let mut usages = Vec::new();
                let flags = ku.value;
                if flags.digital_signature() {
                    usages.push("digitalSignature".to_string());
                }
                if flags.key_encipherment() {
                    usages.push("keyEncipherment".to_string());
                }
                if flags.key_cert_sign() {
                    usages.push("keyCertSign".to_string());
                }
                if flags.crl_sign() {
                    usages.push("cRLSign".to_string());
                }
                usages
            })
            .unwrap_or_default();

        Ok(ParsedCertificate {
            subject_cn,
            issuer_cn,
            not_before,
            not_after,
            serial: cert.serial.to_string(),
            san_dns_names,
            is_ca,
            key_usage,
        })
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
        let parsed = self
            .parse_certificate(&cert_data)
            .map_err(|e| anyhow!("Certificate parse error: {}", e))?;

        // Check validity
        let now = SystemTime::now();
        if now < parsed.not_before {
            return Err(anyhow!("Certificate is not yet valid"));
        }
        if now > parsed.not_after {
            return Err(anyhow!("Certificate has expired"));
        }

        // Calculate days until expiry
        let duration_until_expiry = parsed
            .not_after
            .duration_since(now)
            .unwrap_or(Duration::ZERO);
        let days_until_expiry = duration_until_expiry.as_secs() / (24 * 60 * 60);
        let needs_renewal = days_until_expiry <= self.config.renewal_threshold_days;

        info!(
            "Certificate status: valid=true, expires in {} days, needs_renewal={}",
            days_until_expiry, needs_renewal
        );

        Ok(CertificateStatus {
            valid: true,
            expires_at: parsed.not_after,
            days_until_expiry,
            needs_renewal,
            subject: parsed.subject_cn.unwrap_or_default(),
            issuer: parsed.issuer_cn.unwrap_or_default(),
            serial: parsed.serial,
            not_before: parsed.not_before,
        })
    }

    /// Generate and save certificates to files
    pub async fn generate_and_save_certificates(&self) -> Result<()> {
        let cert_path = self.get_cert_path();
        let key_path = self.get_key_path();

        info!("Generating self-signed certificate: {:?}", cert_path);

        let generated = self
            .generate_self_signed()
            .map_err(|e| anyhow!("Certificate generation failed: {}", e))?;

        // Write certificate and key files
        fs::write(&cert_path, generated.cert_pem.as_bytes()).await?;
        fs::write(&key_path, generated.key_pem.as_bytes()).await?;

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

    /// Generate and save CA certificates for mTLS
    pub async fn generate_and_save_ca(&self) -> Result<()> {
        let ca_cert_path = self.cert_dir.join("ca.crt");
        let ca_key_path = self.cert_dir.join("ca.key");

        info!("Generating CA certificate: {:?}", ca_cert_path);

        let generated = self
            .generate_ca()
            .map_err(|e| anyhow!("CA generation failed: {}", e))?;

        // Write CA certificate and key files
        fs::write(&ca_cert_path, generated.cert_pem.as_bytes()).await?;
        fs::write(&ca_key_path, generated.key_pem.as_bytes()).await?;

        // Set appropriate file permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&ca_key_path).await?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&ca_key_path, perms).await?;
        }

        info!("Generated CA certificate: {:?}", ca_cert_path);
        info!("Generated CA private key: {:?}", ca_key_path);

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
            self.generate_and_save_certificates().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get certificate file path
    pub fn get_cert_path(&self) -> PathBuf {
        self.config
            .cert_file
            .clone()
            .unwrap_or_else(|| self.cert_dir.join("server.crt"))
    }

    /// Get private key file path
    pub fn get_key_path(&self) -> PathBuf {
        self.config
            .key_file
            .clone()
            .unwrap_or_else(|| self.cert_dir.join("server.key"))
    }

    /// Get CA certificate file path
    pub fn get_ca_path(&self) -> PathBuf {
        self.config
            .ca_file
            .clone()
            .unwrap_or_else(|| self.cert_dir.join("ca.crt"))
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

    /// Get configuration reference
    pub fn config(&self) -> &CertificateConfig {
        &self.config
    }
}

/// Convert ASN.1 time to SystemTime
fn asn1_time_to_system_time(time: &ASN1Time) -> Result<SystemTime, TlsError> {
    // Get the timestamp as seconds since epoch
    let datetime = time.to_datetime();
    let timestamp = datetime.unix_timestamp();

    if timestamp >= 0 {
        Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp as u64))
    } else {
        Err(TlsError::CertificateParse(
            "Invalid certificate time".to_string(),
        ))
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
            san_dns_names: vec!["localhost".to_string(), "*.localhost".to_string()],
            san_ip_addresses: vec!["127.0.0.1".to_string(), "::1".to_string()],
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
            san_dns_names: vec![domain.to_string(), format!("*.{}", domain)],
            san_ip_addresses: vec![],
            ..Default::default()
        }
    }

    /// Load certificate chain from PEM file
    pub fn load_certs_from_pem(pem_data: &[u8]) -> Result<Vec<rustls::Certificate>, TlsError> {
        let mut reader = std::io::BufReader::new(pem_data);
        rustls_pemfile::certs(&mut reader)
            .map_err(|e| TlsError::CertificateParse(e.to_string()))?
            .into_iter()
            .map(|cert| Ok(rustls::Certificate(cert)))
            .collect()
    }

    /// Load private key from PEM file
    pub fn load_private_key_from_pem(pem_data: &[u8]) -> Result<rustls::PrivateKey, TlsError> {
        let mut reader = std::io::BufReader::new(pem_data);

        // Try to load as PKCS#8 first
        let keys = rustls_pemfile::pkcs8_private_keys(&mut reader)
            .map_err(|e| TlsError::CertificateParse(e.to_string()))?;
        if !keys.is_empty() {
            return Ok(rustls::PrivateKey(keys[0].clone()));
        }

        // Reset reader and try EC keys
        let mut reader = std::io::BufReader::new(pem_data);
        let keys = rustls_pemfile::ec_private_keys(&mut reader)
            .map_err(|e| TlsError::CertificateParse(e.to_string()))?;
        if !keys.is_empty() {
            return Ok(rustls::PrivateKey(keys[0].clone()));
        }

        // Reset reader and try RSA keys
        let mut reader = std::io::BufReader::new(pem_data);
        let keys = rustls_pemfile::rsa_private_keys(&mut reader)
            .map_err(|e| TlsError::CertificateParse(e.to_string()))?;
        if !keys.is_empty() {
            return Ok(rustls::PrivateKey(keys[0].clone()));
        }

        Err(TlsError::CertificateParse(
            "No private key found in PEM data".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config() -> CertificateConfig {
        CertificateConfig {
            subject: CertificateSubject {
                common_name: "test.local".to_string(),
                organization: Some("Test Org".to_string()),
                organizational_unit: Some("Test Unit".to_string()),
                country: Some("US".to_string()),
                state: Some("CA".to_string()),
                locality: Some("San Francisco".to_string()),
                email: Some("test@test.local".to_string()),
            },
            validity_days: 365,
            san_dns_names: vec!["test.local".to_string(), "localhost".to_string()],
            san_ip_addresses: vec!["127.0.0.1".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn test_generate_self_signed_certificate() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CertificateManager::new(test_config(), temp_dir.path().to_path_buf());

        let cert = manager.generate_self_signed().unwrap();
        assert!(cert.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(cert.cert_pem.contains("END CERTIFICATE"));
        assert!(cert.key_pem.contains("BEGIN PRIVATE KEY"));
        assert!(cert.key_pem.contains("END PRIVATE KEY"));
    }

    #[test]
    fn test_parse_certificate() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CertificateManager::new(test_config(), temp_dir.path().to_path_buf());

        let cert = manager.generate_self_signed().unwrap();
        let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

        assert_eq!(parsed.subject_cn, Some("test.local".to_string()));
        assert_eq!(parsed.issuer_cn, Some("test.local".to_string())); // Self-signed
        assert!(!parsed.is_ca);
        assert!(parsed.key_usage.contains(&"digitalSignature".to_string()));
    }

    #[test]
    fn test_certificate_validity_period() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = test_config();
        config.validity_days = 30;
        let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

        let cert = manager.generate_self_signed().unwrap();
        let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

        let now = SystemTime::now();
        assert!(parsed.not_before <= now);
        assert!(parsed.not_after > now);

        // Should be valid for approximately 30 days
        let duration = parsed.not_after.duration_since(parsed.not_before).unwrap();
        let days = duration.as_secs() / (24 * 60 * 60);
        assert!(days >= 29 && days <= 31);
    }

    #[test]
    fn test_generate_ca_certificate() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CertificateManager::new(test_config(), temp_dir.path().to_path_buf());

        let ca = manager.generate_ca().unwrap();
        let parsed = manager.parse_certificate(ca.cert_pem.as_bytes()).unwrap();

        assert!(parsed.subject_cn.unwrap().contains("CA"));
        assert!(parsed.is_ca);
        assert!(parsed.key_usage.contains(&"keyCertSign".to_string()));
    }

    #[test]
    fn test_generate_client_certificate() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CertificateManager::new(test_config(), temp_dir.path().to_path_buf());

        // Generate CA first
        let ca = manager.generate_ca().unwrap();

        // Generate client cert signed by CA
        let client = manager
            .generate_client_cert("client1.test.local", &ca.cert_pem, &ca.key_pem)
            .unwrap();
        let parsed = manager
            .parse_certificate(client.cert_pem.as_bytes())
            .unwrap();

        assert_eq!(parsed.subject_cn, Some("client1.test.local".to_string()));
        assert!(!parsed.is_ca);
        // Issuer should be the CA
        assert!(parsed.issuer_cn.unwrap().contains("CA"));
    }

    #[test]
    fn test_san_dns_names() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CertificateManager::new(test_config(), temp_dir.path().to_path_buf());

        let cert = manager.generate_self_signed().unwrap();
        let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

        assert!(parsed.san_dns_names.contains(&"test.local".to_string()));
        assert!(parsed.san_dns_names.contains(&"localhost".to_string()));
    }

    #[tokio::test]
    async fn test_generate_and_save_certificates() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CertificateManager::new(test_config(), temp_dir.path().to_path_buf());

        manager.generate_and_save_certificates().await.unwrap();

        assert!(manager.get_cert_path().exists());
        assert!(manager.get_key_path().exists());

        // Verify permissions on key file (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(manager.get_key_path())
                .unwrap()
                .permissions();
            assert_eq!(perms.mode() & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn test_validate_certificates() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CertificateManager::new(test_config(), temp_dir.path().to_path_buf());

        // Generate certificates
        manager.generate_and_save_certificates().await.unwrap();

        // Validate
        let status = manager.validate_certificates().await.unwrap();
        assert!(status.valid);
        assert!(status.days_until_expiry > 360); // Should be ~365 days
        assert!(!status.needs_renewal);
        assert_eq!(status.subject, "test.local");
    }

    #[tokio::test]
    async fn test_renewal_check() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = test_config();
        config.renewal_threshold_days = 400; // Higher than validity
        let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

        manager.generate_and_save_certificates().await.unwrap();

        // With threshold higher than validity, should need renewal
        let status = manager.validate_certificates().await.unwrap();
        assert!(status.needs_renewal);
    }

    #[test]
    fn test_load_certs_from_pem() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CertificateManager::new(test_config(), temp_dir.path().to_path_buf());

        let cert = manager.generate_self_signed().unwrap();
        let certs = utils::load_certs_from_pem(cert.cert_pem.as_bytes()).unwrap();

        assert_eq!(certs.len(), 1);
    }

    #[test]
    fn test_load_private_key_from_pem() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CertificateManager::new(test_config(), temp_dir.path().to_path_buf());

        let cert = manager.generate_self_signed().unwrap();
        let key = utils::load_private_key_from_pem(cert.key_pem.as_bytes()).unwrap();

        assert!(!key.0.is_empty());
    }

    #[test]
    fn test_development_config() {
        let config = utils::development_config();
        assert!(config.auto_generate);
        assert_eq!(config.renewal_threshold_days, 7);
        assert_eq!(config.validity_days, 90);
    }

    #[test]
    fn test_production_config() {
        let config = utils::production_config("example.com");
        assert!(!config.auto_generate);
        assert_eq!(config.subject.common_name, "example.com");
        assert!(config.san_dns_names.contains(&"example.com".to_string()));
    }
}

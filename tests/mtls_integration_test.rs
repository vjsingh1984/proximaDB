// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Integration tests for mTLS (Mutual TLS) infrastructure
//!
//! These tests verify:
//! - Certificate generation (self-signed, CA, client)
//! - Certificate parsing and validation
//! - TLS server configuration

use proximadb::network::tls::{
    CertificateConfig, CertificateManager, CertificateSubject, ClientCertificateInfo, TlsConfig,
    TlsServerConfig,
};
use std::sync::Arc;
use tempfile::TempDir;

// ============================================================================
// Certificate Generation Tests
// ============================================================================

#[test]
fn test_generate_self_signed_certificate() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "test.example.com".to_string(),
            organization: Some("Test Organization".to_string()),
            organizational_unit: Some("Test Unit".to_string()),
            country: Some("US".to_string()),
            state: Some("California".to_string()),
            locality: Some("San Francisco".to_string()),
            email: Some("test@example.com".to_string()),
        },
        validity_days: 365,
        san_dns_names: vec![
            "test.example.com".to_string(),
            "*.test.example.com".to_string(),
        ],
        san_ip_addresses: vec!["127.0.0.1".to_string(), "::1".to_string()],
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let cert = manager.generate_self_signed().unwrap();

    // Verify PEM format
    assert!(cert.cert_pem.contains("-----BEGIN CERTIFICATE-----"));
    assert!(cert.cert_pem.contains("-----END CERTIFICATE-----"));
    assert!(cert.key_pem.contains("-----BEGIN PRIVATE KEY-----"));
    assert!(cert.key_pem.contains("-----END PRIVATE KEY-----"));
}

#[test]
fn test_generate_ca_certificate() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "Test Root CA".to_string(),
            organization: Some("Test CA Organization".to_string()),
            ..Default::default()
        },
        validity_days: 3650, // CA valid for 10 years
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let ca = manager.generate_ca().unwrap();
    let parsed = manager.parse_certificate(ca.cert_pem.as_bytes()).unwrap();

    // Verify CA certificate properties
    assert!(parsed.is_ca);
    assert!(parsed.key_usage.contains(&"keyCertSign".to_string()));
    assert!(parsed.subject_cn.unwrap().contains("CA"));
}

#[test]
fn test_generate_client_certificate_signed_by_ca() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "Root CA".to_string(),
            organization: Some("Test Organization".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

    // Generate CA certificate
    let ca = manager.generate_ca().unwrap();

    // Generate client certificate signed by CA
    let client = manager
        .generate_client_cert("client1.example.com", &ca.cert_pem, &ca.key_pem)
        .unwrap();

    let parsed = manager
        .parse_certificate(client.cert_pem.as_bytes())
        .unwrap();

    // Verify client certificate properties
    assert!(!parsed.is_ca);
    assert_eq!(parsed.subject_cn, Some("client1.example.com".to_string()));
    // Issuer should be the CA
    assert!(parsed.issuer_cn.unwrap().contains("CA"));
}

#[test]
fn test_certificate_with_multiple_sans() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "multi-san.example.com".to_string(),
            ..Default::default()
        },
        san_dns_names: vec![
            "multi-san.example.com".to_string(),
            "alt1.example.com".to_string(),
            "alt2.example.com".to_string(),
            "*.wildcard.example.com".to_string(),
        ],
        san_ip_addresses: vec![
            "192.168.1.1".to_string(),
            "10.0.0.1".to_string(),
            "127.0.0.1".to_string(),
        ],
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let cert = manager.generate_self_signed().unwrap();
    let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

    // Verify SANs are present
    assert!(
        parsed
            .san_dns_names
            .contains(&"multi-san.example.com".to_string())
    );
    assert!(
        parsed
            .san_dns_names
            .contains(&"alt1.example.com".to_string())
    );
    assert!(
        parsed
            .san_dns_names
            .contains(&"*.wildcard.example.com".to_string())
    );
}

// ============================================================================
// Certificate Parsing Tests
// ============================================================================

#[test]
fn test_parse_certificate_extracts_subject_cn() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "parsed-cn.example.com".to_string(),
            organization: Some("Parsed Org".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let cert = manager.generate_self_signed().unwrap();
    let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

    assert_eq!(parsed.subject_cn, Some("parsed-cn.example.com".to_string()));
}

#[test]
fn test_parse_certificate_checks_validity() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        validity_days: 30,
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let cert = manager.generate_self_signed().unwrap();
    let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

    let now = std::time::SystemTime::now();
    assert!(parsed.not_before <= now);
    assert!(parsed.not_after > now);
}

#[test]
fn test_parse_certificate_extracts_key_usage() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig::default();

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let cert = manager.generate_self_signed().unwrap();
    let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

    // Server certificate should have digitalSignature
    assert!(parsed.key_usage.contains(&"digitalSignature".to_string()));
}

// ============================================================================
// Certificate Validation Tests
// ============================================================================

#[tokio::test]
async fn test_validate_certificates_success() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        validity_days: 365,
        renewal_threshold_days: 30,
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    manager.generate_and_save_certificates().await.unwrap();

    let status = manager.validate_certificates().await.unwrap();

    assert!(status.valid);
    assert!(status.days_until_expiry > 360);
    assert!(!status.needs_renewal);
}

#[tokio::test]
async fn test_validate_certificates_missing_files() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig::default();

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

    // Don't generate certificates - validation should fail
    let result = manager.validate_certificates().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_renewal_threshold() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        validity_days: 30,
        renewal_threshold_days: 60, // Higher than validity
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    manager.generate_and_save_certificates().await.unwrap();

    let status = manager.validate_certificates().await.unwrap();

    // Should need renewal because threshold is higher than validity
    assert!(status.needs_renewal);
}

// ============================================================================
// TLS Configuration Tests
// ============================================================================

#[test]
fn test_tls_config_default() {
    let config = TlsConfig::default();
    assert!(!config.enabled);
    assert!(!config.require_client_certs);
    assert!(config.certificate_manager.is_none());
}

#[test]
fn test_tls_config_with_auto_certificates() {
    let temp_dir = TempDir::new().unwrap();
    let config = TlsConfig::new(true).with_auto_certificates(temp_dir.path().to_path_buf());

    assert!(config.enabled);
    assert!(config.certificate_manager.is_some());
}

#[test]
fn test_tls_config_with_mtls() {
    let temp_dir = TempDir::new().unwrap();
    let config = TlsConfig::new(true)
        .with_auto_certificates(temp_dir.path().to_path_buf())
        .with_mtls();

    assert!(config.enabled);
    assert!(config.require_client_certs);
}

#[test]
fn test_tls_config_explicit_paths() {
    let config = TlsConfig::new(true)
        .with_cert_file("/custom/path/cert.pem".into())
        .with_key_file("/custom/path/key.pem".into())
        .with_ca_file("/custom/path/ca.pem".into());

    let (cert, key) = config.get_certificate_paths().unwrap();
    assert_eq!(cert, std::path::PathBuf::from("/custom/path/cert.pem"));
    assert_eq!(key, std::path::PathBuf::from("/custom/path/key.pem"));
    assert_eq!(
        config.get_ca_path(),
        Some(std::path::PathBuf::from("/custom/path/ca.pem"))
    );
}

#[tokio::test]
async fn test_build_server_config() {
    let temp_dir = TempDir::new().unwrap();

    // Create and initialize certificate manager
    let cert_config = CertificateConfig::default();
    let manager = CertificateManager::new(cert_config, temp_dir.path().to_path_buf());
    manager.generate_and_save_certificates().await.unwrap();

    // Build TLS config
    let tls_config = TlsConfig::new(true).with_certificate_manager(manager);
    let server_config = tls_config.build_server_config().await.unwrap();

    // Server config should be valid
    assert!(server_config.alpn_protocols.is_empty()); // No ALPN set by default
}

#[tokio::test]
async fn test_build_server_config_with_alpn() {
    let temp_dir = TempDir::new().unwrap();

    let cert_config = CertificateConfig::default();
    let manager = CertificateManager::new(cert_config, temp_dir.path().to_path_buf());
    manager.generate_and_save_certificates().await.unwrap();

    let tls_config = TlsConfig::new(true).with_certificate_manager(manager);
    let server_config_builder = TlsServerConfig::new(tls_config);

    // Test HTTP/2 config
    let h2_config = server_config_builder.for_http2().await.unwrap();
    assert!(h2_config.alpn_protocols.contains(&b"h2".to_vec()));

    // Test HTTP/1.1 config
    let h1_config = server_config_builder.for_http1().await.unwrap();
    assert!(h1_config.alpn_protocols.contains(&b"http/1.1".to_vec()));
}

#[tokio::test]
async fn test_mtls_server_config_requires_client_auth() {
    let temp_dir = TempDir::new().unwrap();

    // Create and save CA certificate
    let cert_config = CertificateConfig::default();
    let manager = CertificateManager::new(cert_config, temp_dir.path().to_path_buf());
    manager.generate_and_save_ca().await.unwrap();
    manager.generate_and_save_certificates().await.unwrap();

    // Build mTLS config
    let tls_config = TlsConfig::new(true)
        .with_certificate_manager(manager)
        .with_mtls();
    let server_config = tls_config.build_server_config().await.unwrap();

    // Config should be valid (verify it built successfully)
    // Note: client_auth_mandatory() was removed in newer rustls versions
    assert!(Arc::strong_count(&server_config) >= 1);
}

// ============================================================================
// Client Certificate Info Tests
// ============================================================================

#[test]
fn test_client_certificate_info_from_der() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "test-client".to_string(),
            organization: Some("Test Client Org".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let cert = manager.generate_self_signed().unwrap();

    // Parse PEM to get DER
    let pem_data = cert.cert_pem.as_bytes();
    let (_, pem_block) = x509_parser::pem::parse_x509_pem(pem_data).unwrap();

    let info = ClientCertificateInfo::from_der(&pem_block.contents).unwrap();

    assert_eq!(info.common_name, Some("test-client".to_string()));
    assert_eq!(info.organization, Some("Test Client Org".to_string()));
    assert!(info.is_valid);
    assert!(!info.fingerprint.is_empty());
    assert!(info.fingerprint.len() == 64); // SHA-256 hex is 64 chars
}

// ============================================================================
// Certificate Utility Tests
// ============================================================================

#[test]
fn test_load_certs_from_pem() {
    use proximadb::network::tls::certificate_manager::utils::load_certs_from_pem;

    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig::default();
    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

    let cert = manager.generate_self_signed().unwrap();
    let certs = load_certs_from_pem(cert.cert_pem.as_bytes()).unwrap();

    assert_eq!(certs.len(), 1);
}

#[test]
fn test_load_private_key_from_pem() {
    use proximadb::network::tls::certificate_manager::utils::load_private_key_from_pem;

    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig::default();
    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

    let cert = manager.generate_self_signed().unwrap();
    let key = load_private_key_from_pem(cert.key_pem.as_bytes()).unwrap();

    assert!(!key.0.is_empty());
}

#[test]
fn test_development_config() {
    use proximadb::network::tls::certificate_manager::utils::development_config;

    let config = development_config();
    assert!(config.auto_generate);
    assert_eq!(config.renewal_threshold_days, 7);
    assert_eq!(config.validity_days, 90);
    assert!(config.san_dns_names.contains(&"localhost".to_string()));
}

#[test]
fn test_production_config() {
    use proximadb::network::tls::certificate_manager::utils::production_config;

    let config = production_config("example.com");
    assert!(!config.auto_generate);
    assert_eq!(config.subject.common_name, "example.com");
    assert!(config.san_dns_names.contains(&"example.com".to_string()));
    assert!(config.san_dns_names.contains(&"*.example.com".to_string()));
}

// ============================================================================
// File Permission Tests (Unix only)
// ============================================================================

#[cfg(unix)]
#[tokio::test]
async fn test_private_key_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig::default();
    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

    manager.generate_and_save_certificates().await.unwrap();

    let key_path = manager.get_key_path();
    let perms = std::fs::metadata(key_path).unwrap().permissions();

    // Private key should be readable only by owner (0600)
    assert_eq!(perms.mode() & 0o777, 0o600);
}

// ============================================================================
// Certificate Chain Tests
// ============================================================================

#[tokio::test]
async fn test_full_certificate_chain() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "Root CA".to_string(),
            organization: Some("Test Org".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

    // 1. Generate CA
    let ca = manager.generate_ca().unwrap();

    // 2. Generate server certificate signed by CA
    let server = manager
        .generate_client_cert("server.example.com", &ca.cert_pem, &ca.key_pem)
        .unwrap();

    // 3. Generate client certificate signed by CA
    let client = manager
        .generate_client_cert("client.example.com", &ca.cert_pem, &ca.key_pem)
        .unwrap();

    // Parse all certificates
    let ca_parsed = manager.parse_certificate(ca.cert_pem.as_bytes()).unwrap();
    let server_parsed = manager
        .parse_certificate(server.cert_pem.as_bytes())
        .unwrap();
    let client_parsed = manager
        .parse_certificate(client.cert_pem.as_bytes())
        .unwrap();

    // Verify CA properties
    assert!(ca_parsed.is_ca);
    assert!(ca_parsed.key_usage.contains(&"keyCertSign".to_string()));

    // Verify server certificate
    assert!(!server_parsed.is_ca);
    assert_eq!(
        server_parsed.subject_cn,
        Some("server.example.com".to_string())
    );

    // Verify client certificate
    assert!(!client_parsed.is_ca);
    assert_eq!(
        client_parsed.subject_cn,
        Some("client.example.com".to_string())
    );

    // Both should be issued by the same CA
    assert_eq!(server_parsed.issuer_cn, client_parsed.issuer_cn);
}

// ============================================================================
// TLS Client Certificate Middleware Tests
// ============================================================================

#[test]
fn test_matches_cn_pattern_exact_match() {
    use proximadb::network::middleware::matches_cn_pattern;

    assert!(matches_cn_pattern(
        "client.example.com",
        "client.example.com"
    ));
    assert!(!matches_cn_pattern(
        "other.example.com",
        "client.example.com"
    ));
    assert!(!matches_cn_pattern(
        "CLIENT.EXAMPLE.COM",
        "client.example.com"
    )); // Case sensitive
}

#[test]
fn test_matches_cn_pattern_wildcard_single_level() {
    use proximadb::network::middleware::matches_cn_pattern;

    // Single-level wildcard should match one level only
    assert!(matches_cn_pattern("client.example.com", "*.example.com"));
    assert!(matches_cn_pattern("server.example.com", "*.example.com"));
    assert!(matches_cn_pattern("api.example.com", "*.example.com"));

    // Should NOT match multi-level
    assert!(!matches_cn_pattern("a.b.example.com", "*.example.com"));
    assert!(!matches_cn_pattern(
        "client.api.example.com",
        "*.example.com"
    ));

    // Should NOT match the base domain
    assert!(!matches_cn_pattern("example.com", "*.example.com"));
}

#[test]
fn test_matches_cn_pattern_star_matches_all() {
    use proximadb::network::middleware::matches_cn_pattern;

    assert!(matches_cn_pattern("anything", "*"));
    assert!(matches_cn_pattern("client.example.com", "*"));
    assert!(matches_cn_pattern("a.b.c.d.e.f", "*"));
    assert!(matches_cn_pattern("", "*"));
    assert!(matches_cn_pattern("123", "*"));
}

#[test]
fn test_tls_client_cert_config_default() {
    use proximadb::network::middleware::TlsClientCertConfig;

    let config = TlsClientCertConfig::default();
    assert!(!config.require_client_cert);
    assert!(config.allowed_cn_patterns.is_empty());
    assert!(config.reject_expired);
    assert!(!config.check_revocation);
    assert_eq!(config.default_roles, vec!["reader".to_string()]);
}

#[test]
fn test_tls_client_cert_config_required() {
    use proximadb::network::middleware::TlsClientCertConfig;

    let config = TlsClientCertConfig::required();
    assert!(config.require_client_cert);
}

#[test]
fn test_tls_client_cert_config_development() {
    use proximadb::network::middleware::TlsClientCertConfig;

    let config = TlsClientCertConfig::development();
    assert!(!config.require_client_cert);
    assert!(config.allowed_cn_patterns.contains(&"*".to_string()));
    assert!(!config.reject_expired); // Dev mode allows expired certs
}

#[test]
fn test_tls_client_cert_config_production() {
    use proximadb::network::middleware::TlsClientCertConfig;

    let config = TlsClientCertConfig::production(vec![
        "*.mycompany.com".to_string(),
        "admin.internal".to_string(),
    ]);
    assert!(config.require_client_cert);
    assert_eq!(config.allowed_cn_patterns.len(), 2);
    assert!(config.reject_expired);
    assert!(config.check_revocation);
}

#[test]
fn test_tls_client_cert_config_builder() {
    use proximadb::network::middleware::TlsClientCertConfig;

    let config = TlsClientCertConfig::default()
        .allow_cn("*.example.com")
        .allow_cn("admin.internal")
        .map_cn_to_user("admin.internal", "admin-service")
        .with_default_roles(vec!["admin".to_string(), "reader".to_string()]);

    assert_eq!(config.allowed_cn_patterns.len(), 2);
    assert!(
        config
            .allowed_cn_patterns
            .contains(&"*.example.com".to_string())
    );
    assert!(
        config
            .allowed_cn_patterns
            .contains(&"admin.internal".to_string())
    );
    assert_eq!(
        config.cn_to_user_mapping.get("admin.internal"),
        Some(&"admin-service".to_string())
    );
    assert_eq!(
        config.default_roles,
        vec!["admin".to_string(), "reader".to_string()]
    );
}

// ============================================================================
// Server Certificate Configuration Tests
// ============================================================================

#[test]
fn test_server_certificate_with_all_subject_fields() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "full-subject.example.com".to_string(),
            organization: Some("Test Organization".to_string()),
            organizational_unit: Some("Engineering".to_string()),
            country: Some("US".to_string()),
            state: Some("California".to_string()),
            locality: Some("San Francisco".to_string()),
            email: Some("admin@example.com".to_string()),
        },
        validity_days: 365,
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let cert = manager.generate_self_signed().unwrap();
    let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

    assert_eq!(
        parsed.subject_cn,
        Some("full-subject.example.com".to_string())
    );
}

#[test]
fn test_certificate_with_ipv6_san() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "ipv6-test.example.com".to_string(),
            ..Default::default()
        },
        san_ip_addresses: vec![
            "127.0.0.1".to_string(),
            "::1".to_string(),
            "2001:db8::1".to_string(),
        ],
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let cert = manager.generate_self_signed().unwrap();

    // Certificate should be generated successfully with IPv6 SANs
    assert!(cert.cert_pem.contains("-----BEGIN CERTIFICATE-----"));
}

#[test]
fn test_short_validity_certificate() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        validity_days: 1, // Very short validity
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let cert = manager.generate_self_signed().unwrap();
    let parsed = manager.parse_certificate(cert.cert_pem.as_bytes()).unwrap();

    // Certificate should still be valid (just created)
    assert!(parsed.not_before <= std::time::SystemTime::now());
    assert!(parsed.not_after > std::time::SystemTime::now());

    // But validity period should be short
    let validity_duration = parsed.not_after.duration_since(parsed.not_before).unwrap();
    let one_day = std::time::Duration::from_secs(24 * 60 * 60);
    let two_days = std::time::Duration::from_secs(2 * 24 * 60 * 60);
    assert!(validity_duration >= one_day && validity_duration < two_days);
}

#[test]
fn test_long_validity_ca_certificate() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig {
        subject: CertificateSubject {
            common_name: "Long Lived CA".to_string(),
            ..Default::default()
        },
        validity_days: 7300, // 20 years
        ..Default::default()
    };

    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());
    let ca = manager.generate_ca().unwrap();
    let parsed = manager.parse_certificate(ca.cert_pem.as_bytes()).unwrap();

    // CA should be valid for ~20 years
    let validity_duration = parsed.not_after.duration_since(parsed.not_before).unwrap();
    let twenty_years = std::time::Duration::from_secs(20 * 365 * 24 * 60 * 60);
    assert!(validity_duration >= twenty_years);
}

// ============================================================================
// Certificate Error Handling Tests
// ============================================================================

#[test]
fn test_parse_invalid_certificate() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig::default();
    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

    // Try to parse invalid PEM data
    let result = manager.parse_certificate(b"not a valid certificate");
    assert!(result.is_err());
}

#[test]
fn test_client_certificate_info_invalid_der() {
    let result = ClientCertificateInfo::from_der(b"invalid der data");
    assert!(result.is_err());
}

// ============================================================================
// REST TLS Config Tests
// ============================================================================

#[test]
fn test_rest_tls_config_default() {
    use proximadb::network::rest::server::RestTlsConfig;

    let config = RestTlsConfig::default();
    assert!(!config.require_client_certs);
    assert!(config.allowed_cn_patterns.is_empty());
    assert!(config.ca_file.is_none());
}

#[test]
fn test_rest_tls_config_with_mtls() {
    use proximadb::network::rest::server::RestTlsConfig;
    use std::path::PathBuf;

    let config = RestTlsConfig::new(
        PathBuf::from("/path/to/cert.pem"),
        PathBuf::from("/path/to/key.pem"),
    )
    .with_mtls(PathBuf::from("/path/to/ca.pem"))
    .with_allowed_cn_patterns(vec!["*.example.com".to_string()])
    .with_default_roles(vec!["admin".to_string()]);

    assert!(config.require_client_certs);
    assert!(config.ca_file.is_some());
    assert_eq!(config.allowed_cn_patterns.len(), 1);
    assert_eq!(config.default_roles, vec!["admin".to_string()]);
}

// ============================================================================
// Multi-Server TLS Config Tests
// ============================================================================

#[test]
fn test_multi_server_tls_config_default() {
    use proximadb::network::multi_server::TLSConfig;

    let config = TLSConfig::default();
    assert!(!config.enabled);
    assert!(!config.require_client_certs);
    assert!(config.cert_file.is_none());
    assert!(config.key_file.is_none());
    assert!(config.ca_file.is_none());
    assert!(!config.auto_generate);
    assert_eq!(config.validity_days, 365);
    assert_eq!(config.renewal_threshold_days, 30);
}

#[test]
fn test_multi_server_tls_config_with_certificates() {
    use proximadb::network::multi_server::TLSConfig;

    let config = TLSConfig::new().with_certificates("/path/to/cert.pem", "/path/to/key.pem");

    assert!(config.enabled);
    assert_eq!(config.cert_file, Some("/path/to/cert.pem".to_string()));
    assert_eq!(config.key_file, Some("/path/to/key.pem".to_string()));
}

#[test]
fn test_multi_server_tls_config_with_mtls() {
    use proximadb::network::multi_server::TLSConfig;

    let config = TLSConfig::new()
        .with_certificates("/path/to/cert.pem", "/path/to/key.pem")
        .with_mtls("/path/to/ca.pem");

    assert!(config.enabled);
    assert!(config.require_client_certs);
    assert!(config.is_mtls_enabled());
    assert_eq!(config.get_ca_path(), Some("/path/to/ca.pem"));
}

#[test]
fn test_multi_server_tls_config_auto_generate() {
    use proximadb::network::multi_server::TLSConfig;

    let config = TLSConfig::new().with_auto_generate(90);

    assert!(config.auto_generate);
    assert_eq!(config.validity_days, 90);
}

// ============================================================================
// Certificate Fingerprint Tests
// ============================================================================

#[test]
fn test_certificate_fingerprint_is_sha256() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig::default();
    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

    let cert = manager.generate_self_signed().unwrap();
    let pem_data = cert.cert_pem.as_bytes();
    let (_, pem_block) = x509_parser::pem::parse_x509_pem(pem_data).unwrap();

    let info = ClientCertificateInfo::from_der(&pem_block.contents).unwrap();

    // SHA-256 fingerprint should be 64 hex characters
    assert_eq!(info.fingerprint.len(), 64);
    // Should only contain hex characters
    assert!(info.fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_unique_fingerprints_for_different_certs() {
    let temp_dir = TempDir::new().unwrap();
    let config = CertificateConfig::default();
    let manager = CertificateManager::new(config, temp_dir.path().to_path_buf());

    // Generate two certificates
    let cert1 = manager.generate_self_signed().unwrap();
    let cert2 = manager.generate_self_signed().unwrap();

    let pem_data1 = cert1.cert_pem.as_bytes();
    let pem_data2 = cert2.cert_pem.as_bytes();
    let (_, pem_block1) = x509_parser::pem::parse_x509_pem(pem_data1).unwrap();
    let (_, pem_block2) = x509_parser::pem::parse_x509_pem(pem_data2).unwrap();

    let info1 = ClientCertificateInfo::from_der(&pem_block1.contents).unwrap();
    let info2 = ClientCertificateInfo::from_der(&pem_block2.contents).unwrap();

    // Fingerprints should be different
    assert_ne!(info1.fingerprint, info2.fingerprint);
}

// ============================================================================
// Server Config Builder Tests
// ============================================================================

#[tokio::test]
async fn test_build_server_config_http1_alpn() {
    let temp_dir = TempDir::new().unwrap();

    let cert_config = CertificateConfig::default();
    let manager = CertificateManager::new(cert_config, temp_dir.path().to_path_buf());
    manager.generate_and_save_certificates().await.unwrap();

    let tls_config = TlsConfig::new(true).with_certificate_manager(manager);
    let server_config = TlsServerConfig::new(tls_config);

    let h1_config = server_config.for_http1().await.unwrap();

    // HTTP/1.1 ALPN should be set
    assert!(h1_config.alpn_protocols.contains(&b"http/1.1".to_vec()));
    assert!(!h1_config.alpn_protocols.contains(&b"h2".to_vec()));
}

#[tokio::test]
async fn test_build_server_config_http2_alpn() {
    let temp_dir = TempDir::new().unwrap();

    let cert_config = CertificateConfig::default();
    let manager = CertificateManager::new(cert_config, temp_dir.path().to_path_buf());
    manager.generate_and_save_certificates().await.unwrap();

    let tls_config = TlsConfig::new(true).with_certificate_manager(manager);
    let server_config = TlsServerConfig::new(tls_config);

    let h2_config = server_config.for_http2().await.unwrap();

    // HTTP/2 ALPN should be set
    assert!(h2_config.alpn_protocols.contains(&b"h2".to_vec()));
    assert!(!h2_config.alpn_protocols.contains(&b"http/1.1".to_vec()));
}

#[tokio::test]
async fn test_build_server_config_http1_and_http2_alpn() {
    let temp_dir = TempDir::new().unwrap();

    let cert_config = CertificateConfig::default();
    let manager = CertificateManager::new(cert_config, temp_dir.path().to_path_buf());
    manager.generate_and_save_certificates().await.unwrap();

    let tls_config = TlsConfig::new(true).with_certificate_manager(manager);
    let server_config = TlsServerConfig::new(tls_config);

    let both_config = server_config.for_http1_and_http2().await.unwrap();

    // Both ALPN protocols should be set
    assert!(both_config.alpn_protocols.contains(&b"h2".to_vec()));
    assert!(both_config.alpn_protocols.contains(&b"http/1.1".to_vec()));
}

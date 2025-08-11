// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metadata Backend Configuration Examples
//!
//! This example demonstrates how to configure different metadata backends
//! using the ProximaDB server builder pattern.

use anyhow::Result;
use proximadb::server::builder::ServerBuilder;
use tracing::{debug, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    debug!("🚀 ProximaDB Metadata Backend Configuration Examples");
    debug!("=====================================================");

    // Example 1: Local filesystem metadata backend
    debug!("\n📁 Example 1: Local Filesystem Metadata Backend");
    let _server_local = ServerBuilder::new()
        .with_server_endpoint("127.0.0.1", 5678)
        .configure_storage(|storage| {
            storage.with_local_metadata_backend("/data/proximadb/metadata")
        })
        .build()
        .await?;

    info!("✅ Local filesystem metadata backend configured");
    debug!("   📂 Storage path: /data/proximadb/metadata");

    // Example 2: AWS S3 metadata backend with IAM role
    debug!("\n☁️ Example 2: AWS S3 Metadata Backend (IAM Role)");
    let _server_s3_iam = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .configure_storage(|storage| {
            storage.with_s3_metadata_backend(
                "my-proximadb-bucket",
                "us-west-2",
                true, // Use IAM role
            )
        })
        .build()
        .await?;

    info!("✅ S3 metadata backend configured with IAM role");
    debug!("   🪣 Bucket: my-proximadb-bucket");
    debug!("   🌍 Region: us-west-2");
    debug!("   🔐 Auth: IAM Role");

    // Example 3: Azure Blob Storage metadata backend with Managed Identity
    debug!("\n🔵 Example 3: Azure Blob Storage Metadata Backend");
    let _server_azure = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .configure_storage(|storage| {
            storage.with_azure_metadata_backend(
                "myproximadbaccount",
                "metadata-container",
                true, // Use Managed Identity
            )
        })
        .build()
        .await?;

    info!("✅ Azure metadata backend configured with Managed Identity");
    debug!("   🏦 Account: myproximadbaccount");
    debug!("   📦 Container: metadata-container");
    debug!("   🔐 Auth: Managed Identity");

    // Example 4: Google Cloud Storage metadata backend with Workload Identity
    debug!("\n🟡 Example 4: Google Cloud Storage Metadata Backend");
    let _server_gcs = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .configure_storage(|storage| {
            storage.with_gcs_metadata_backend(
                "my-project-id",
                "proximadb-metadata-bucket",
                true, // Use Workload Identity
            )
        })
        .build()
        .await?;

    info!("✅ GCS metadata backend configured with Workload Identity");
    info!("   📊 Project: my-project-id");
    debug!("   🪣 Bucket: proximadb-metadata-bucket");
    debug!("   🔐 Auth: Workload Identity");

    // Example 5: Memory metadata backend (for testing)
    debug!("\n🧠 Example 5: Memory Metadata Backend (Testing)");
    let _server_memory = ServerBuilder::new()
        .with_server_endpoint("127.0.0.1", 5678)
        .configure_storage(|storage| storage.with_memory_metadata_backend())
        .build()
        .await?;

    info!("✅ Memory metadata backend configured");
    warn!("   ⚠️  Note: Data will not persist across restarts");

    // Example 6: Custom metadata backend configuration
    debug!("\n⚙️ Example 6: Custom Metadata Backend Configuration");
    let _server_custom = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .configure_storage(|storage| {
            storage.configure_metadata_backend(|| {
                use proximadb::core::config::{
                    CloudStorageConfig, MetadataBackendConfig, S3Config,
                };

                MetadataBackendConfig {
                    backend_type: "filestore".to_string(),
                    storage_url: "s3://custom-bucket/custom-path/metadata".to_string(),
                    cloud_config: Some(CloudStorageConfig {
                        s3_config: Some(S3Config {
                            region: "eu-central-1".to_string(),
                            bucket: "custom-bucket".to_string(),
                            access_key_id: Some("AKIAEXAMPLE".to_string()),
                            secret_access_key: Some("secret123".to_string()),
                            use_iam_role: false,
                            endpoint: Some("https://custom-s3-endpoint.com".to_string()),
                        }),
                        azure_config: None,
                        gcs_config: None,
                    }),
                    cache_size_mb: Some(512),
                    flush_interval_secs: Some(90),
                }
            })
        })
        .build()
        .await?;

    info!("✅ Custom metadata backend configured");
    debug!("   🪣 Custom S3 endpoint with access keys");
    debug!("   💾 Cache: 512MB, Flush: 90s");

    info!("\n🎉 All metadata backend examples configured successfully!");
    debug!("\n💡 Usage Notes:");
    debug!("   - Use local filesystem for development and single-node deployments");
    debug!("   - Use cloud storage (S3/Azure/GCS) for production and multi-region deployments");
    debug!("   - Use memory backend only for testing and ephemeral scenarios");
    debug!("   - Configure cloud authentication according to your security policies");

    Ok(())
}

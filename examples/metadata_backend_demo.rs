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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::init();

    println!("🚀 ProximaDB Metadata Backend Configuration Examples");
    println!("=====================================================");

    // Example 1: Local filesystem metadata backend
    println!("\n📁 Example 1: Local Filesystem Metadata Backend");
    let _server_local = ServerBuilder::new()
        .with_server_endpoint("127.0.0.1", 5678)
        .configure_storage(|storage| {
            storage.with_local_metadata_backend("/data/proximadb/metadata")
        })
        .build()
        .await?;
    
    println!("✅ Local filesystem metadata backend configured");
    println!("   📂 Storage path: /data/proximadb/metadata");

    // Example 2: AWS S3 metadata backend with IAM role
    println!("\n☁️ Example 2: AWS S3 Metadata Backend (IAM Role)");
    let _server_s3_iam = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .configure_storage(|storage| {
            storage.with_s3_metadata_backend(
                "my-proximadb-bucket", 
                "us-west-2", 
                true  // Use IAM role
            )
        })
        .build()
        .await?;
    
    println!("✅ S3 metadata backend configured with IAM role");
    println!("   🪣 Bucket: my-proximadb-bucket");
    println!("   🌍 Region: us-west-2");
    println!("   🔐 Auth: IAM Role");

    // Example 3: Azure Blob Storage metadata backend with Managed Identity
    println!("\n🔵 Example 3: Azure Blob Storage Metadata Backend");
    let _server_azure = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .configure_storage(|storage| {
            storage.with_azure_metadata_backend(
                "myproximadbaccount",
                "metadata-container",
                true  // Use Managed Identity
            )
        })
        .build()
        .await?;
    
    println!("✅ Azure metadata backend configured with Managed Identity");
    println!("   🏦 Account: myproximadbaccount");
    println!("   📦 Container: metadata-container");
    println!("   🔐 Auth: Managed Identity");

    // Example 4: Google Cloud Storage metadata backend with Workload Identity
    println!("\n🟡 Example 4: Google Cloud Storage Metadata Backend");
    let _server_gcs = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .configure_storage(|storage| {
            storage.with_gcs_metadata_backend(
                "my-project-id",
                "proximadb-metadata-bucket",
                true  // Use Workload Identity
            )
        })
        .build()
        .await?;
    
    println!("✅ GCS metadata backend configured with Workload Identity");
    println!("   📊 Project: my-project-id");
    println!("   🪣 Bucket: proximadb-metadata-bucket");
    println!("   🔐 Auth: Workload Identity");

    // Example 5: Memory metadata backend (for testing)
    println!("\n🧠 Example 5: Memory Metadata Backend (Testing)");
    let _server_memory = ServerBuilder::new()
        .with_server_endpoint("127.0.0.1", 5678)
        .configure_storage(|storage| {
            storage.with_memory_metadata_backend()
        })
        .build()
        .await?;
    
    println!("✅ Memory metadata backend configured");
    println!("   ⚠️  Note: Data will not persist across restarts");

    // Example 6: Custom metadata backend configuration
    println!("\n⚙️ Example 6: Custom Metadata Backend Configuration");
    let _server_custom = ServerBuilder::new()
        .with_server_endpoint("0.0.0.0", 5678)
        .configure_storage(|storage| {
            storage.configure_metadata_backend(|| {
                use proximadb::core::config::{MetadataBackendConfig, CloudStorageConfig, S3Config};
                
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
    
    println!("✅ Custom metadata backend configured");
    println!("   🪣 Custom S3 endpoint with access keys");
    println!("   💾 Cache: 512MB, Flush: 90s");

    println!("\n🎉 All metadata backend examples configured successfully!");
    println!("\n💡 Usage Notes:");
    println!("   - Use local filesystem for development and single-node deployments");
    println!("   - Use cloud storage (S3/Azure/GCS) for production and multi-region deployments");
    println!("   - Use memory backend only for testing and ephemeral scenarios");
    println!("   - Configure cloud authentication according to your security policies");

    Ok(())
}
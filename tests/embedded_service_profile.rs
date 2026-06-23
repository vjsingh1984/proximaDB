//! Embedded co-design (Phase 1): the `Embedded` service profile must NOT build
//! the network-only observability/billing machinery, while `Server` keeps it.
//!
//! A fused in-process database deletes Dimension 2 (network) — there is no
//! Prometheus scrape and no billing/egress surface — so paying for the metrics
//! persistence + chargeback publisher in-process violates co-design tenets 1
//! ("don't pay for a dimension you deleted") and 5 ("egress/KOU is inert in
//! embedded"). `metrics_updater` is the observable proxy: present only for the
//! networked server.

use proximadb::core::config::{StorageConfig, StorageLocation};
use proximadb::network::multi_server::{ServiceProfile, SharedServices};
use tempfile::TempDir;

fn storage_config(dir: &std::path::Path) -> StorageConfig {
    let mut sc = StorageConfig::default();
    sc.metadata_url = format!("file://{}/metadata", dir.display());
    sc.storage_locations = vec![StorageLocation {
        url: format!("file://{}/storage", dir.display()),
        weight: 1,
        tags: vec![],
    }];
    sc.wal_config.write_buffer_directory = format!("file://{}/wal", dir.display());
    sc
}

#[tokio::test]
async fn embedded_profile_omits_billing_machinery_server_keeps_it() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Fused / in-process: no metrics persistence + billing publisher.
    let emb_dir = TempDir::new().expect("tempdir");
    let (embedded, _) = SharedServices::new(
        None,
        &storage_config(emb_dir.path()),
        None,
        None,
        ServiceProfile::Embedded,
    )
    .await
    .expect("embedded SharedServices");
    assert!(
        embedded.metrics_updater.is_none(),
        "Embedded profile must not construct the metrics/billing publisher"
    );

    // Networked server: keeps the observability/billing surface.
    let srv_dir = TempDir::new().expect("tempdir");
    let (server, _) = SharedServices::new(
        None,
        &storage_config(srv_dir.path()),
        None,
        None,
        ServiceProfile::Server,
    )
    .await
    .expect("server SharedServices");
    assert!(
        server.metrics_updater.is_some(),
        "Server profile must construct the metrics/billing publisher"
    );
}

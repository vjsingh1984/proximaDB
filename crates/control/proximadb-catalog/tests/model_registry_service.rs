use std::sync::Arc;

use proximadb_catalog::CatalogManager;
use proximadb_catalog::mlops::{
    CatalogEmbeddingModelVersion, CatalogMlopsAsset, CatalogModelRegistryMutation,
    CatalogModelUsePolicy,
};
use proximadb_catalog::model_registry_service::{
    CatalogModelRegistryService, ModelRegistryServiceError,
};
use proximadb_catalog::native::{NativeCatalog, NativeCatalogConfig};

async fn service() -> (tempfile::TempDir, CatalogModelRegistryService) {
    let directory = tempfile::tempdir().unwrap();
    let catalog = NativeCatalog::new(
        "default".to_string(),
        NativeCatalogConfig {
            storage_url: directory.path().to_string_lossy().to_string(),
            metadata_format: "json".to_string(),
            versioned: false,
            max_versions: 100,
        },
        Arc::new(proximadb_catalog::cache::CatalogCache::new(64, 60)),
    )
    .await
    .unwrap();
    let manager = Arc::new(CatalogManager::new());
    manager.register(Arc::new(catalog)).await.unwrap();
    (directory, CatalogModelRegistryService::new(manager))
}

fn model_version() -> CatalogEmbeddingModelVersion {
    let asset: CatalogMlopsAsset = serde_json::from_str(include_str!(
        "../../../../clients/python/tests/fixtures/embedding_model_xcatalog_asset.json"
    ))
    .unwrap();
    let CatalogMlopsAsset::EmbeddingModel(registry) = asset;
    registry.version(1).unwrap().clone()
}

#[tokio::test]
async fn lifecycle_service_is_tenant_scoped_and_lists_deterministically() {
    let (_directory, service) = service().await;

    let zeta = service.create_registry("tenant-a", "zeta").await.unwrap();
    let alpha = service.create_registry("tenant-a", "alpha").await.unwrap();
    let other = service.create_registry("tenant-b", "alpha").await.unwrap();
    assert_ne!(alpha.asset_id, other.asset_id);

    let tenant_a = service.list_registries("tenant-a").await.unwrap();
    assert_eq!(
        tenant_a
            .iter()
            .map(|record| record.registry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    assert_eq!(
        service
            .get_registry("tenant-a", "zeta")
            .await
            .unwrap()
            .asset_id,
        zeta.asset_id
    );
    assert!(matches!(
        service.create_registry("tenant-a", "zeta").await,
        Err(ModelRegistryServiceError::AlreadyExists { .. })
    ));
    assert!(matches!(
        service.get_registry("tenant-b", "zeta").await,
        Err(ModelRegistryServiceError::NotFound { .. })
    ));

    let left = service.clone();
    let right = service.clone();
    let (left, right) = tokio::join!(
        left.create_registry("tenant-a", "concurrent"),
        right.create_registry("tenant-a", "concurrent")
    );
    assert_ne!(left.is_ok(), right.is_ok());
    assert!(matches!(
        left.err().or_else(|| right.err()),
        Some(ModelRegistryServiceError::AlreadyExists { .. })
    ));
}

#[tokio::test]
async fn alias_resolution_returns_a_concrete_binding_that_does_not_drift() {
    let (_directory, service) = service().await;
    service
        .create_registry("tenant-a", "search-embedding")
        .await
        .unwrap();
    service
        .apply_mutation(
            "tenant-a",
            "search-embedding",
            0,
            CatalogModelRegistryMutation::register_version(model_version()),
        )
        .await
        .unwrap();

    assert!(matches!(
        service
            .apply_mutation(
                "tenant-a",
                "search-embedding",
                0,
                CatalogModelRegistryMutation::set_alias("stale", 1),
            )
            .await,
        Err(ModelRegistryServiceError::Contract(
            proximadb_catalog::mlops::CatalogModelContractError::RevisionConflict {
                expected: 0,
                current: 1,
            }
        ))
    ));
    service
        .apply_mutation(
            "tenant-a",
            "search-embedding",
            1,
            CatalogModelRegistryMutation::set_alias("champion", 1),
        )
        .await
        .unwrap();

    let first = service
        .resolve_alias_binding(
            "tenant-a",
            "search-embedding",
            "champion",
            768,
            &CatalogModelUsePolicy::registration_only(),
        )
        .await
        .unwrap();
    assert_eq!(first.binding.model_version, Some(1));
    assert_eq!(first.snapshot.model.version, 1);
    assert_eq!(
        first.binding.contract_sha256.as_deref(),
        Some(first.snapshot.contract_sha256.as_str())
    );

    let mut second_version = model_version();
    second_version.version = 2;
    second_version.artifact.digest =
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_string();
    service
        .apply_mutation(
            "tenant-a",
            "search-embedding",
            2,
            CatalogModelRegistryMutation::register_version(second_version),
        )
        .await
        .unwrap();
    service
        .apply_mutation(
            "tenant-a",
            "search-embedding",
            3,
            CatalogModelRegistryMutation::set_alias("champion", 2),
        )
        .await
        .unwrap();

    let current = service
        .resolve_alias_binding(
            "tenant-a",
            "search-embedding",
            "champion",
            768,
            &CatalogModelUsePolicy::registration_only(),
        )
        .await
        .unwrap();
    assert_eq!(current.binding.model_version, Some(2));
    assert_eq!(first.binding.model_version, Some(1));
    assert_eq!(first.snapshot.model.version, 1);
}

#[test]
fn service_rejects_empty_authority_segments_before_catalog_io() {
    let manager = Arc::new(CatalogManager::new());
    let service = CatalogModelRegistryService::new(manager);
    assert!(matches!(
        service.registry_identifier("", "model"),
        Err(ModelRegistryServiceError::InvalidTenant { .. })
    ));
    assert!(matches!(
        service.registry_identifier("tenant-a", "  "),
        Err(ModelRegistryServiceError::InvalidName { .. })
    ));
    assert!(matches!(
        service.registry_identifier("../tenant-b", "model"),
        Err(ModelRegistryServiceError::InvalidTenant { .. })
    ));
    assert!(matches!(
        service.registry_identifier("tenant-a", "../model"),
        Err(ModelRegistryServiceError::InvalidName { .. })
    ));
}

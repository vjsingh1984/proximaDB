use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use proximadb_catalog::mlops::{
    CatalogArtifactDescriptor, CatalogDeploymentBinding, CatalogDimensionPolicy,
    CatalogDimensionRange, CatalogEmbeddingInputContract, CatalogEmbeddingModelRegistry,
    CatalogEmbeddingModelVersion, CatalogEmbeddingOutputContract, CatalogEvaluationEvidence,
    CatalogMlopsAsset, CatalogModelAccess, CatalogModelDecision, CatalogModelDecisionKind,
    CatalogModelGovernance, CatalogModelRegistryMutation, CatalogModelUsePolicy,
};
use proximadb_catalog::native::{NativeCatalog, NativeCatalogConfig};
use proximadb_catalog::schema::validate_schema;
use proximadb_catalog::{
    Catalog, CatalogColumn, CatalogEmbeddingConfig, CatalogMlopsAssetExt, CatalogTableSchema,
    TableIdentifier,
};

const MODEL_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DATASET_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn named_wire_payloads_preserve_the_existing_json_contract() {
    let range = CatalogDimensionPolicy::Range(CatalogDimensionRange { minimum: 128 });
    assert_eq!(
        serde_json::to_value(range).unwrap(),
        serde_json::json!({ "range": { "minimum": 128 } })
    );
    assert_eq!(
        serde_json::to_value(CatalogModelRegistryMutation::set_alias("champion", 7)).unwrap(),
        serde_json::json!({
            "operation": "set_alias",
            "payload": { "alias": "champion", "version": 7 }
        })
    );
}

fn version(number: u64) -> CatalogEmbeddingModelVersion {
    let artifact = CatalogArtifactDescriptor::new(
        "hf://BAAI/bge-base-en-v1.5@aed724fc5",
        MODEL_DIGEST,
        438_000_000,
        "application/vnd.huggingface.safetensors",
    )
    .unwrap();
    let input = CatalogEmbeddingInputContract::new(
        "aed724fc5",
        "BAAI/bge-base-en-v1.5",
        "aed724fc5",
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        512,
        2,
        "{text}",
        "Represent this sentence for searching relevant passages: {text}",
    )
    .unwrap();
    let output = CatalogEmbeddingOutputContract::new(
        768,
        CatalogDimensionPolicy::Fixed,
        vec![768],
        true,
        "mean",
    )
    .unwrap();

    CatalogEmbeddingModelVersion::new(
        number,
        "BAAI/bge-base-en-v1.5",
        artifact,
        input,
        output,
        1_786_400_000_000,
    )
    .unwrap()
    .with_governance(
        CatalogModelGovernance::new(
            "mit",
            CatalogModelAccess::Open,
            false,
            ["kserve".to_string(), "sentence-transformers".to_string()],
        )
        .unwrap(),
    )
}

#[test]
fn artifact_requires_a_content_digest_not_only_a_mutable_uri() {
    let error = CatalogArtifactDescriptor::new(
        "hf://BAAI/bge-base-en-v1.5@main",
        "main",
        1,
        "application/octet-stream",
    )
    .unwrap_err();

    assert!(error.to_string().contains("sha256"));
}

#[test]
fn rendered_input_budget_is_validated_at_the_contract_boundary() {
    let error = CatalogEmbeddingInputContract::new(
        "model-sha",
        "tokenizer-id",
        "tokenizer-sha",
        MODEL_DIGEST,
        512,
        512,
        "{text}",
        "{text}",
    )
    .unwrap_err();

    assert!(error.to_string().contains("special token"));

    let duplicate_placeholder = CatalogEmbeddingInputContract::new(
        "model-sha",
        "tokenizer-id",
        "tokenizer-sha",
        MODEL_DIGEST,
        512,
        2,
        "{text} {text}",
        "{text}",
    )
    .unwrap_err();
    assert!(duplicate_placeholder.to_string().contains("exactly one"));
}

#[test]
fn dimension_policy_cannot_advertise_impossible_outputs() {
    let error = CatalogEmbeddingOutputContract::new(
        768,
        CatalogDimensionPolicy::Discrete,
        vec![1024, 768, 256],
        true,
        "mean",
    )
    .unwrap_err();

    assert!(error.to_string().contains("native dimension"));
}

#[test]
fn versions_are_immutable_while_aliases_are_mutable_references() {
    let mut registry = CatalogEmbeddingModelRegistry::new("search-embedding").unwrap();
    registry.register_version(version(1)).unwrap();
    registry.set_alias("champion", 1).unwrap();

    assert_eq!(registry.resolve_alias("champion").unwrap().version, 1);
    assert!(registry.register_version(version(1)).is_err());

    let mut second = version(2);
    second.artifact.digest =
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_string();
    registry.register_version(second).unwrap();
    registry.set_alias("champion", 2).unwrap();

    assert_eq!(registry.resolve_alias("champion").unwrap().version, 2);
    assert_eq!(registry.version(1).unwrap().version, 1);
}

#[test]
fn evidence_approval_and_deployment_are_distinct_version_pinned_records() {
    let mut registry = CatalogEmbeddingModelRegistry::new("search-embedding").unwrap();
    registry.register_version(version(1)).unwrap();

    let evidence = CatalogEvaluationEvidence::new(
        "eval-bge-1",
        1,
        "rag-retrieval-v3",
        DATASET_DIGEST,
        "proximadb-eval@2.1.0",
        BTreeMap::from([
            ("ndcg_at_10".to_string(), 0.73),
            ("recall_at_10".to_string(), 0.82),
        ]),
        1_786_400_100_000,
    )
    .unwrap();
    registry.append_evidence(evidence).unwrap();

    registry
        .record_decision(
            CatalogModelDecision::new(
                "decision-bge-1",
                1,
                CatalogModelDecisionKind::Approved,
                vec!["eval-bge-1".to_string()],
                "principal:ml-reviewers",
                1_786_400_200_000,
            )
            .unwrap(),
        )
        .unwrap();

    registry
        .upsert_deployment(
            CatalogDeploymentBinding::new(
                "prod-search",
                1,
                MODEL_DIGEST,
                "kserve",
                "inference://prod/search-embedding",
                1_786_400_300_000,
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(registry.evidence.len(), 1);
    assert_eq!(registry.decisions.len(), 1);
    assert_eq!(registry.deployments["prod-search"].version, 1);

    let bad_binding = CatalogDeploymentBinding::new(
        "prod-search",
        1,
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        "kserve",
        "inference://prod/search-embedding",
        1_786_400_400_000,
    )
    .unwrap();
    assert!(registry.upsert_deployment(bad_binding).is_err());

    let unapproved_runtime = CatalogDeploymentBinding::new(
        "prod-search-onnx",
        1,
        MODEL_DIGEST,
        "onnx-runtime",
        "inference://prod/search-embedding-onnx",
        1_786_400_500_000,
    )
    .unwrap();
    assert!(registry.upsert_deployment(unapproved_runtime).is_err());
}

#[test]
fn contract_hash_is_stable_and_changes_with_executable_semantics() {
    let first = version(1);
    let same = version(1);
    assert_eq!(
        first.contract_sha256().unwrap(),
        same.contract_sha256().unwrap()
    );

    let mut changed = version(1);
    changed.input.document_template = "passage: {text}".to_string();
    assert_ne!(
        first.contract_sha256().unwrap(),
        changed.contract_sha256().unwrap()
    );

    let mut relocated = first.clone();
    relocated.artifact.uri = "az://models/bge-base/aed724fc5".to_string();
    assert_eq!(
        first.contract_sha256().unwrap(),
        relocated.contract_sha256().unwrap(),
        "artifact location is not content identity"
    );
}

#[test]
fn collection_embedding_binding_requires_asset_version_and_contract_hash_together() {
    let schema = CatalogTableSchema::new("documents")
        .with_column(CatalogColumn::new(
            1,
            "id",
            proximadb_data_model::ProximaType::String,
        ))
        .with_embedding_config(CatalogEmbeddingConfig {
            model: "search-embedding".to_string(),
            dimension: 768,
            model_asset_id: Some(42),
            ..Default::default()
        });

    let error = validate_schema(&schema).unwrap_err();
    assert!(error.to_string().contains("model version"));

    let valid = CatalogTableSchema::new("documents")
        .with_column(CatalogColumn::new(
            1,
            "id",
            proximadb_data_model::ProximaType::String,
        ))
        .with_embedding_config(
            CatalogEmbeddingConfig::pinned(
                "search-embedding",
                768,
                42,
                1,
                "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            )
            .unwrap(),
        );
    validate_schema(&valid).unwrap();
}

#[test]
fn optimistic_revision_rejects_stale_catalog_writers() {
    let mut registry = CatalogEmbeddingModelRegistry::new("search-embedding").unwrap();
    registry
        .apply(
            0,
            CatalogModelRegistryMutation::register_version(version(1)),
        )
        .unwrap();
    assert_eq!(registry.revision, 1);

    let error = registry
        .apply(0, CatalogModelRegistryMutation::set_alias("champion", 1))
        .unwrap_err();
    assert!(error.to_string().contains("revision conflict"));
    assert!(registry.aliases.is_empty());
}

#[test]
fn imported_registry_revalidates_nested_evidence_and_decisions() {
    let approval_without_evidence = CatalogModelDecision::new(
        "unsafe-approval",
        1,
        CatalogModelDecisionKind::Approved,
        vec![],
        "principal:reviewer",
        1,
    )
    .unwrap_err();
    assert!(approval_without_evidence.to_string().contains("evidence"));

    let mut registry = CatalogEmbeddingModelRegistry::new("search-embedding").unwrap();
    registry.register_version(version(1)).unwrap();
    registry.evidence.push(CatalogEvaluationEvidence {
        evidence_id: "imported-invalid".to_string(),
        version: 1,
        dataset_name: "rag-eval".to_string(),
        dataset_digest: DATASET_DIGEST.to_string(),
        evaluator: "eval@1".to_string(),
        metrics: BTreeMap::from([("recall".to_string(), f64::NAN)]),
        created_at_ms: 1,
    });
    assert!(registry.validate().is_err());
}

#[test]
fn legacy_catalog_rows_default_to_no_mlops_facet() {
    let legacy = serde_json::json!({
        "name": "legacy",
        "columns": [],
        "primary_key": [],
        "indexes": [],
        "schema_version": 1,
        "properties": {},
        "location": null,
        "created_at_ms": 0,
        "updated_at_ms": 0
    });

    let schema: CatalogTableSchema = serde_json::from_value(legacy).unwrap();
    assert!(schema.mlops_asset.is_none());
}

#[test]
fn opaque_schema_value_round_trips_numeric_model_version_keys() {
    let mut registry = CatalogEmbeddingModelRegistry::new("search-embedding").unwrap();
    registry.register_version(version(1)).unwrap();
    let schema = CatalogTableSchema::new("search-embedding")
        .with_mlops_asset(CatalogMlopsAsset::EmbeddingModel(registry))
        .unwrap();

    let CatalogMlopsAsset::EmbeddingModel(round_tripped) =
        schema.mlops_asset_as_typed().unwrap().unwrap();
    assert_eq!(round_tripped.version(1).unwrap().version, 1);
}

#[test]
fn python_sdk_golden_asset_is_native_serde_compatible() {
    let json = include_str!(
        "../../../../clients/python/tests/fixtures/embedding_model_xcatalog_asset.json"
    );
    let asset: CatalogMlopsAsset = serde_json::from_str(json).unwrap();
    asset.validate().unwrap();

    let CatalogMlopsAsset::EmbeddingModel(registry) = asset;
    assert_eq!(registry.name, "search-embedding");
    assert_eq!(registry.version(1).unwrap().input.special_token_count, 2);
}

#[tokio::test]
async fn native_xcatalog_persists_command_shaped_registry_mutations() {
    let tmp = tempfile::tempdir().unwrap();
    let config = NativeCatalogConfig {
        storage_url: tmp.path().to_string_lossy().to_string(),
        metadata_format: "json".to_string(),
        versioned: false,
        max_versions: 100,
    };
    let cache = Arc::new(proximadb_catalog::cache::CatalogCache::new(64, 60));
    let catalog = NativeCatalog::new("test".to_string(), config.clone(), cache)
        .await
        .unwrap();
    let namespace = vec!["tenant-a".to_string(), "mlops".to_string()];
    catalog
        .create_namespace(&namespace, HashMap::new())
        .await
        .unwrap();
    let id = TableIdentifier::new(namespace, "search-embedding");
    let asset = CatalogMlopsAsset::EmbeddingModel(
        CatalogEmbeddingModelRegistry::new("search-embedding").unwrap(),
    );
    catalog
        .create_table(
            &id,
            CatalogTableSchema::new("search-embedding")
                .with_mlops_asset(asset)
                .unwrap(),
        )
        .await
        .unwrap();

    catalog
        .apply_model_registry_mutation(
            &id,
            0,
            CatalogModelRegistryMutation::register_version(version(1)),
        )
        .await
        .unwrap();
    catalog
        .apply_model_registry_mutation(
            &id,
            1,
            CatalogModelRegistryMutation::set_alias("champion", 1),
        )
        .await
        .unwrap();

    drop(catalog);
    let reloaded = NativeCatalog::new(
        "test".to_string(),
        config,
        Arc::new(proximadb_catalog::cache::CatalogCache::new(64, 60)),
    )
    .await
    .unwrap();
    let schema = reloaded.get_table(&id).await.unwrap();
    let CatalogMlopsAsset::EmbeddingModel(registry) =
        schema.mlops_asset_as_typed().unwrap().unwrap();
    assert_eq!(registry.revision, 2);
    assert_eq!(registry.resolve_alias("champion").unwrap().version, 1);
}

#[test]
fn model_use_resolution_enforces_digest_dimension_approval_and_runtime() {
    let mut registry = CatalogEmbeddingModelRegistry::new("search-embedding").unwrap();
    registry.register_version(version(1)).unwrap();
    let digest = registry.version(1).unwrap().contract_sha256().unwrap();

    let unapproved = registry
        .resolve_use(
            1,
            &digest,
            768,
            &CatalogModelUsePolicy::approved_for_runtime("sentence-transformers"),
        )
        .unwrap_err();
    assert!(unapproved.to_string().contains("not approved"));

    registry
        .append_evidence(
            CatalogEvaluationEvidence::new(
                "eval-bge-1",
                1,
                "rag-retrieval-v3",
                DATASET_DIGEST,
                "proximadb-eval@2.1.0",
                BTreeMap::from([("recall_at_10".to_string(), 0.82)]),
                10,
            )
            .unwrap(),
        )
        .unwrap();
    registry
        .record_decision(
            CatalogModelDecision::new(
                "approve-bge-1",
                1,
                CatalogModelDecisionKind::Approved,
                vec!["eval-bge-1".to_string()],
                "principal:ml-reviewers",
                20,
            )
            .unwrap(),
        )
        .unwrap();

    let resolved = registry
        .resolve_use(
            1,
            &digest,
            768,
            &CatalogModelUsePolicy::approved_for_runtime("sentence-transformers"),
        )
        .unwrap();
    assert_eq!(resolved.version, 1);

    let wrong_digest = registry
        .resolve_use(
            1,
            "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            768,
            &CatalogModelUsePolicy::approved_for_runtime("sentence-transformers"),
        )
        .unwrap_err();
    assert!(wrong_digest.to_string().contains("contract digest"));

    let wrong_dimension = registry
        .resolve_use(
            1,
            &digest,
            384,
            &CatalogModelUsePolicy::approved_for_runtime("sentence-transformers"),
        )
        .unwrap_err();
    assert!(wrong_dimension.to_string().contains("dimension 384"));

    let wrong_runtime = registry
        .resolve_use(
            1,
            &digest,
            768,
            &CatalogModelUsePolicy::approved_for_runtime("onnx-runtime"),
        )
        .unwrap_err();
    assert!(wrong_runtime.to_string().contains("onnx-runtime"));

    registry
        .record_decision(
            CatalogModelDecision::new(
                "deprecate-bge-1",
                1,
                CatalogModelDecisionKind::Deprecated,
                vec![],
                "principal:ml-reviewers",
                30,
            )
            .unwrap(),
        )
        .unwrap();
    let deprecated = registry
        .resolve_use(
            1,
            &digest,
            768,
            &CatalogModelUsePolicy::approved_for_runtime("sentence-transformers"),
        )
        .unwrap_err();
    assert!(deprecated.to_string().contains("deprecated"));
}

#[tokio::test]
async fn catalog_resolves_a_collection_binding_to_one_immutable_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = NativeCatalog::new(
        "test".to_string(),
        NativeCatalogConfig {
            storage_url: tmp.path().to_string_lossy().to_string(),
            metadata_format: "json".to_string(),
            versioned: false,
            max_versions: 100,
        },
        Arc::new(proximadb_catalog::cache::CatalogCache::new(64, 60)),
    )
    .await
    .unwrap();
    let namespace = vec!["tenant-a".to_string(), "mlops".to_string()];
    catalog
        .create_namespace(&namespace, HashMap::new())
        .await
        .unwrap();
    let id = TableIdentifier::new(namespace, "search-embedding");
    let created = catalog
        .create_table(
            &id,
            CatalogTableSchema::new("search-embedding")
                .with_mlops_asset(CatalogMlopsAsset::EmbeddingModel(
                    CatalogEmbeddingModelRegistry::new("search-embedding").unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let asset_id = created.object_id.unwrap();
    catalog
        .apply_model_registry_mutation(
            &id,
            0,
            CatalogModelRegistryMutation::register_version(version(1)),
        )
        .await
        .unwrap();
    let digest = version(1).contract_sha256().unwrap();
    let binding =
        CatalogEmbeddingConfig::pinned("search-embedding", 768, asset_id, 1, digest.clone())
            .unwrap();

    let resolved = catalog
        .resolve_embedding_model_binding(&binding, &CatalogModelUsePolicy::registration_only())
        .await
        .unwrap();
    assert_eq!(resolved.asset_id, asset_id);
    assert_eq!(resolved.registry_name, "search-embedding");
    assert_eq!(resolved.contract_sha256, digest);
    assert_eq!(resolved.model.version, 1);

    let mismatched_name = CatalogEmbeddingConfig::pinned(
        "some-other-route",
        768,
        asset_id,
        1,
        resolved.contract_sha256,
    )
    .unwrap();
    let error = catalog
        .resolve_embedding_model_binding(
            &mismatched_name,
            &CatalogModelUsePolicy::registration_only(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("some-other-route"));
}

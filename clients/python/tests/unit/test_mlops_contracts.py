from __future__ import annotations

import json
from pathlib import Path

from proximadb_sdk.chunking_strategies import InputRenderer, ResolvedInputContract
from proximadb_sdk.integrations.mlops import (
    ArtifactDescriptor,
    AutoMLRunSpec,
    EmbeddingModelRegistration,
    ExperimentTrackerDDL,
    FeatureTableDDL,
    optional_integrations,
)


class _Counter:
    name = "BAAI/bge-base-en-v1.5"
    fingerprint = "c" * 64
    advertised_limit = 512

    def count(self, text: str) -> int:
        return len(text.split()) + 2

    def content_offsets(self, text: str):
        return None


def test_feature_table_ddl_emits_pgwire_catalog_contract() -> None:
    ddl = FeatureTableDDL(
        "Customer Churn Features",
        embedding_dimension=64,
        storage_engine="VIPER",
        physical_layout="columnar",
    )

    statements = ddl.statements()

    assert statements[0].startswith(
        'CREATE TABLE IF NOT EXISTS "customer_churn_features"'
    )
    assert "\"features\" JSONB NOT NULL DEFAULT '{}'::jsonb" in statements[0]
    assert '"embedding" VECTOR(64)' in statements[0]
    assert 'PRIMARY KEY ("entity_id", "event_time")' in statements[0]
    assert any(
        "xcatalog.namespace=features.customer_churn_features;engine=VIPER;layout=columnar;kind=feature_table"
        in statement
        for statement in statements
    )


def test_experiment_tracker_ddl_covers_runs_metrics_registry_predictions() -> None:
    statements = ExperimentTrackerDDL(prefix="victor").statements()

    assert any(
        'CREATE TABLE IF NOT EXISTS "victor_runs"' in statement
        for statement in statements
    )
    assert any(
        'CREATE TABLE IF NOT EXISTS "victor_metrics"' in statement
        for statement in statements
    )
    assert any(
        'CREATE TABLE IF NOT EXISTS "victor_params"' in statement
        for statement in statements
    )
    assert any(
        'CREATE TABLE IF NOT EXISTS "victor_models"' in statement
        for statement in statements
    )
    assert any(
        'CREATE TABLE IF NOT EXISTS "victor_predictions"' in statement
        for statement in statements
    )
    assert any("kind=experiment_runs" in statement for statement in statements)


def test_automl_spec_and_optional_integrations_are_dependency_light() -> None:
    spec = AutoMLRunSpec(
        framework="autogluon",
        target="churn",
        feature_table="customer_churn_features",
        label_column="label",
        eval_metric="roc_auc",
        time_limit_seconds=600,
    )

    metadata = spec.metadata()

    assert metadata["framework"] == "autogluon"
    assert metadata["feature_table"] == "customer_churn_features"
    availability = optional_integrations()
    assert set(availability) == {"mlflow", "autogluon", "pycaret"}
    assert all(isinstance(value, bool) for value in availability.values())


def test_resolved_embedding_contract_maps_to_native_xcatalog_asset() -> None:
    resolved = ResolvedInputContract(
        model_id="BAAI/bge-base-en-v1.5",
        model_revision="aed724fc5",
        counter=_Counter(),
        effective_context_limit=512,
        renderer=InputRenderer(
            document_template="{text}",
            query_template="Represent passages: {text}",
        ),
        native_dimension=768,
        output_dimension=768,
    )
    registration = EmbeddingModelRegistration.from_resolved_contract(
        registered_name="search-embedding",
        version=1,
        contract=resolved,
        artifact=ArtifactDescriptor(
            uri="hf://BAAI/bge-base-en-v1.5@aed724fc5",
            digest="sha256:" + "a" * 64,
            size_bytes=438_000_000,
            media_type="application/vnd.huggingface.repository.v1",
        ),
        created_at_ms=1_786_400_000_000,
        license_id="mit",
        approved_runtimes=("sentence-transformers",),
    )

    asset = registration.xcatalog_asset()
    version = asset["contract"]["versions"]["1"]
    assert asset["kind"] == "embedding_model"
    assert version["input"]["effective_context_limit"] == 512
    assert version["input"]["special_token_count"] == 2
    assert version["input"]["tokenizer_fingerprint"] == "sha256:" + "c" * 64
    assert version["output"]["dimension_policy"] == "fixed"
    assert version["governance"]["approved_runtimes"] == ["sentence-transformers"]
    fixture = Path(__file__).parents[1] / "fixtures/embedding_model_xcatalog_asset.json"
    assert asset == json.loads(fixture.read_text(encoding="utf-8"))


def test_matryoshka_contract_maps_discrete_and_range_dimension_policies() -> None:
    discrete = ResolvedInputContract(
        model_id="nomic-ai/nomic-embed-text-v1.5",
        model_revision="model-sha",
        counter=_Counter(),
        effective_context_limit=512,
        native_dimension=768,
        supported_output_dimensions=(768, 512, 256),
    )
    ranged = ResolvedInputContract(
        model_id="Qwen/Qwen3-Embedding-0.6B",
        model_revision="model-sha",
        counter=_Counter(),
        effective_context_limit=512,
        native_dimension=1024,
        minimum_output_dimension=32,
    )
    artifact = ArtifactDescriptor(
        "hf://model@sha", "sha256:" + "a" * 64, 1, "application/octet-stream"
    )

    discrete_payload = EmbeddingModelRegistration.from_resolved_contract(
        "nomic", 1, discrete, artifact, 1, approved_runtimes=("st",)
    ).model_version()["output"]
    range_payload = EmbeddingModelRegistration.from_resolved_contract(
        "qwen", 1, ranged, artifact, 1, approved_runtimes=("st",)
    ).model_version()["output"]

    assert discrete_payload["dimension_policy"] == "discrete"
    assert range_payload["dimension_policy"] == {"range": {"minimum": 32}}


def test_artifact_descriptor_requires_content_addressing() -> None:
    try:
        ArtifactDescriptor("hf://model@main", "main", 1, "application/octet-stream")
    except ValueError as error:
        assert "sha256" in str(error)
    else:
        raise AssertionError("mutable artifact reference must be rejected")

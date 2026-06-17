from __future__ import annotations

from proximadb_sdk.integrations.mlops import (
    AutoMLRunSpec,
    ExperimentTrackerDDL,
    FeatureTableDDL,
    optional_integrations,
)


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
